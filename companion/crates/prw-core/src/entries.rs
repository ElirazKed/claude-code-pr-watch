//! What `results.json` keeps of each PR, and what a failed fetch and a round do to it
//! (`hooks/cache.ts`).
//!
//! These work on an entry's raw JSON, not on [`Entry`](crate::protocol::Entry): a writer
//! changes only the fields it means to, so whatever else an entry holds, a newer writer's
//! field or GitHub's, is passed on as it was. Each reads its fields as the mod's spreads and
//! `??` do: a key that is there counts as there, whatever its value.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use crate::lenient::ms_of;

/// What a PR no logged-in gh account could see said before the card named the logins asked:
/// an older version's entry may still say it.
pub const NOT_FOUND: &str = "Not found, or no gh account can see it";

/// A PR no account can see is asked about again only this often (ms).
pub const UNREACHABLE_MS: i64 = 300_000;

/// How long fetches may fail before a card that has data says so (ms).
pub const QUIET_MS: i64 = 120_000;

/// How long a round keeps a PR no live session watches, after its last fetch (ms): a session
/// back from a restart within the hour still finds its PRs.
pub const KEEP_MS: i64 = 3_600_000;

fn field<'a>(entry: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    entry?.as_object()?.get(key)
}

/// When an entry was fetched (or last failed), where it says.
fn at_of(entry: Option<&Value>) -> Option<i64> {
    field(entry, "at").and_then(ms_of)
}

/// An entry for a PR fetched at `at`.
#[must_use]
pub fn fetched_entry(pr: Map<String, Value>, at: i64) -> Value {
    Value::Object(Map::from_iter([
        ("at".to_owned(), at.into()),
        ("pr".to_owned(), Value::Object(pr)),
    ]))
}

/// Whether a PR was never read because no account could see it (`isMiss`). An older
/// version's entry has no flag, only the words.
#[must_use]
pub fn is_miss(entry: Option<&Value>) -> bool {
    entry.is_some()
        && field(entry, "pr").is_none()
        && (field(entry, "isUnreachable") == Some(&Value::Bool(true))
            || field(entry, "error").and_then(Value::as_str) == Some(NOT_FOUND))
}

/// Whether a round leaves this PR out (`isUnreachable`): a miss [`UNREACHABLE_MS`] ago at
/// most.
#[must_use]
pub fn is_unreachable(entry: Option<&Value>, at: i64) -> bool {
    is_miss(entry) && at_of(entry).is_some_and(|was| at.saturating_sub(was) < UNREACHABLE_MS)
}

/// A PR's entry after a fetch at `at` failed with `error` (`failedEntry`). One never read
/// says so at once; one with data keeps it and its time, and shows the error once it has
/// failed for [`QUIET_MS`]. `is_miss`: no gh account could see it.
#[must_use]
pub fn failed_entry(entry: Option<&Value>, error: &str, at: i64, is_miss: bool) -> Value {
    let Some(kept) = entry
        .and_then(Value::as_object)
        .filter(|e| e.contains_key("pr"))
    else {
        return if is_miss {
            json!({"at": at, "error": error, "isUnreachable": true})
        } else {
            json!({"at": at, "error": error})
        };
    };
    let mut kept = kept.clone();
    let was = kept.remove("error");
    let failed_since = kept.remove("failedSince");
    kept.remove("isUnreachable");
    // An older version's entry has an error and no failedSince: its `at` is when it failed.
    let since = failed_since
        .as_ref()
        .and_then(ms_of)
        .or_else(|| was.and_then(|_| at_of(entry)))
        .unwrap_or(at);
    kept.insert("failedSince".into(), since.into());
    if at.saturating_sub(since) >= QUIET_MS {
        kept.insert("error".into(), error.into());
    }
    Value::Object(kept)
}

/// Whether an entry is of a finished PR: read, and no longer open. A round never asks it
/// again, so its entry is what says it merged.
#[must_use]
pub fn is_finished(entry: Option<&Value>) -> bool {
    field(entry, "pr").is_some_and(|pr| pr.get("state").and_then(Value::as_str) != Some("OPEN"))
}

/// What a round keeps (`keptEntries`): every PR a live session watches, however old, and the
/// rest until [`KEEP_MS`] old.
#[must_use]
pub fn kept_entries<S: std::hash::BuildHasher>(
    prs: Map<String, Value>,
    live: &HashSet<String, S>,
    at: i64,
) -> Map<String, Value> {
    prs.into_iter()
        .filter(|(url, entry)| {
            live.contains(url)
                || at_of(Some(entry)).is_some_and(|was| at.saturating_sub(was) < KEEP_MS)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_791_374_400_000;

    fn pr() -> Value {
        json!({"number": 7, "title": "Retry uploads", "state": "OPEN"})
    }

    fn with_pr(at: i64) -> Value {
        json!({"at": at, "pr": pr()})
    }

    #[test]
    fn a_pr_with_data_keeps_it_and_its_time_and_says_nothing_at_first() {
        assert_eq!(
            failed_entry(Some(&with_pr(T)), "gh: HTTP 502", T + 20_000, false),
            json!({"at": T, "pr": pr(), "failedSince": T + 20_000})
        );
    }

    #[test]
    fn the_error_shows_once_the_failures_have_lasted_quiet_ms_counted_from_the_first() {
        let entry = failed_entry(Some(&with_pr(T)), "gh: HTTP 502", T + 60_000, false);
        let entry = failed_entry(
            Some(&entry),
            "gh: HTTP 504",
            T + 60_000 + QUIET_MS - 1,
            false,
        );
        assert_eq!(entry.get("error"), None);
        let entry = failed_entry(Some(&entry), "gh: HTTP 504", T + 60_000 + QUIET_MS, false);
        assert_eq!(
            entry,
            json!({"at": T, "pr": pr(), "failedSince": T + 60_000, "error": "gh: HTTP 504"})
        );
    }

    #[test]
    fn a_pr_never_read_says_so_at_once() {
        assert_eq!(
            failed_entry(None, "gh: HTTP 502", T, false),
            json!({"at": T, "error": "gh: HTTP 502"})
        );
        let failed = json!({"at": T - 60_000, "error": "gh: HTTP 502"});
        assert_eq!(
            failed_entry(Some(&failed), NOT_FOUND, T, false),
            json!({"at": T, "error": NOT_FOUND})
        );
    }

    #[test]
    fn an_older_versions_failed_entry_counts_from_its_time() {
        let older = json!({"at": T, "pr": pr(), "error": "gh: HTTP 502"});
        assert_eq!(
            failed_entry(Some(&older), "gh: HTTP 502", T + 60_000, false),
            json!({"at": T, "pr": pr(), "failedSince": T})
        );
        assert_eq!(
            failed_entry(Some(&older), "gh: HTTP 502", T + QUIET_MS, false),
            json!({"at": T, "pr": pr(), "failedSince": T, "error": "gh: HTTP 502"})
        );
    }

    #[test]
    fn fields_the_entry_has_that_this_doesnt_know_pass_on() {
        let newer = json!({"at": T, "pr": pr(), "by": "prw", "isUnreachable": true});
        assert_eq!(
            failed_entry(Some(&newer), "gh: HTTP 502", T + 1, false),
            json!({"at": T, "pr": pr(), "by": "prw", "failedSince": T + 1})
        );
    }

    #[test]
    fn a_pr_no_account_can_see_is_flagged_and_left_out_for_unreachable_ms() {
        let entry = failed_entry(None, NOT_FOUND, T, true);
        assert_eq!(
            entry,
            json!({"at": T, "error": NOT_FOUND, "isUnreachable": true})
        );
        assert!(is_unreachable(Some(&entry), T + UNREACHABLE_MS - 1));
        assert!(!is_unreachable(Some(&entry), T + UNREACHABLE_MS));
        // A first fetch that failed on the way is no miss: the next round asks.
        let blip = failed_entry(None, "gh: HTTP 502", T, false);
        assert!(!is_unreachable(Some(&blip), T + 10_000));
        assert!(!is_unreachable(None, T));
    }

    #[test]
    fn a_miss_however_old_is_one_never_read_that_no_account_could_see() {
        let flagged = failed_entry(None, "Not found as octocat or hubot", T, true);
        assert!(is_miss(Some(&flagged)));
        assert!(is_miss(Some(
            &json!({"at": T - 10 * UNREACHABLE_MS, "error": NOT_FOUND})
        )));
        assert!(!is_miss(Some(&json!({"at": T, "error": "gh: HTTP 502"}))));
        assert!(!is_miss(Some(
            &json!({"at": T, "pr": pr(), "error": NOT_FOUND})
        )));
        assert!(!is_miss(None));
        // An older version's miss has the words and no flag; one with data is never left out.
        assert!(is_unreachable(
            Some(&json!({"at": T, "error": NOT_FOUND})),
            T + 10_000
        ));
        assert!(!is_unreachable(
            Some(&json!({"at": T, "pr": pr(), "error": NOT_FOUND})),
            T + 10_000
        ));
        // Read at last: the flag goes with the miss.
        assert_eq!(
            failed_entry(Some(&with_pr(T)), NOT_FOUND, T + 10_000, true),
            json!({"at": T, "pr": pr(), "failedSince": T + 10_000})
        );
    }

    #[test]
    fn a_read_pr_that_is_no_longer_open_is_finished() {
        assert!(!is_finished(Some(&with_pr(T))));
        assert!(is_finished(Some(
            &json!({"at": T, "pr": {"state": "MERGED"}})
        )));
        assert!(!is_finished(Some(&json!({"at": T, "error": NOT_FOUND}))));
        assert!(!is_finished(None));
    }

    #[test]
    fn every_live_pr_however_old_and_the_rest_until_keep_ms_old() {
        let at = T + 10 * KEEP_MS;
        let prs: Map<String, Value> = [
            ("live", with_pr(at - 3 * KEEP_MS)),
            ("recent", with_pr(at - KEEP_MS + 1)),
            ("dead", with_pr(at - KEEP_MS)),
            (
                "unreachable",
                json!({"at": at - 2 * KEEP_MS, "error": NOT_FOUND}),
            ),
            ("odd", json!(4)),
        ]
        .into_iter()
        .map(|(url, e)| (url.to_owned(), e))
        .collect();
        let live: HashSet<String> = ["live".into(), "elsewhere".into()].into();
        let kept: Vec<String> = kept_entries(prs, &live, at).keys().cloned().collect();
        assert_eq!(kept, ["live", "recent"]);
        assert!(kept_entries(Map::new(), &live, at).is_empty());
    }
}
