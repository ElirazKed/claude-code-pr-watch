//! What the full view changes on GitHub: a reply to a review thread (`r`), the thread
//! resolved or reopened (`x`), and an approval (`a`); and, from anywhere, the PR closed or
//! reopened (`X`, `gh pr close` or `gh pr reopen`: [`close_argv`]). The rest are each one
//! GraphQL mutation, sent as the request's JSON body on gh's stdin ([`MUTATE_ARGV`]): a
//! reply's text never goes in argv, so a text starting with `-` or `@`, or holding newlines,
//! posts as typed.

use serde_json::json;

/// `gh api graphql`, the request's body read from stdin.
pub const MUTATE_ARGV: [&str; 4] = ["api", "graphql", "--input", "-"];

const REPLY: &str = "mutation($thread: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $thread, body: $body}) { comment { url } }
}";
const RESOLVE: &str = "mutation($thread: ID!) {
  resolveReviewThread(input: {threadId: $thread}) { thread { isResolved } }
}";
const REOPEN: &str = "mutation($thread: ID!) {
  unresolveReviewThread(input: {threadId: $thread}) { thread { isResolved } }
}";
const APPROVE: &str = "mutation($pr: ID!) {
  addPullRequestReview(input: {pullRequestId: $pr, event: APPROVE}) { pullRequestReview { state } }
}";

/// A reply to the thread `thread` (its node id), the text whole.
#[must_use]
pub fn reply_input(thread: &str, text: &str) -> String {
    json!({ "query": REPLY, "variables": { "thread": thread, "body": text } }).to_string()
}

/// The thread resolved, or reopened where `resolve` is false.
#[must_use]
pub fn resolve_input(thread: &str, resolve: bool) -> String {
    let query = if resolve { RESOLVE } else { REOPEN };
    json!({ "query": query, "variables": { "thread": thread } }).to_string()
}

/// An approval with no text, of the PR `pr` (its node id).
#[must_use]
pub fn approve_input(pr: &str) -> String {
    json!({ "query": APPROVE, "variables": { "pr": pr } }).to_string()
}

/// `gh pr close` (or `gh pr reopen`, where `close` is false) of the PR at `url`.
#[must_use]
pub fn close_argv(url: &str, close: bool) -> Vec<String> {
    let verb = if close { "close" } else { "reopen" };
    ["pr", verb, url].map(str::to_owned).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::Value;

    fn parsed(input: &str) -> Value {
        serde_json::from_str(input).expect("JSON")
    }

    #[test]
    fn x_resolves_or_reopens_the_thread_it_names() {
        let mutation = |resolve| {
            let input = parsed(&resolve_input("PRRT_1", resolve));
            assert_eq!(input["variables"], json!({ "thread": "PRRT_1" }));
            input["query"]
                .as_str()
                .and_then(|q| q.split_whitespace().nth(3))
                .map(str::to_owned)
        };
        assert_eq!(
            mutation(true).as_deref(),
            Some("resolveReviewThread(input:")
        );
        assert_eq!(
            mutation(false).as_deref(),
            Some("unresolveReviewThread(input:")
        );
    }

    #[test]
    fn a_approves_the_pr_it_names_with_no_text() {
        let input = parsed(&approve_input("PR_kw"));
        assert!(
            input["query"]
                .as_str()
                .is_some_and(|q| q.contains("event: APPROVE"))
        );
        assert_eq!(input["variables"], json!({ "pr": "PR_kw" }));
    }

    #[test]
    fn x_closes_or_reopens_the_pr_it_names() {
        let url = "https://github.com/acme/app/pull/7";
        assert_eq!(close_argv(url, true), ["pr", "close", url]);
        assert_eq!(close_argv(url, false), ["pr", "reopen", url]);
    }

    proptest! {
        #[test]
        fn any_reply_posts_whole_and_only_as_a_variable(text in "\\PC{0,60}", thread in "[A-Za-z0-9_-]{1,20}") {
            let input = parsed(&reply_input(&thread, &text));
            prop_assert_eq!(&input["variables"], &json!({ "thread": thread, "body": text }));
            prop_assert_eq!(input["query"].as_str(), Some(REPLY));
        }
    }
}
