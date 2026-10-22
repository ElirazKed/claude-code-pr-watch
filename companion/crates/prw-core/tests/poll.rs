//! Properties of taking part in the poll: `should_poll` never takes a live mod's held turn
//! and only grows more willing as time passes; a round's merge passes on what it doesn't
//! set; a failed fetch never loses a PR's data; the parsers of GitHub's replies are total.

use std::collections::HashSet;

use proptest::prelude::*;
use prw_core::batch::{parse_http, parse_reply, ref_from_url};
use prw_core::entries::{failed_entry, kept_entries};
use prw_core::poll::{Failure, Outcome, Pace, Round, due, merge, should_poll};
use prw_core::protocol::{Lease, SessionFile, Writer};
use prw_core::search::{MINE, parse_searches};
use prw_core::ui::Session;
use serde_json::{Map, Value, json};

const T: i64 = 1_791_374_400_000;

/// JSON of any shape, a few levels deep.
fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        "[a-zA-Z0-9 ]{0,8}".prop_map(Value::from),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map("[a-z]{1,6}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

fn url() -> impl Strategy<Value = String> {
    prop_oneof![
        (1_u64..6).prop_map(|n| format!("https://github.com/acme/app/pull/{n}")),
        Just("nonsense".to_owned()),
    ]
}

fn writer() -> impl Strategy<Value = Option<Writer>> {
    prop_oneof![Just(None), Just(Some(Writer::Mod)), Just(Some(Writer::Prw))]
}

fn sessions() -> impl Strategy<Value = Vec<Session>> {
    prop::collection::vec(
        (writer(), -200_000_i64..1_000).prop_map(|(writer, ago)| Session {
            id: "s".into(),
            file: SessionFile {
                writer,
                ..SessionFile::default()
            },
            mtime: T + ago,
        }),
        0..4,
    )
}

fn lease() -> impl Strategy<Value = Option<Lease>> {
    proptest::option::of(
        (prop_oneof![Just("mod"), Just("prw")], -60_000_i64..60_000).prop_map(|(owner, d)| Lease {
            owner: owner.into(),
            until: T + d,
        }),
    )
}

/// An entry that may or may not have data, and odd fields beside.
fn entry() -> impl Strategy<Value = Value> {
    (
        proptest::option::of(any_json()),
        proptest::option::of(-400_000_i64..0),
        proptest::option::of("[a-z ]{0,10}"),
        proptest::option::of(-400_000_i64..0),
        any_json(),
    )
        .prop_map(|(pr, at, error, since, other)| {
            let mut e = Map::new();
            if let Some(pr) = pr {
                e.insert("pr".into(), pr);
            }
            if let Some(at) = at {
                e.insert("at".into(), (T + at).into());
            }
            if let Some(error) = error {
                e.insert("error".into(), error.into());
            }
            if let Some(since) = since {
                e.insert("failedSince".into(), (T + since).into());
            }
            e.insert("other".into(), other);
            Value::Object(e)
        })
}

fn prs() -> impl Strategy<Value = Map<String, Value>> {
    prop::collection::btree_map(url(), entry(), 0..5).prop_map(|m| m.into_iter().collect())
}

proptest! {
    #[test]
    fn should_poll_never_takes_a_live_mods_held_turn(sessions in sessions(), next_at in -300_000_i64..300_000) {
        let is_mod_live = sessions.iter().any(|s| s.is_mod() && T - s.mtime < 60_000);
        let held = Lease { owner: "mod".into(), until: T + 1 };
        if is_mod_live {
            prop_assert!(!should_poll(&sessions, Some(&held), T + next_at, T));
        }
    }

    #[test]
    fn should_poll_only_grows_more_willing_with_time(sessions in sessions(), lease in lease(), next_at in -300_000_i64..300_000, later in 0_i64..400_000) {
        if should_poll(&sessions, lease.as_ref(), T + next_at, T) {
            prop_assert!(should_poll(&sessions, lease.as_ref(), T + next_at, T + later));
        }
    }

    #[test]
    fn a_failed_fetch_never_loses_a_prs_data_or_its_time(entry in entry(), error in "[a-z ]{1,10}", later in 0_i64..400_000, is_miss in any::<bool>()) {
        let after = failed_entry(Some(&entry), &error, T + later, is_miss);
        if entry.get("pr").is_some() {
            prop_assert_eq!(after.get("pr"), entry.get("pr"));
            prop_assert_eq!(after.get("at"), entry.get("at"));
            prop_assert_eq!(after.get("other"), entry.get("other"));
            prop_assert!(after.get("failedSince").and_then(Value::as_i64).is_some_and(|s| s <= T + later));
            prop_assert_eq!(after.get("isUnreachable"), None);
        } else {
            prop_assert_eq!(after.get("error"), Some(&Value::from(error)));
            prop_assert_eq!(after.get("at"), Some(&Value::from(T + later)));
        }
    }

    #[test]
    fn a_round_keeps_every_live_pr_and_drops_none_it_should_keep(prs in prs(), live in prop::collection::hash_set(url(), 0..4)) {
        let kept = kept_entries(prs.clone(), &live, T);
        for url in prs.keys() {
            if live.contains(url) {
                prop_assert!(kept.contains_key(url));
            }
        }
        prop_assert!(kept.keys().all(|url| prs.contains_key(url)));
    }

    #[test]
    fn due_asks_only_open_reachable_prs_least_recently_asked_first(prs in prs(), urls in prop::collection::hash_set(url(), 0..6)) {
        let urls: Vec<String> = urls.into_iter().collect();
        let asked = due(&urls, &prs, T);
        prop_assert!(asked.iter().all(|r| urls.contains(&r.url) && ref_from_url(&r.url).is_some()));
        let when = |url: &str| {
            let e = prs.get(url);
            let f = |k: &str| e.and_then(|e| e.get(k)).and_then(Value::as_i64);
            f("failedSince").or_else(|| f("at")).unwrap_or(0)
        };
        prop_assert!(asked.windows(2).all(|w| when(&w[0].url) <= when(&w[1].url)));
    }

    #[test]
    fn a_merge_passes_on_every_top_level_field_it_doesnt_set(extra in prop::collection::btree_map("[a-z]{3,8}", any_json(), 0..4), prs in prs(), found in prop::collection::btree_map(url(), any_json(), 0..3), failed in prop::collection::btree_map(url(), "[a-z]{1,6}", 0..3), is_round in any::<bool>()) {
        const SET: [&str; 8] = ["fetchedAt", "nextAt", "prs", "accounts", "rate", "rates", "latest", "heldUntil"];
        let mut raw: Map<String, Value> = extra.clone().into_iter().collect();
        raw.insert("prs".into(), Value::Object(prs.clone()));
        let outcome = Outcome {
            found: found
                .clone()
                .into_iter()
                .filter_map(|(u, pr)| match pr { Value::Object(pr) => Some((u, pr)), _ => None })
                .collect(),
            // A fetch finds a PR or fails it, never both.
            failed: failed
                .into_iter()
                .filter(|(u, _)| !found.contains_key(u))
                .map(|(u, error)| (u, Failure { error, is_miss: false }))
                .collect(),
            ..Outcome::default()
        };
        let live: HashSet<String> = prs.keys().cloned().collect();
        let round = Round { live: &live, pace: Pace::default() };
        let out = merge(raw, &outcome, is_round.then_some(&round), T, T + 1);
        for (key, value) in &extra {
            if !SET.contains(&key.as_str()) {
                prop_assert_eq!(out.get(key), Some(value));
            }
        }
        // Every PR it neither fetched nor failed is as it was (all are live here).
        let out_prs = out["prs"].as_object().unwrap();
        for (url, entry) in &prs {
            if !outcome.found.contains_key(url) && !outcome.failed.contains_key(url) {
                prop_assert_eq!(out_prs.get(url), Some(entry));
            }
        }
        for (url, pr) in &outcome.found {
            prop_assert_eq!(&out_prs[url], &json!({"at": T, "pr": pr}));
        }
    }

    #[test]
    fn the_reply_parsers_are_total(stdout in any::<String>(), stderr in any::<String>()) {
        let refs = [ref_from_url("https://github.com/acme/app/pull/1").unwrap()];
        let http = parse_http(&stdout);
        prop_assert!(stdout.ends_with(&http.body));
        let reply = parse_reply(&http.body, &stderr, &refs);
        prop_assert!(reply.error.is_some() || reply.found.len() + reply.missing.len() == refs.len());
        let _ = parse_searches(&stdout, &stderr, &[MINE]);
    }

    #[test]
    fn every_pr_asked_is_found_or_missing_whatever_github_sent(data in any_json(), n in 1_usize..4) {
        let refs: Vec<_> = (1..=n).map(|i| ref_from_url(&format!("https://github.com/acme/app/pull/{i}")).unwrap()).collect();
        let reply = parse_reply(&json!({"data": data}).to_string(), "", &refs);
        if reply.error.is_none() {
            prop_assert_eq!(reply.found.len() + reply.missing.len(), refs.len());
        }
    }
}
