//! Finding a login's own open PRs: one GitHub search per gh login, run as that login, for the
//! PRs `prw` announces in its session file. The search finds URLs only; the poll (the mod's,
//! or `prw`'s) fetches them like any other session's.

use serde_json::Value;

use crate::batch::{first_line, or, ref_from_url};
use crate::lenient::read;
use crate::protocol::RateLimit;

/// The most PRs one search returns (GitHub's page cap).
pub const SEARCH_FIRST: usize = 100;

/// How often `prw` searches again (ms): a PR opened elsewhere shows within this. One point a
/// search, so a minute is 60 points an hour per login.
pub const SEARCH_MS: i64 = 60_000;

/// The search for `login`'s open PRs, in repos that aren't archived. GitHub logins are
/// letters, digits and dashes; the query text is written as a JSON string all the same.
#[must_use]
pub fn mine_query(login: &str) -> String {
    let search = Value::String(format!("is:pr is:open archived:false author:{login}"));
    format!(
        "query {{\n  rateLimit {{ limit remaining resetAt }}\n  search(query: {search}, type: ISSUE, first: {SEARCH_FIRST}) {{ issueCount nodes {{ ... on PullRequest {{ url }} }} }}\n}}"
    )
}

/// What a search found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hits {
    /// The PRs' URLs, as GitHub ordered them (only github.com PR URLs).
    pub urls: Vec<String>,
    /// How many PRs match in all: more than `urls` holds past [`SEARCH_FIRST`].
    pub total: u64,
    pub rate: Option<RateLimit>,
}

/// `gh api graphql`'s reply to [`mine_query`].
///
/// # Errors
///
/// What the card says where the call failed as a whole: gh's stderr, or GitHub's message.
pub fn parse_hits(stdout: &str, stderr: &str) -> Result<Hits, String> {
    let reply: Value = serde_json::from_str(stdout)
        .map_err(|_| or(first_line(stderr), "gh api graphql failed"))?;
    let Some(search) = reply.pointer("/data/search").filter(|s| s.is_object()) else {
        let said = reply.pointer("/errors/0/message").and_then(Value::as_str);
        return Err(said.map_or_else(
            || or(first_line(stderr), "GitHub returned no data"),
            str::to_owned,
        ));
    };
    let urls = search
        .get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|node| node.get("url")?.as_str())
        .filter(|url| ref_from_url(url).is_some())
        .map(str::to_owned)
        .collect();
    Ok(Hits {
        urls,
        total: search
            .get("issueCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        rate: reply.pointer("/data/rateLimit").cloned().and_then(read),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_search_is_the_logins_open_prs_and_the_rate_limit() {
        let query = mine_query("octocat");
        assert!(query.contains(
            r#"search(query: "is:pr is:open archived:false author:octocat", type: ISSUE, first: 100)"#
        ));
        assert!(query.contains("rateLimit { limit remaining resetAt }"));
        assert!(query.contains("... on PullRequest { url }"));
        // Whatever the text, it stays inside its string.
        assert!(mine_query(r#"a" b"#).contains(r#"author:a\" b""#));
    }

    #[test]
    fn hits_are_pr_urls_in_githubs_order() {
        let stdout = json!({"data": {
            "rateLimit": {"limit": 5000, "remaining": 4990, "resetAt": "2026-10-07T13:00:00Z"},
            "search": {"issueCount": 112, "nodes": [
                {"url": "https://github.com/acme/web/pull/3"},
                {},
                {"url": "https://github.com/acme/app/issues/9"},
                {"url": "https://github.com/acme/app/pull/1"}
            ]}
        }});
        let hits = parse_hits(&stdout.to_string(), "").unwrap();
        assert_eq!(
            hits.urls,
            [
                "https://github.com/acme/web/pull/3",
                "https://github.com/acme/app/pull/1"
            ]
        );
        assert_eq!(hits.total, 112);
        assert_eq!(hits.rate.map(|r| r.remaining), Some(4990));
    }

    #[test]
    fn a_failed_search_says_why() {
        assert_eq!(
            parse_hits("", "gh: Bad credentials (HTTP 401)\n"),
            Err("gh: Bad credentials (HTTP 401)".into())
        );
        assert_eq!(parse_hits("", ""), Err("gh api graphql failed".into()));
        let refused = json!({"data": null, "errors": [{"message": "Something went wrong"}]});
        assert_eq!(
            parse_hits(&refused.to_string(), ""),
            Err("Something went wrong".into())
        );
        assert_eq!(parse_hits("{}", ""), Err("GitHub returned no data".into()));
    }
}
