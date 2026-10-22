//! GitHub's search, for the PRs `prw` announces: each gh login's own open PRs ("My PRs"), and
//! the saved searches of `config.toml` ([`crate::config`]), all of one login's in one call, as
//! that login. A search finds URLs only; the poll (the mod's, or `prw`'s) fetches them like
//! any other session's.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;

use serde_json::Value;

use crate::batch::{first_line, or, ref_from_url};
use crate::lenient::read;
use crate::protocol::RateLimit;

/// The most PRs one search returns (GitHub's page cap).
pub const SEARCH_FIRST: usize = 100;

/// How often `prw` searches again (ms): a PR opened elsewhere shows within this. One point a
/// search, so a minute is 60 points an hour per login and search.
pub const SEARCH_MS: i64 = 60_000;

/// The alias "My PRs" goes by in a call; saved searches go by `s0`, `s1`, … ([`saved_alias`]).
pub const MINE: &str = "mine";

/// The alias of the `i`th saved search in a call.
#[must_use]
pub fn saved_alias(i: usize) -> String {
    format!("s{i}")
}

/// One search in a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// Its name in the reply: [`MINE`] or a [`saved_alias`].
    pub alias: String,
    /// What GitHub is asked, as typed in its search box.
    pub text: String,
    /// The most PRs to list.
    pub first: usize,
}

/// `login`'s open PRs, in repos that aren't archived.
#[must_use]
pub fn mine(login: &str) -> Ask {
    Ask {
        alias: MINE.to_owned(),
        text: format!("is:pr is:open archived:false author:{login}"),
        first: SEARCH_FIRST,
    }
}

/// The searches of one call, with the rate limit. Each text is written as a JSON string, so
/// no query can close its own.
#[must_use]
pub fn searches_query(asks: &[Ask]) -> String {
    let searches = asks.iter().fold(String::new(), |mut out, ask| {
        let _ = writeln!(
            out,
            "  {}: search(query: {}, type: ISSUE, first: {}) {{ issueCount nodes {{ ... on PullRequest {{ url }} }} }}",
            ask.alias,
            Value::String(ask.text.clone()),
            ask.first.min(SEARCH_FIRST)
        );
        out
    });
    format!("query {{\n  rateLimit {{ limit remaining resetAt }}\n{searches}}}")
}

/// What a search found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hits {
    /// The PRs' URLs, as GitHub ordered them (only github.com PR URLs).
    pub urls: Vec<String>,
    /// How many PRs match in all: more than `urls` holds past its `first`.
    pub total: u64,
}

/// What one call found: each alias's hits, or why that search failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    pub hits: BTreeMap<String, Result<Hits, String>>,
    pub rate: Option<RateLimit>,
}

/// `gh api graphql`'s reply to [`searches_query`] for these `aliases`.
///
/// # Errors
///
/// What the lists say where the call failed as a whole: gh's stderr, or GitHub's message.
/// Where only some searches failed, each says so in [`Found::hits`].
pub fn parse_searches(stdout: &str, stderr: &str, aliases: &[&str]) -> Result<Found, String> {
    let reply: Value = serde_json::from_str(stdout)
        .map_err(|_| or(first_line(stderr), "gh api graphql failed"))?;
    let said = |alias: Option<&str>| {
        reply
            .get("errors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|e| alias.is_none_or(|a| e.pointer("/path/0").and_then(Value::as_str) == Some(a)))
            .and_then(|e| e.get("message")?.as_str())
            .map(str::to_owned)
    };
    let Some(data) = reply.get("data").filter(|d| d.is_object()) else {
        return Err(said(None).unwrap_or_else(|| or(first_line(stderr), "GitHub returned no data")));
    };
    let hits = aliases
        .iter()
        .map(|&alias| {
            let hits = data
                .get(alias)
                .filter(|s| s.is_object())
                .map(hits_of)
                .ok_or_else(|| {
                    said(Some(alias)).unwrap_or_else(|| "GitHub returned no data".to_owned())
                });
            (alias.to_owned(), hits)
        })
        .collect();
    Ok(Found {
        hits,
        rate: data.get("rateLimit").cloned().and_then(read),
    })
}

fn hits_of(search: &Value) -> Hits {
    Hits {
        urls: search
            .get("nodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|node| node.get("url")?.as_str())
            .filter(|url| ref_from_url(url).is_some())
            .map(str::to_owned)
            .collect(),
        total: search
            .get("issueCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

/// One search's finds as several logins made them, as one list: each PR once, in the
/// order found (login by login), at most `cap`. The total is the most any login counted,
/// or the PRs found where those are more (logins that each see some of them).
#[must_use]
pub fn gather<'a>(finds: impl IntoIterator<Item = &'a Hits>, cap: usize) -> Hits {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    let mut total = 0;
    for hits in finds {
        total = total.max(hits.total);
        urls.extend(
            hits.urls
                .iter()
                .filter(|url| seen.insert(url.as_str()))
                .cloned(),
        );
    }
    let total = total.max(urls.len() as u64);
    urls.truncate(cap);
    Hits { urls, total }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn saved(i: usize, text: &str) -> Ask {
        Ask {
            alias: saved_alias(i),
            text: text.to_owned(),
            first: 30,
        }
    }

    #[test]
    fn one_call_asks_every_search_and_the_rate_limit() {
        let query = searches_query(&[
            mine("octocat"),
            saved(0, "is:pr is:open review-requested:@me"),
        ]);
        assert!(query.contains(
            r#"mine: search(query: "is:pr is:open archived:false author:octocat", type: ISSUE, first: 100)"#
        ));
        assert!(query.contains(
            r#"s0: search(query: "is:pr is:open review-requested:@me", type: ISSUE, first: 30)"#
        ));
        assert!(query.contains("rateLimit { limit remaining resetAt }"));
        assert!(query.contains("... on PullRequest { url }"));
        // Whatever the text, it stays inside its string.
        assert!(searches_query(&[mine(r#"a" b"#)]).contains(r#"author:a\" b""#));
    }

    #[test]
    fn hits_are_pr_urls_in_githubs_order() {
        let stdout = json!({"data": {
            "rateLimit": {"limit": 5000, "remaining": 4990, "resetAt": "2026-10-07T13:00:00Z"},
            "mine": {"issueCount": 112, "nodes": [
                {"url": "https://github.com/acme/web/pull/3"},
                {},
                {"url": "https://github.com/acme/app/issues/9"},
                {"url": "https://github.com/acme/app/pull/1"}
            ]}
        }});
        let found = parse_searches(&stdout.to_string(), "", &[MINE]).unwrap();
        let hits = found.hits[MINE].clone().unwrap();
        assert_eq!(
            hits.urls,
            [
                "https://github.com/acme/web/pull/3",
                "https://github.com/acme/app/pull/1"
            ]
        );
        assert_eq!(hits.total, 112);
        assert_eq!(found.rate.map(|r| r.remaining), Some(4990));
    }

    #[test]
    fn one_search_failing_leaves_the_others() {
        let stdout = json!({
            "data": {"mine": {"issueCount": 0, "nodes": []}, "s0": null},
            "errors": [{"path": ["s0"], "message": "Invalid search query"}]
        });
        let found = parse_searches(&stdout.to_string(), "", &[MINE, "s0", "s1"]).unwrap();
        assert_eq!(found.hits[MINE], Ok(Hits::default()));
        assert_eq!(found.hits["s0"], Err("Invalid search query".into()));
        assert_eq!(found.hits["s1"], Err("GitHub returned no data".into()));
    }

    #[test]
    fn a_failed_call_says_why() {
        assert_eq!(
            parse_searches("", "gh: Bad credentials (HTTP 401)\n", &[MINE]),
            Err("gh: Bad credentials (HTTP 401)".into())
        );
        assert_eq!(
            parse_searches("", "", &[MINE]),
            Err("gh api graphql failed".into())
        );
        let refused = json!({"data": null, "errors": [{"message": "Something went wrong"}]});
        assert_eq!(
            parse_searches(&refused.to_string(), "", &[MINE]),
            Err("Something went wrong".into())
        );
        assert_eq!(
            parse_searches("{}", "", &[MINE]),
            Err("GitHub returned no data".into())
        );
    }

    #[test]
    fn logins_finds_go_together_each_once() {
        let a = Hits {
            urls: vec!["u1".into(), "u2".into()],
            total: 2,
        };
        let b = Hits {
            urls: vec!["u2".into(), "u3".into()],
            total: 40,
        };
        assert_eq!(
            gather([&a, &b], 30),
            Hits {
                urls: vec!["u1".into(), "u2".into(), "u3".into()],
                total: 40
            }
        );
        assert_eq!(gather([&a, &b], 2).urls, ["u1", "u2"]);
        assert_eq!(gather([], 30), Hits::default());
    }

    proptest! {
        #[test]
        fn parse_searches_is_total(stdout in any::<String>(), stderr in any::<String>()) {
            let _ = parse_searches(&stdout, &stderr, &[MINE, "s0"]);
        }

        #[test]
        fn gather_lists_each_found_pr_once_up_to_the_cap(
            finds in prop::collection::vec(
                (prop::collection::vec("u[0-9]", 0..8), 0_u64..50),
                0..4,
            ),
            cap in 0_usize..10,
        ) {
            let finds: Vec<Hits> = finds
                .into_iter()
                .map(|(urls, total)| Hits { urls, total })
                .collect();
            let out = gather(&finds, cap);
            prop_assert!(out.urls.len() <= cap);
            let unique: HashSet<&String> = out.urls.iter().collect();
            prop_assert_eq!(unique.len(), out.urls.len());
            prop_assert!(out.urls.iter().all(|u| finds.iter().any(|f| f.urls.contains(u))));
            prop_assert!(out.total >= out.urls.len() as u64);
            prop_assert!(finds.iter().all(|f| out.total >= f.total));
            // Nothing found is left out but past the cap.
            let every: HashSet<&String> = finds.iter().flat_map(|f| &f.urls).collect();
            prop_assert_eq!(out.urls.len(), every.len().min(cap));
        }
    }
}
