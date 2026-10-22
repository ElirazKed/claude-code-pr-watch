//! Handing a PR's open review threads to an agent: where each one is, who said what, and the
//! message that asks to address them. A port of `hooks/address.ts`; the binary only fetches.

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value;

use crate::batch::{PrRef, first_line, or};
use crate::handoff::{PrFacts, chars, clip};
use crate::lenient::{lenient, lenient_vec, read};

/// The newest threads read on a press.
pub const THREADS: usize = 100;
/// The comments read of each thread (the first and its replies).
pub const THREAD_COMMENTS: usize = 10;
/// What one comment may take.
pub const COMMENT_CHARS: usize = 1_000;
/// What one thread may take.
pub const THREAD_CHARS: usize = 3_000;
/// What every thread together may take: enough to act on, not a transcript of the review.
pub const REVIEW_CHARS: usize = 12_000;

/// A comment's author.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Login {
    #[serde(deserialize_with = "lenient")]
    pub login: String,
}

/// A review comment (`GhComment`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GhComment {
    /// `None` for a deleted account.
    #[serde(deserialize_with = "lenient")]
    pub author: Option<Login>,
    #[serde(deserialize_with = "lenient")]
    pub body: String,
    #[serde(deserialize_with = "lenient")]
    pub url: String,
}

/// A thread's comments, and how many it has in all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Comments {
    #[serde(deserialize_with = "lenient")]
    pub total_count: Option<usize>,
    #[serde(deserialize_with = "lenient_vec")]
    pub nodes: Vec<GhComment>,
}

/// A review thread as GitHub has it (`GhThread`). `line` and `start_line` are where it sits
/// on the PR's code now (`None` once outdated, mostly); the `original` pair, where it was left.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GhThread {
    #[serde(deserialize_with = "lenient")]
    pub is_resolved: bool,
    #[serde(deserialize_with = "lenient")]
    pub is_outdated: bool,
    #[serde(deserialize_with = "lenient")]
    pub path: String,
    #[serde(deserialize_with = "lenient")]
    pub line: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub start_line: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub original_line: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub original_start_line: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub comments: Comments,
}

/// One PR's review threads with what they say (`threadsQuery`). Asked only on a press, never
/// by the poll, whose query only counts them; the alias shape is the batch's.
#[must_use]
pub fn threads_query(pr: &PrRef) -> String {
    let mut parts = pr.repo.split('/');
    let owner = Value::String(parts.next().unwrap_or_default().to_owned());
    let name = Value::String(parts.next().unwrap_or_default().to_owned());
    format!(
        "query {{
  p0: repository(owner: {owner}, name: {name}) {{ pullRequest(number: {}) {{
    reviewThreads(last: {THREADS}) {{ totalCount nodes {{
      isResolved isOutdated path line startLine originalLine originalStartLine
      comments(first: {THREAD_COMMENTS}) {{ totalCount nodes {{ author {{ login }} body url }} }}
    }} }}
  }} }}
}}",
        pr.number
    )
}

/// The threads gh printed, and how many the PR has in all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Threads {
    pub threads: Vec<GhThread>,
    pub total: usize,
}

/// `gh api graphql`'s reply to [`threads_query`] (`parseThreads`).
///
/// # Errors
///
/// Why there are none: gh's stderr, GitHub's message, or a PR this account can't see.
pub fn parse_threads(stdout: &str, stderr: &str) -> Result<Threads, String> {
    let reply: Value = serde_json::from_str(stdout)
        .map_err(|_| or(first_line(stderr), "gh api graphql failed"))?;
    let Some(data) = reply.get("data").filter(|d| !d.is_null()) else {
        let said = reply.pointer("/errors/0/message").and_then(Value::as_str);
        return Err(said.map_or_else(
            || or(first_line(stderr), "GitHub returned no data"),
            str::to_owned,
        ));
    };
    let Some(threads) = data
        .pointer("/p0/pullRequest/reviewThreads")
        .filter(|t| t.is_object())
    else {
        return Err("Not found, or this gh account can't see it".to_owned());
    };
    let threads: Vec<GhThread> = threads
        .get("nodes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|node| read(node.clone()).unwrap_or_default())
        .collect();
    let total = data
        .pointer("/p0/pullRequest/reviewThreads/totalCount")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(threads.len());
    Ok(Threads { threads, total })
}

fn line_of(thread: &GhThread) -> u64 {
    thread.line.or(thread.original_line).unwrap_or(0)
}

/// The threads still to address, by file and line: a resolved one is done, an outdated one
/// may not be (`openThreads`).
#[must_use]
pub fn open_threads(threads: &[GhThread]) -> Vec<&GhThread> {
    let mut open: Vec<&GhThread> = threads.iter().filter(|t| !t.is_resolved).collect();
    open.sort_by(|a, b| a.path.cmp(&b.path).then(line_of(a).cmp(&line_of(b))));
    open
}

fn span(start: Option<u64>, end: Option<u64>) -> String {
    match (start, end) {
        (_, None) => String::new(),
        (Some(start), Some(end)) if start != end => format!("{start}-{end}"),
        (_, Some(end)) => end.to_string(),
    }
}

/// Where a thread is (`spot`): "src/upload.ts:42", "src/upload.ts:40-42", "src/upload.ts
/// (outdated; was line 42)", or "src/upload.ts (the whole file)" for a comment on the file.
#[must_use]
pub fn spot(thread: &GhThread) -> String {
    let path = &thread.path;
    let now = span(thread.start_line, thread.line);
    if !thread.is_outdated {
        return if now.is_empty() {
            format!("{path} (the whole file)")
        } else {
            format!("{path}:{now}")
        };
    }
    if !now.is_empty() {
        return format!("{path}:{now} (outdated)");
    }
    let was = span(thread.original_start_line, thread.original_line);
    if was.is_empty() {
        format!("{path} (outdated)")
    } else {
        let s = if was.contains('-') { "s" } else { "" };
        format!("{path} (outdated; was line{s} {was})")
    }
}

static HTML_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").expect("a valid pattern"));
static BLANKS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n[ \t]*\n(?:[ \t]*\n)+").expect("a valid pattern"));
static LATER_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\n([^\n])").expect("a valid pattern"));

/// A comment as the agent needs it: without the HTML comments review apps keep their
/// bookkeeping in, or runs of blank lines, and clipped (`cleanBody`).
#[must_use]
pub fn clean_body(body: &str) -> String {
    let text = body.replace('\r', "");
    let text = HTML_COMMENT.replace_all(&text, "");
    let text = BLANKS.replace_all(&text, "\n\n");
    let text = clip(text.trim(), COMMENT_CHARS);
    if text.is_empty() {
        "(no text)".to_owned()
    } else {
        text
    }
}

/// The prompt's first words (`addressHeading`).
#[must_use]
pub fn address_heading(pr: &PrFacts) -> String {
    format!("Review comments to address on {}#{}:", pr.repo, pr.number)
}

/// One thread, numbered: where it is and its link, then each comment under it by its author,
/// a body's later lines (not its blank ones) indented further so a reply can't pass for one.
fn thread_block(thread: &GhThread, n: usize, author: &str) -> String {
    let comments = &thread.comments.nodes;
    let link = comments.first().map_or("no link", |c| c.url.as_str());
    let mut lines = vec![format!("{n}. {} · {link}", spot(thread))];
    lines.extend(comments.iter().map(|c| {
        let login = c.author.as_ref().map_or("ghost", |a| a.login.as_str());
        let who = if login.eq_ignore_ascii_case(author) {
            format!("@{login} (PR author)")
        } else {
            format!("@{login}")
        };
        let body = clean_body(&c.body);
        format!("   {who}: {}", LATER_LINE.replace_all(&body, "\n     $1"))
    }));
    let unread = thread
        .comments
        .total_count
        .unwrap_or(comments.len())
        .saturating_sub(comments.len());
    if unread > 0 {
        let replies = if unread == 1 { "reply" } else { "replies" };
        lines.push(format!("   ({unread} more {replies}, not read)"));
    }
    clip(&lines.join("\n"), THREAD_CHARS)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The message for the agent: the PR, each open thread (outdated ones marked), and the ask
/// (`addressPrompt`). `total` is how many threads the PR has, of which `threads` were read.
/// It never asks for a push or a merge, nor for a word on GitHub: replies and resolving stay
/// the person's call.
#[must_use]
pub fn address_prompt(pr: &PrFacts, threads: &[GhThread], total: usize) -> String {
    let open = open_threads(threads);
    let branch = pr.branch();
    let outdated = open.iter().filter(|t| t.is_outdated).count();
    let mut blocks: Vec<String> = Vec::new();
    let mut size = 0;
    for thread in &open {
        let block = thread_block(thread, blocks.len() + 1, &pr.author);
        if !blocks.is_empty() && size + chars(&block) > REVIEW_CHARS {
            break;
        }
        size += chars(&block) + 2;
        blocks.push(block);
    }
    let count = format!(
        "{} unresolved review thread{}",
        open.len(),
        plural(open.len())
    );
    let stale = if outdated == 0 {
        String::new()
    } else {
        let which = if outdated == 1 { "it" } else { "each" };
        format!(" ({outdated} outdated: the code changed since, so check {which} still applies)")
    };
    let mut out = vec![
        format!("{} {}", address_heading(pr), pr.title()),
        format!(
            "{} · {branch} → {} · files: {}/files",
            pr.url,
            pr.base(),
            pr.url
        ),
        String::new(),
        format!("{count}, by file{stale}:"),
        String::new(),
        blocks.join("\n\n"),
    ];
    let unshown = open.len() - blocks.len();
    if unshown > 0 {
        out.extend([
            String::new(),
            format!(
                "{unshown} more unresolved thread{} didn't fit here; they're on the PR's files tab.",
                plural(unshown)
            ),
        ]);
    }
    if total > threads.len() {
        out.extend([
            String::new(),
            format!(
                "Only the newest {} of {total} threads were read; older ones may be open too.",
                threads.len()
            ),
        ]);
    }
    out.extend([
        String::new(),
        format!(
            "Address each on this branch ({branch}): change the code where a comment is right, and where you'd disagree or a \
             comment needs my call, say so instead. If {} or {branch} isn't checked out here, say so first.",
            pr.repo
        ),
        "Don't reply to or resolve any thread on GitHub: if one needs a reply, draft it here for me to read first."
            .to_owned(),
    ]);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;
    use std::cell::Cell;

    fn facts() -> PrFacts {
        PrFacts {
            repo: "acme/widgets".to_owned(),
            number: 12,
            title: "Retry uploads with exponential backoff".to_owned(),
            url: "https://github.com/acme/widgets/pull/12".to_owned(),
            branch: "feat/retry".to_owned(),
            base: "main".to_owned(),
            author: "octocat".to_owned(),
        }
    }

    thread_local! {
        static IDS: Cell<u32> = const { Cell::new(100) };
    }

    /// A thread on `path` at `line`, its comments given as (login, body): open, current,
    /// single-line.
    fn thread(path: &str, line: Option<u64>, comments: &[(&str, &str)]) -> GhThread {
        GhThread {
            is_resolved: false,
            is_outdated: false,
            path: path.to_owned(),
            line,
            start_line: None,
            original_line: line,
            original_start_line: None,
            comments: Comments {
                total_count: Some(comments.len()),
                nodes: comments
                    .iter()
                    .map(|(login, body)| {
                        let id = IDS.with(|ids| {
                            ids.set(ids.get() + 1);
                            ids.get()
                        });
                        GhComment {
                            author: Some(Login {
                                login: (*login).to_owned(),
                            }),
                            body: (*body).to_owned(),
                            url: format!("{}#discussion_r{id}", facts().url),
                        }
                    })
                    .collect(),
            },
        }
    }

    #[test]
    fn resolved_threads_are_done_and_the_rest_go_by_file_and_line() {
        let threads = [
            thread("src/upload.ts", Some(40), &[("alice", "b")]),
            thread("src/api.ts", Some(9), &[("bob", "a")]),
            GhThread {
                is_resolved: true,
                ..thread("src/upload.ts", Some(7), &[("alice", "done")])
            },
            thread("src/upload.ts", Some(3), &[("carol", "c")]),
        ];
        let at: Vec<String> = open_threads(&threads)
            .iter()
            .map(|t| format!("{}:{}", t.path, t.line.unwrap_or(0)))
            .collect();
        assert_eq!(at, ["src/api.ts:9", "src/upload.ts:3", "src/upload.ts:40"]);
    }

    #[test]
    fn a_line_a_range_and_a_comment_on_the_whole_file() {
        let at = |start, line, original| GhThread {
            start_line: start,
            line,
            original_line: original,
            ..thread("src/upload.ts", None, &[])
        };
        assert_eq!(spot(&at(None, Some(42), None)), "src/upload.ts:42");
        assert_eq!(spot(&at(Some(40), Some(42), None)), "src/upload.ts:40-42");
        assert_eq!(spot(&at(Some(42), Some(42), None)), "src/upload.ts:42");
        assert_eq!(
            spot(&at(None, None, None)),
            "src/upload.ts (the whole file)"
        );
    }

    #[test]
    fn outdated_where_it_was_left_or_where_github_still_places_it() {
        let old = |line, start, original| GhThread {
            is_outdated: true,
            line,
            original_start_line: start,
            original_line: original,
            ..thread("src/upload.ts", None, &[])
        };
        assert_eq!(
            spot(&old(None, None, Some(7))),
            "src/upload.ts (outdated; was line 7)"
        );
        assert_eq!(
            spot(&old(None, Some(5), Some(7))),
            "src/upload.ts (outdated; was lines 5-7)"
        );
        assert_eq!(
            spot(&old(Some(12), None, None)),
            "src/upload.ts:12 (outdated)"
        );
        assert_eq!(spot(&old(None, None, None)), "src/upload.ts (outdated)");
    }

    #[test]
    fn a_comment_loses_bookkeeping_and_blank_runs_and_is_clipped() {
        assert_eq!(
            clean_body("<!-- bot:meta {\"id\":1} -->\r\nUse the config value.\n\n\n  \nThanks!\n"),
            "Use the config value.\n\nThanks!"
        );
        let long = clean_body(&"x".repeat(5_000));
        assert_eq!(chars(&long), COMMENT_CHARS);
        assert!(long.ends_with('…'));
        assert_eq!(clean_body("<!-- only metadata -->"), "(no text)");
    }

    #[test]
    fn the_prompt_lists_each_open_thread_with_its_replies_and_asks() {
        let text = address_prompt(
            &facts(),
            &[
                GhThread {
                    start_line: Some(40),
                    ..thread(
                        "src/upload.ts",
                        Some(42),
                        &[
                            (
                                "alice",
                                "Retry count should come from config,\nnot a literal.",
                            ),
                            ("octocat", "Agreed, moving it."),
                        ],
                    )
                },
                GhThread {
                    is_outdated: true,
                    line: None,
                    original_line: Some(7),
                    ..thread("src/upload.ts", None, &[("bob", "Is this still needed?")])
                },
                GhThread {
                    is_resolved: true,
                    ..thread("src/api.ts", Some(9), &[("carol", "Already fixed.")])
                },
            ],
            3,
        );
        assert!(text.starts_with("Review comments to address on acme/widgets#12: Retry uploads with exponential backoff\n"));
        assert!(text.contains("https://github.com/acme/widgets/pull/12 · feat/retry → main · files: https://github.com/acme/widgets/pull/12/files"));
        assert!(text.contains("2 unresolved review threads, by file (1 outdated: the code changed since, so check it still applies):"));
        assert!(text.contains("1. src/upload.ts (outdated; was line 7) · https://github.com/acme/widgets/pull/12#discussion_r"));
        assert!(text.contains("\n   @bob: Is this still needed?"));
        assert!(text.contains(
            "2. src/upload.ts:40-42 · https://github.com/acme/widgets/pull/12#discussion_r"
        ));
        assert!(text.contains("   @alice: Retry count should come from config,\n     not a literal.\n   @octocat (PR author): Agreed, moving it."));
        assert!(!text.contains("Already fixed"));
        assert!(
            text.contains("If acme/widgets or feat/retry isn't checked out here, say so first.")
        );
        assert!(text.ends_with("Don't reply to or resolve any thread on GitHub: if one needs a reply, draft it here for me to read first."));
    }

    #[test]
    fn replies_past_those_read_and_threads_past_those_asked_are_counted() {
        let mut t = thread("src/upload.ts", Some(3), &[("alice", "one")]);
        t.comments.total_count = Some(13);
        let text = address_prompt(&facts(), &[t], 140);
        assert!(text.contains("   (12 more replies, not read)"));
        assert!(
            text.contains(
                "Only the newest 1 of 140 threads were read; older ones may be open too."
            )
        );
    }

    #[test]
    fn a_bodys_blank_lines_stay_blank_and_a_deleted_account_has_a_name() {
        let text = address_prompt(
            &facts(),
            &[thread(
                "src/upload.ts",
                Some(3),
                &[("alice", "First.\n\nSecond.")],
            )],
            1,
        );
        assert!(text.contains("   @alice: First.\n\n     Second."));
        let mut t = thread("src/upload.ts", Some(3), &[("alice", "x")]);
        t.comments.nodes[0].author = None;
        assert!(address_prompt(&facts(), &[t], 1).contains("   @ghost: x"));
    }

    #[test]
    fn one_long_thread_is_clipped_and_every_thread_together_stays_inside_the_cap() {
        let body = "y".repeat(900);
        let names: Vec<String> = (0..10).map(|i| format!("reviewer{i}")).collect();
        let long: Vec<(&str, &str)> = names.iter().map(|n| (n.as_str(), body.as_str())).collect();
        let threads: Vec<GhThread> = (0..20)
            .map(|i| thread(&format!("src/file{i:02}.ts"), Some(i + 1), &long))
            .collect();
        let text = address_prompt(&facts(), &threads, 20);
        let blocks: Vec<&str> = text
            .split("\n\n")
            .filter(|b| {
                b.split_once(". ")
                    .is_some_and(|(n, _)| n.parse::<usize>().is_ok())
            })
            .collect();
        assert!(!blocks.is_empty() && blocks.len() < 20);
        assert!(blocks.iter().all(|b| chars(b) <= THREAD_CHARS));
        assert!(blocks.iter().map(|b| chars(b)).sum::<usize>() <= REVIEW_CHARS);
        assert!(text.contains(&format!(
            "{} more unresolved threads didn't fit here; they're on the PR's files tab.",
            20 - blocks.len()
        )));
    }

    #[test]
    fn the_query_is_the_batch_alias_shape_with_what_each_comment_said() {
        let query = threads_query(&PrRef {
            url: facts().url,
            repo: "acme/widgets".to_owned(),
            number: 12,
        });
        assert!(query.contains(
            r#"p0: repository(owner: "acme", name: "widgets") { pullRequest(number: 12)"#
        ));
        assert!(query.contains("reviewThreads(last: 100) { totalCount nodes {"));
        assert!(
            query.contains(
                "isResolved isOutdated path line startLine originalLine originalStartLine"
            )
        );
        assert!(
            query
                .contains("comments(first: 10) { totalCount nodes { author { login } body url } }")
        );
    }

    #[test]
    fn the_replys_threads_and_total_or_why_there_are_none() {
        let ok = json!({"data": {"p0": {"pullRequest": {"reviewThreads": {"totalCount": 4, "nodes": [
            {"isResolved": false, "isOutdated": false, "path": "src/upload.ts", "line": 3,
             "comments": {"totalCount": 1, "nodes": [{"author": {"login": "alice"}, "body": "x", "url": "u"}]}}
        ]}}}}});
        let read = parse_threads(&ok.to_string(), "").expect("threads");
        assert_eq!(read.total, 4);
        assert_eq!(
            read.threads[0].comments.nodes[0]
                .author
                .as_ref()
                .map(|a| a.login.as_str()),
            Some("alice")
        );
        assert_eq!(
            parse_threads(&json!({"data": {"p0": null}}).to_string(), ""),
            Err("Not found, or this gh account can't see it".to_owned())
        );
        assert_eq!(
            parse_threads(
                &json!({"errors": [{"message": "Bad credentials"}]}).to_string(),
                ""
            ),
            Err("Bad credentials".to_owned())
        );
        assert_eq!(
            parse_threads("", "gh: no network\n"),
            Err("gh: no network".to_owned())
        );
    }

    proptest! {
        #[test]
        fn every_open_thread_is_in_the_prompt_or_counted(n in 0usize..30, resolved in proptest::collection::vec(any::<bool>(), 30)) {
            let threads: Vec<GhThread> = (0..n)
                .map(|i| GhThread { is_resolved: resolved[i], ..thread(&format!("f{i:02}"), Some(1), &[("a", "b")]) })
                .collect();
            let open = threads.iter().filter(|t| !t.is_resolved).count();
            let text = address_prompt(&facts(), &threads, n);
            prop_assert!(text.starts_with(&address_heading(&facts())));
            let shown = (1..=open).filter(|i| text.contains(&format!("\n{i}. f"))).count();
            prop_assert_eq!(shown, open);
        }
    }
}
