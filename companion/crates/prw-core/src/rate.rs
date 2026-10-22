//! Each gh login's hourly budget (`hooks/rate.ts`), as a round keeps it in `results.json`, and
//! what holds the poll back. GraphQL counts points (the poll's own replies say how many are
//! left, for free); REST counts requests (GitHub's `/rate_limit`, which costs nothing).

use std::collections::BTreeMap;

use serde_json::Value;

use crate::protocol::{Bucket, LoginRates, RateLimit};
use crate::time::{format_iso, parse_iso};

/// By login, as gh names it.
pub type Rates = BTreeMap<String, LoginRates>;

/// A login with fewer points than this left is not polled until its reset.
pub const LOW_RATE: i64 = 300;
/// How often a round asks `/rate_limit` for each login (ms).
pub const REST_MS: i64 = 300_000;
/// GitHub's hourly limit for a person's token, where a reply doesn't say.
const DEFAULT_LIMIT: i64 = 5_000;

/// A bucket from a reply's headers or its `rateLimit` field, seen at `at` (`bucketOf`).
#[must_use]
pub fn bucket_of(rate: Option<&RateLimit>, at: i64) -> Option<Bucket> {
    let rate = rate?;
    Some(Bucket {
        limit: rate.limit.unwrap_or(DEFAULT_LIMIT),
        remaining: rate.remaining,
        reset_at: parse_iso(&rate.reset_at)?,
        seen_at: at,
    })
}

/// `gh api rate_limit`'s reply (`restOf`): REST's requests and GraphQL's points, each reset
/// in epoch seconds; `None` where it says neither.
#[must_use]
pub fn rest_of(stdout: &str, at: i64) -> Option<LoginRates> {
    let reply: Value = serde_json::from_str(stdout).ok()?;
    let one = |name: &str| -> Option<Bucket> {
        let b = reply.get("resources")?.get(name)?;
        let field = |key: &str| b.get(key).and_then(Value::as_f64);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "GitHub's counts and reset times are whole numbers far inside i64"
        )]
        Some(Bucket {
            limit: field("limit")? as i64,
            remaining: field("remaining")? as i64,
            reset_at: (field("reset")? * 1_000.0) as i64,
            seen_at: at,
        })
    };
    let (core, graphql) = (one("core"), one("graphql"));
    (core.is_some() || graphql.is_some()).then_some(LoginRates {
        graphql,
        core,
        asked_at: None,
    })
}

/// The later of two sightings of a bucket.
fn later(a: Option<Bucket>, b: Option<Bucket>) -> Option<Bucket> {
    match (a, b) {
        (None, b) => b,
        (Some(a), Some(b)) if a.seen_at < b.seen_at => Some(b),
        (a, _) => a,
    }
}

/// `rates` with what was just seen of `login` folded in (`withSeen`): the later sighting of
/// each bucket, and when `/rate_limit` was last asked.
#[must_use]
pub fn with_seen(mut rates: Rates, login: &str, seen: &LoginRates) -> Rates {
    let was = rates.remove(login).unwrap_or_default();
    let asked_at = match (seen.asked_at, was.asked_at) {
        (None, was) => was,
        (Some(seen), Some(was)) if was > seen => Some(was),
        (seen, _) => seen,
    };
    rates.insert(
        login.to_owned(),
        LoginRates {
            graphql: later(seen.graphql, was.graphql),
            core: later(seen.core, was.core),
            asked_at,
        },
    );
    rates
}

/// Whether a round asks `/rate_limit` for `login` now (`isRestDue`): [`REST_MS`] after it last
/// did, answered or not.
#[must_use]
pub fn is_rest_due(rates: &Rates, login: &str, at: i64) -> bool {
    let asked = rates.get(login).and_then(|r| r.asked_at).unwrap_or(0);
    at.saturating_sub(asked) >= REST_MS
}

/// Until when `login` is left out of the poll (`lowUntil`): while its points are low, until
/// their reset. `None`: it isn't.
#[must_use]
pub fn low_until(rates: &Rates, login: &str, at: i64) -> Option<i64> {
    let points = rates.get(login)?.graphql?;
    (points.remaining < LOW_RATE && points.reset_at > at).then_some(points.reset_at)
}

/// Until when the whole round waits (`roundWaitsUntil`): only once every login it needs is
/// low, and then until the first of them resets. `None`: some login still has points.
#[must_use]
pub fn round_waits_until(rates: &Rates, logins: &[String], at: i64) -> Option<i64> {
    let resets: Option<Vec<i64>> = logins.iter().map(|l| low_until(rates, l, at)).collect();
    resets?.into_iter().min()
}

/// The `rate` field older versions read (`legacyRate`): the login with the most points left.
#[must_use]
pub fn legacy_rate(rates: &Rates) -> Option<RateLimit> {
    let best = rates
        .values()
        .filter_map(|r| r.graphql)
        .fold(None, |best: Option<Bucket>, b| match best {
            Some(best) if best.remaining >= b.remaining => Some(best),
            _ => Some(b),
        })?;
    Some(RateLimit {
        remaining: best.remaining,
        reset_at: format_iso(best.reset_at)?,
        limit: Some(best.limit),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_791_374_400_000;

    fn bucket(remaining: i64, reset_at: i64, seen_at: i64) -> Bucket {
        Bucket {
            limit: 5_000,
            remaining,
            reset_at,
            seen_at,
        }
    }

    fn points(remaining: i64, reset_at: i64) -> LoginRates {
        LoginRates {
            graphql: Some(bucket(remaining, reset_at, T)),
            ..LoginRates::default()
        }
    }

    #[test]
    fn a_replys_rate_is_a_bucket_and_a_bad_one_none() {
        let rate = RateLimit {
            remaining: 4_961,
            reset_at: "2026-10-07T13:00:00Z".into(),
            limit: None,
        };
        assert_eq!(
            bucket_of(Some(&rate), T),
            Some(Bucket {
                limit: 5_000,
                remaining: 4_961,
                reset_at: 1_791_378_000_000,
                seen_at: T
            })
        );
        let bad = RateLimit {
            reset_at: "soon".into(),
            ..rate
        };
        assert_eq!(bucket_of(Some(&bad), T), None);
        assert_eq!(bucket_of(None, T), None);
    }

    #[test]
    fn rate_limits_reply_gives_both_buckets() {
        let stdout = r#"{"resources": {"core": {"limit": 5000, "remaining": 4900, "reset": 1791378000, "used": 100}, "graphql": {"limit": 5000, "remaining": 4000, "reset": 1791378060}}}"#;
        let read = rest_of(stdout, T).unwrap();
        assert_eq!(read.core, Some(bucket(4_900, 1_791_378_000_000, T)));
        assert_eq!(read.graphql, Some(bucket(4_000, 1_791_378_060_000, T)));
        assert_eq!(rest_of(r#"{"resources": {"search": {}}}"#, T), None);
        assert_eq!(rest_of("nope", T), None);
        let core_only = rest_of(
            r#"{"resources": {"core": {"limit": 1, "remaining": 1, "reset": 1}, "graphql": {"limit": "x"}}}"#,
            T,
        )
        .unwrap();
        assert_eq!(core_only.graphql, None);
    }

    #[test]
    fn the_later_sighting_of_each_bucket_wins_and_the_last_ask() {
        let rates: Rates = [(
            "octocat".to_owned(),
            LoginRates {
                graphql: Some(bucket(4_000, T + 60_000, T)),
                core: Some(bucket(4_900, T + 60_000, T + 10)),
                asked_at: Some(T),
            },
        )]
        .into();
        let seen = LoginRates {
            graphql: Some(bucket(3_990, T + 60_000, T + 5)),
            core: Some(bucket(4_950, T + 60_000, T + 5)),
            asked_at: None,
        };
        let after = with_seen(rates, "octocat", &seen);
        let now = &after["octocat"];
        assert_eq!(now.graphql.map(|b| b.remaining), Some(3_990));
        assert_eq!(now.core.map(|b| b.remaining), Some(4_900));
        assert_eq!(now.asked_at, Some(T));
        let later_ask = LoginRates {
            asked_at: Some(T + 1),
            ..LoginRates::default()
        };
        assert_eq!(
            with_seen(after, "octocat", &later_ask)["octocat"].asked_at,
            Some(T + 1)
        );
    }

    #[test]
    fn rest_is_asked_every_rest_ms_per_login() {
        let rates: Rates = [(
            "octocat".to_owned(),
            LoginRates {
                asked_at: Some(T),
                ..LoginRates::default()
            },
        )]
        .into();
        assert!(!is_rest_due(&rates, "octocat", T + REST_MS - 1));
        assert!(is_rest_due(&rates, "octocat", T + REST_MS));
        assert!(is_rest_due(&rates, "hubot", T));
    }

    #[test]
    fn a_low_login_waits_alone_and_the_round_only_once_every_login_is_low() {
        let rates: Rates = [
            ("octocat".to_owned(), points(LOW_RATE - 1, T + 60_000)),
            ("hubot".to_owned(), points(LOW_RATE, T + 30_000)),
            ("monalisa".to_owned(), points(10, T + 30_000)),
        ]
        .into();
        assert_eq!(low_until(&rates, "octocat", T), Some(T + 60_000));
        assert_eq!(low_until(&rates, "hubot", T), None);
        // Past its reset, it is full again.
        assert_eq!(low_until(&rates, "octocat", T + 60_000), None);
        let names = |n: &[&str]| n.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        assert_eq!(
            round_waits_until(&rates, &names(&["octocat", "monalisa"]), T),
            Some(T + 30_000)
        );
        assert_eq!(
            round_waits_until(&rates, &names(&["octocat", "hubot"]), T),
            None
        );
        assert_eq!(round_waits_until(&rates, &[], T), None);
    }

    #[test]
    fn the_legacy_rate_is_the_login_with_the_most_points() {
        let rates: Rates = [
            ("hubot".to_owned(), points(4_000, 1_791_378_000_000)),
            ("octocat".to_owned(), points(200, T)),
        ]
        .into();
        assert_eq!(
            legacy_rate(&rates),
            Some(RateLimit {
                remaining: 4_000,
                reset_at: "2026-10-07T13:00:00.000Z".into(),
                limit: Some(5_000)
            })
        );
        assert_eq!(legacy_rate(&Rates::new()), None);
    }
}
