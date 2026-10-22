//! Taking part in the poll (`companion/PROTOCOL.md`; the mod's `tick` and `publish` in
//! `hooks/register.tsx`): whether `prw` takes the turn at all (the give-way rule), the lease,
//! which PRs a round asks, and what a round's answers do to `results.json`.
//!
//! The round's merge works on the file's raw JSON: it sets the fields a round sets and the
//! entries it fetched, and passes everything else on as it was read.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value, json};

use crate::batch::{PrRef, ref_from_url};
use crate::entries::{failed_entry, fetched_entry, is_finished, is_unreachable, kept_entries};
use crate::gh::GhPr;
use crate::lenient::{map_of, ms_of};
use crate::protocol::{Lease, LoginRates};
use crate::rate::{Rates, legacy_rate, round_waits_until, with_seen};
use crate::status::derive;
use crate::ui::{Session, is_live};

/// How long a turn lasts (ms).
pub const LEASE_MS: i64 = 30_000;

/// A live mod gives way once the turn has been free this long past `nextAt` (ms): two of the
/// mod's default idle intervals.
pub const FREE_MS: i64 = 120_000;

/// The poll's pace (ms): the next round this long after one, while any PR moves or none does
/// (the mod's `poll_active_seconds` and `poll_idle_seconds`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    pub fast_ms: i64,
    pub slow_ms: i64,
}

impl Default for Pace {
    fn default() -> Self {
        Self {
            fast_ms: 20_000,
            slow_ms: 60_000,
        }
    }
}

/// Whether `prw` takes part in the poll at `at` (`should_poll`): no live session is a mod's,
/// or one is but the turn has been free for [`FREE_MS`] past `nextAt` (a mod too old to poll
/// for others, or stuck). Taking part still means waiting for `nextAt` and taking the lease.
#[must_use]
pub fn should_poll(sessions: &[Session], lease: Option<&Lease>, next_at: i64, at: i64) -> bool {
    let is_mod_live = sessions.iter().any(|s| s.is_mod() && is_live(s.mtime, at));
    let is_free = lease.is_none_or(|l| l.until <= at);
    !is_mod_live || (is_free && at.saturating_sub(next_at) > FREE_MS)
}

/// Whether `own` may claim the lease (`takeLease`'s first read): nobody else holds it now.
#[must_use]
pub fn may_take(lease: Option<&Lease>, own: &str, at: i64) -> bool {
    lease.is_none_or(|l| l.owner == own || l.until <= at)
}

/// The lease `own` writes to claim the turn at `at`.
#[must_use]
pub fn claim(own: &str, at: i64) -> Lease {
    Lease {
        owner: own.to_owned(),
        until: at.saturating_add(LEASE_MS),
    }
}

/// The lease `own` writes to give the turn back.
#[must_use]
pub fn release(own: &str) -> Lease {
    Lease {
        owner: own.to_owned(),
        until: 0,
    }
}

/// The PRs a round asks (`tick`'s `due`): every URL that names a PR, minus finished ones and
/// ones no account could see lately, the least recently asked first.
#[must_use]
pub fn due(urls: &[String], prs: &Map<String, Value>, at: i64) -> Vec<PrRef> {
    let asked_at = |url: &str| {
        let entry = prs.get(url);
        let field = |key: &str| entry.and_then(|e| e.get(key)).and_then(ms_of);
        field("failedSince").or_else(|| field("at")).unwrap_or(0)
    };
    let mut refs: Vec<PrRef> = urls
        .iter()
        .filter_map(|url| ref_from_url(url))
        .filter(|r| {
            let entry = prs.get(&r.url);
            !is_unreachable(entry, at) && !is_finished(entry)
        })
        .collect();
    refs.sort_by_key(|r| asked_at(&r.url));
    refs
}

/// Why a PR's fetch failed this round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub error: String,
    /// No logged-in gh account could see it.
    pub is_miss: bool,
}

/// What a fetch found, for [`merge`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// By URL, each PR as `parseReply` stores it.
    pub found: BTreeMap<String, Map<String, Value>>,
    pub failed: BTreeMap<String, Failure>,
    /// Repo owner → login, as the fetch left it (the file's, with what it learned).
    pub accounts: BTreeMap<String, String>,
    /// Each login's buckets as the fetch saw them.
    pub rates: Rates,
    /// The logins these PRs are polled as.
    pub needed: Vec<String>,
    /// A secondary limit: how long everyone holds off (ms).
    pub wait_ms: Option<i64>,
}

/// What a round is, for [`merge`]: the PRs live sessions watch (the rest age out), and the
/// pace of the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round<'a> {
    pub live: &'a HashSet<String>,
    pub pace: Pace,
}

/// `results.json` as read just before writing (`raw`), with a fetch's `outcome` merged in
/// (`publish`). `at`: when the fetch started; `now`: when it ended. With a `round`, also the
/// round's own fields (`fetchedAt`, `nextAt`, the entries it keeps).
#[must_use]
pub fn merge(
    mut raw: Map<String, Value>,
    outcome: &Outcome,
    round: Option<&Round<'_>>,
    at: i64,
    now: i64,
) -> Map<String, Value> {
    let defaults = [
        ("fetchedAt", json!(0)),
        ("nextAt", json!(0)),
        ("prs", json!({})),
        ("accounts", json!({})),
        ("rate", Value::Null),
        ("rates", json!({})),
        ("latest", json!({"version": null, "checkedAt": 0})),
    ];
    for (key, value) in defaults {
        raw.entry(key).or_insert(value);
    }
    let mut accounts = object(raw.remove("accounts"));
    for (owner, login) in &outcome.accounts {
        accounts.insert(owner.clone(), login.clone().into());
    }
    let mut prs = object(raw.remove("prs"));
    for (url, pr) in &outcome.found {
        prs.insert(url.clone(), fetched_entry(pr.clone(), at));
    }
    for (url, failure) in &outcome.failed {
        let entry = failed_entry(prs.get(url), &failure.error, at, failure.is_miss);
        prs.insert(url.clone(), entry);
    }
    // Each login's latest sighting wins, whichever session made it. Only the logins seen are
    // rewritten; the others pass on as they were.
    let mut raw_rates = object(raw.remove("rates"));
    let rates = outcome.rates.iter().fold(
        map_of::<LoginRates>(Value::Object(raw_rates.clone())),
        |rates, (login, seen)| with_seen(rates, login, seen),
    );
    for login in outcome.rates.keys() {
        raw_rates.insert(login.clone(), json!(rates[login]));
    }
    if let Some(rate) = legacy_rate(&rates) {
        raw.insert("rate".into(), json!(rate));
    }
    let held_until = outcome
        .wait_ms
        .map(|wait| {
            let was = raw.get("heldUntil").and_then(ms_of).unwrap_or(0);
            was.max(now.saturating_add(wait))
        })
        .or_else(|| raw.get("heldUntil").and_then(ms_of));
    let mut next_at = raw.get("nextAt").and_then(ms_of).unwrap_or(0);
    if let Some(round) = round {
        prs = kept_entries(prs, round.live, at);
        let is_moving = outcome.found.values().any(|pr| {
            serde_json::from_value::<GhPr>(Value::Object(pr.clone()))
                .is_ok_and(|pr| derive(&pr, None).is_moving)
        });
        let pace = if is_moving {
            round.pace.fast_ms
        } else {
            round.pace.slow_ms
        };
        raw.insert("fetchedAt".into(), at.into());
        next_at = at.saturating_add(pace);
        // Only once every login the round needs is low does the round itself wait.
        if let Some(reset) = round_waits_until(&rates, &outcome.needed, at) {
            next_at = next_at.max(reset);
        }
    }
    if let Some(held) = held_until {
        raw.insert("heldUntil".into(), held.into());
        next_at = next_at.max(held);
    }
    raw.insert("nextAt".into(), next_at.into());
    raw.insert("prs".into(), Value::Object(prs));
    raw.insert("accounts".into(), Value::Object(accounts));
    raw.insert("rates".into(), Value::Object(raw_rates));
    raw
}

/// A field's object, or an empty one where it isn't one.
fn object(value: Option<Value>) -> Map<String, Value> {
    match value {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::NOT_FOUND;
    use crate::protocol::{Bucket, SessionFile, Writer};

    const T: i64 = 1_791_374_400_000;
    const A: &str = "https://github.com/acme/app/pull/1";
    const B: &str = "https://github.com/acme/app/pull/2";
    const C: &str = "https://github.com/acme/web/pull/3";

    fn session(writer: Option<Writer>, mtime: i64) -> Session {
        Session {
            id: "s".into(),
            file: SessionFile {
                writer,
                ..SessionFile::default()
            },
            mtime,
        }
    }

    fn lease(owner: &str, until: i64) -> Lease {
        Lease {
            owner: owner.into(),
            until,
        }
    }

    #[test]
    fn prw_polls_when_no_mod_is_live() {
        assert!(should_poll(&[], None, T, T));
        // A prw session, or a mod's that has gone quiet, is no mod polling.
        let quiet = [
            session(Some(Writer::Prw), T),
            session(Some(Writer::Mod), T - 60_000),
        ];
        assert!(should_poll(&quiet, Some(&lease("x", T + 1)), T, T));
    }

    #[test]
    fn a_live_mod_polls_unless_the_turn_has_been_free_too_long() {
        for writer in [Some(Writer::Mod), None] {
            let live = [session(writer, T)];
            assert!(!should_poll(&live, None, T, T));
            assert!(!should_poll(&live, None, T - FREE_MS, T));
            assert!(should_poll(&live, None, T - FREE_MS - 1, T));
            assert!(should_poll(&live, Some(&lease("m", T)), T - FREE_MS - 1, T));
            assert!(!should_poll(
                &live,
                Some(&lease("m", T + 1)),
                T - FREE_MS - 1,
                T
            ));
        }
    }

    #[test]
    fn the_lease_is_free_when_unheld_lapsed_or_ours() {
        assert!(may_take(None, "me", T));
        assert!(may_take(Some(&lease("other", T)), "me", T));
        assert!(may_take(Some(&lease("me", T + 5)), "me", T));
        assert!(!may_take(Some(&lease("other", T + 1)), "me", T));
        assert_eq!(claim("me", T), lease("me", T + LEASE_MS));
        assert_eq!(release("me"), lease("me", 0));
    }

    fn prs(entries: &[(&str, Value)]) -> Map<String, Value> {
        entries
            .iter()
            .map(|(url, e)| ((*url).to_owned(), e.clone()))
            .collect()
    }

    #[test]
    fn a_round_asks_the_least_recently_asked_first_never_the_finished_or_unseen() {
        let entries = prs(&[
            (A, json!({"at": T - 10, "pr": {"state": "OPEN"}})),
            (
                B,
                json!({"at": T - 5, "pr": {"state": "OPEN"}, "failedSince": T - 50}),
            ),
            (C, json!({"at": T - 99, "pr": {"state": "MERGED"}})),
            (
                "https://github.com/acme/app/pull/4",
                json!({"at": T - 1, "error": NOT_FOUND}),
            ),
        ]);
        let urls: Vec<String> = [
            A,
            B,
            C,
            "https://github.com/acme/app/pull/4",
            "https://github.com/acme/app/pull/5",
            "nonsense",
        ]
        .iter()
        .map(|&u| u.to_owned())
        .collect();
        let asked: Vec<String> = due(&urls, &entries, T).into_iter().map(|r| r.url).collect();
        assert_eq!(asked, ["https://github.com/acme/app/pull/5", B, A]);
    }

    fn found(state: &str) -> Map<String, Value> {
        let Value::Object(pr) = json!({"number": 1, "state": state, "unknownToPrw": [1, 2]}) else {
            unreachable!()
        };
        pr
    }

    #[test]
    fn a_fetch_merges_its_prs_and_passes_on_the_rest() {
        let raw = prs(&[
            ("future", json!({"a": 1})),
            ("latest", json!({"version": "0.14.1", "checkedAt": 5})),
            ("nextAt", json!(T - 1)),
            ("accounts", json!({"acme": "hubot", "globex": "octocat"})),
            (
                "prs",
                json!({B: {"at": T - 1_000, "pr": {"state": "OPEN"}}, C: {"at": 1, "x": true}}),
            ),
        ]);
        let outcome = Outcome {
            found: [(A.to_owned(), found("OPEN"))].into(),
            failed: [(
                B.to_owned(),
                Failure {
                    error: "gh: HTTP 502".into(),
                    is_miss: false,
                },
            )]
            .into(),
            accounts: [("acme".to_owned(), "octocat".to_owned())].into(),
            ..Outcome::default()
        };
        let out = merge(raw, &outcome, None, T, T + 3_000);
        assert_eq!(out["future"], json!({"a": 1}));
        assert_eq!(out["latest"], json!({"version": "0.14.1", "checkedAt": 5}));
        assert_eq!(
            out["accounts"],
            json!({"acme": "octocat", "globex": "octocat"})
        );
        assert_eq!(out["prs"][A], json!({"at": T, "pr": found("OPEN")}));
        assert_eq!(out["prs"][B]["failedSince"], json!(T));
        // Not a round: nothing ages out, and the round's own fields stand.
        assert_eq!(out["prs"][C], json!({"at": 1, "x": true}));
        assert_eq!(out["nextAt"], json!(T - 1));
        assert_eq!(out["fetchedAt"], json!(0));
        assert_eq!(out.get("heldUntil"), None);
    }

    #[test]
    fn a_round_sets_its_time_ages_out_the_unwatched_and_paces_by_movement() {
        let raw = prs(&[(
            "prs",
            json!({B: {"at": T - 2 * crate::entries::KEEP_MS}, C: {"at": T - 2 * crate::entries::KEEP_MS}}),
        )]);
        let live: HashSet<String> = [A.to_owned(), C.to_owned()].into();
        let round = Round {
            live: &live,
            pace: Pace::default(),
        };
        let idle = Outcome {
            found: [(A.to_owned(), found("OPEN"))].into(),
            ..Outcome::default()
        };
        let out = merge(raw.clone(), &idle, Some(&round), T, T + 1);
        assert_eq!(out["fetchedAt"], json!(T));
        assert_eq!(out["nextAt"], json!(T + 60_000));
        let kept: Vec<&String> = out["prs"].as_object().unwrap().keys().collect();
        assert_eq!(kept, [A, C]);
        // Checks running: the next round comes sooner.
        let mut busy = found("OPEN");
        busy.insert(
            "statusCheckRollup".into(),
            json!([{"__typename": "CheckRun", "name": "ci", "status": "IN_PROGRESS"}]),
        );
        let moving = Outcome {
            found: [(A.to_owned(), busy)].into(),
            ..Outcome::default()
        };
        assert_eq!(
            merge(raw, &moving, Some(&round), T, T + 1)["nextAt"],
            json!(T + 20_000)
        );
    }

    #[test]
    fn a_hold_only_moves_later_and_the_next_round_waits_for_it_and_for_low_logins() {
        let raw = prs(&[("heldUntil", json!(T + 90_000))]);
        let live = HashSet::new();
        let round = Round {
            live: &live,
            pace: Pace::default(),
        };
        let short = Outcome {
            wait_ms: Some(30_000),
            ..Outcome::default()
        };
        let out = merge(raw.clone(), &short, Some(&round), T, T + 1_000);
        assert_eq!(out["heldUntil"], json!(T + 90_000));
        assert_eq!(out["nextAt"], json!(T + 90_000));
        let long = Outcome {
            wait_ms: Some(120_000),
            ..Outcome::default()
        };
        assert_eq!(
            merge(raw, &long, Some(&round), T, T + 1_000)["heldUntil"],
            json!(T + 121_000)
        );
        let low = Outcome {
            needed: vec!["octocat".into()],
            rates: [(
                "octocat".to_owned(),
                LoginRates {
                    graphql: Some(Bucket {
                        limit: 5_000,
                        remaining: 10,
                        reset_at: T + 600_000,
                        seen_at: T,
                    }),
                    ..LoginRates::default()
                },
            )]
            .into(),
            ..Outcome::default()
        };
        let out = merge(Map::new(), &low, Some(&round), T, T);
        assert_eq!(out["nextAt"], json!(T + 600_000));
        assert_eq!(out["rate"]["remaining"], json!(10));
        assert_eq!(out["rates"]["octocat"]["graphql"]["remaining"], json!(10));
    }

    #[test]
    fn the_logins_a_fetch_saw_are_rewritten_and_the_others_pass_on() {
        let raw = prs(&[(
            "rates",
            json!({"hubot": {"askedAt": 1, "future": true}, "octocat": {"askedAt": 2}}),
        )]);
        let seen = Outcome {
            rates: [(
                "octocat".to_owned(),
                LoginRates {
                    asked_at: Some(3),
                    ..LoginRates::default()
                },
            )]
            .into(),
            ..Outcome::default()
        };
        let out = merge(raw, &seen, None, T, T);
        assert_eq!(
            out["rates"],
            json!({"hubot": {"askedAt": 1, "future": true}, "octocat": {"askedAt": 3}})
        );
    }

    #[test]
    fn an_empty_file_gets_the_mods_empty_fields() {
        let out = merge(Map::new(), &Outcome::default(), None, T, T);
        assert_eq!(
            Value::Object(out),
            json!({"fetchedAt": 0, "nextAt": 0, "prs": {}, "accounts": {}, "rate": null, "rates": {}, "latest": {"version": null, "checkedAt": 0}})
        );
    }
}
