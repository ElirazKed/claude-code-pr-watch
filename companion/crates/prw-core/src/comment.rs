//! Commenting on a PR from its card: the gh call that posts it, what GitHub answers, and how a
//! refusal reads. A port of `hooks/comment.ts`.

use serde_json::{Value, json};

use crate::batch::first_line;

/// Only text with something in it posts: an empty or blank field does nothing (`isPostable`).
#[must_use]
pub fn is_postable(text: &str) -> bool {
    !text.trim().is_empty()
}

/// A comment on the PR's conversation (an issue comment, not a review), `commentArgv`. The
/// text never goes in argv: it goes on stdin as the request's JSON body ([`comment_input`]),
/// so a text starting with `@` or `-`, or holding newlines, posts as typed and is never read
/// as a flag or a file.
#[must_use]
pub fn comment_argv(repo: &str, number: u64) -> Vec<String> {
    [
        "api",
        "--method",
        "POST",
        &format!("repos/{repo}/issues/{number}/comments"),
        "--input",
        "-",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// What gh reads on stdin: the text whole, nothing trimmed (`commentInput`).
#[must_use]
pub fn comment_input(text: &str) -> String {
    json!({ "body": text }).to_string()
}

/// The new comment's link, from GitHub's reply; `None` where it named none (`commentUrl`).
#[must_use]
pub fn comment_url(stdout: &str) -> Option<String> {
    let reply: Value = serde_json::from_str(stdout).ok()?;
    let url = reply.get("html_url")?.as_str()?;
    url.starts_with("https://").then(|| url.to_owned())
}

/// gh's first line, in plain words (`commentError`): "gh: Unable to create comment because
/// issue is locked. (HTTP 403)" is "Couldn't comment: Unable to create comment because issue
/// is locked."
#[must_use]
pub fn comment_error(stderr: &str, login: &str) -> String {
    refused(stderr, login, "comment")
}

/// What gh said when GitHub refused a write, in plain words, after "Couldn't `verb`:". A 404
/// and a 401 say what they mean for `login`.
#[must_use]
pub fn refused(stderr: &str, login: &str, verb: &str) -> String {
    let line = first_line(stderr);
    let text = line.strip_prefix("gh:").map_or(line, str::trim_start);
    let who = if login.is_empty() {
        "this account"
    } else {
        login
    };
    if text.contains("HTTP 404") {
        return format!("Couldn't {verb}: GitHub has no such PR, or {who} can't see it");
    }
    if text.contains("HTTP 401") {
        return format!("Couldn't {verb}: GitHub refused {who}'s token; gh auth login renews it");
    }
    let said = strip_status(text);
    format!(
        "Couldn't {verb}: {}",
        if said.is_empty() {
            "gh api failed"
        } else {
            said
        }
    )
}

/// The text without a closing " (HTTP 403)".
fn strip_status(text: &str) -> &str {
    let Some(open) = text.rfind("(HTTP ") else {
        return text;
    };
    let tail = &text[open + 6..];
    let is_status =
        tail.len() == 4 && tail.ends_with(')') && tail[..3].bytes().all(|b| b.is_ascii_digit());
    if is_status {
        text[..open].trim_end()
    } else {
        text
    }
}

/// What the empty field says: which PR, and which account will post (`commentPlaceholder`).
#[must_use]
pub fn comment_placeholder(number: u64, login: &str) -> String {
    if login.is_empty() {
        format!("Comment on #{number}")
    } else {
        format!("Comment on #{number} as {login}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn only_text_with_something_in_it_posts() {
        assert!(is_postable("LGTM"));
        assert!(is_postable("  /retest\n"));
        assert!(!is_postable(""));
        assert!(!is_postable(" \n\t "));
    }

    #[test]
    fn the_call_names_the_pr_and_reads_the_text_from_stdin() {
        let argv = comment_argv("acme/app", 21);
        assert_eq!(
            argv,
            [
                "api",
                "--method",
                "POST",
                "repos/acme/app/issues/21/comments",
                "--input",
                "-"
            ]
        );
    }

    #[test]
    fn the_new_comments_link_or_none() {
        let reply =
            r#"{"id": 1, "html_url": "https://github.com/acme/app/pull/21#issuecomment-1"}"#;
        assert_eq!(
            comment_url(reply).as_deref(),
            Some("https://github.com/acme/app/pull/21#issuecomment-1")
        );
        for stdout in [
            "{}",
            "null",
            r#"{"html_url": 7}"#,
            "<html>Bad Gateway</html>",
            "",
        ] {
            assert_eq!(comment_url(stdout), None, "{stdout}");
        }
    }

    #[test]
    fn ghs_refusal_in_plain_words() {
        assert_eq!(
            comment_error(
                "gh: Unable to create comment because issue is locked. (HTTP 403)\n",
                "me"
            ),
            "Couldn't comment: Unable to create comment because issue is locked."
        );
        assert_eq!(
            comment_error(
                "gh: Validation Failed (HTTP 422)\n{\"message\":\"Validation Failed\"}\n",
                "me"
            ),
            "Couldn't comment: Validation Failed"
        );
        assert_eq!(
            comment_error("gh: Not Found (HTTP 404)\n", "hubot"),
            "Couldn't comment: GitHub has no such PR, or hubot can't see it"
        );
        assert_eq!(
            comment_error("gh: Bad credentials (HTTP 401)\n", "me"),
            "Couldn't comment: GitHub refused me's token; gh auth login renews it"
        );
        assert_eq!(
            comment_error("error connecting to api.github.com\n", "me"),
            "Couldn't comment: error connecting to api.github.com"
        );
        assert_eq!(comment_error("", "me"), "Couldn't comment: gh api failed");
    }

    #[test]
    fn any_writes_refusal_reads_as_githubs_words() {
        assert_eq!(
            refused(
                "gh: Can not approve your own pull request\n",
                "ada",
                "approve"
            ),
            "Couldn't approve: Can not approve your own pull request"
        );
    }

    #[test]
    fn the_placeholder_names_the_pr_and_the_account() {
        assert_eq!(comment_placeholder(21, "hubot"), "Comment on #21 as hubot");
        assert_eq!(comment_placeholder(21, ""), "Comment on #21");
    }

    proptest! {
        #[test]
        fn any_text_posts_whole(text in "\\PC{0,60}") {
            let input: Value = serde_json::from_str(&comment_input(&text)).expect("JSON");
            prop_assert_eq!(input, json!({ "body": text }));
        }
    }
}
