//! One PR read whole, for the full view (Enter): its description, its conversation (comments,
//! reviews, and review threads with the code they are on), its checks and its changed files.
//! One call, asked when the view opens and every [`REFRESH_MS`] while it shows, never by the
//! poll; a failed job's log is a call of its own, once per job.

use serde::Deserialize;
use serde_json::Value;

use crate::address::{GhComment, GhThread, Login, THREAD_COMMENTS, THREADS};
use crate::batch::{PrRef, checks_asked, json_string, parse_checks};
use crate::gh::{CheckItem, Connection, ReviewState};
use crate::lenient::{lenient, read, string_enum};
use crate::status::{latest_runs, progress_of};
use crate::time::parse_iso;

/// How often an open PR is read again (ms).
pub const REFRESH_MS: i64 = 30_000;
/// The newest comments on the PR itself read.
pub const COMMENTS: usize = 50;
/// The newest reviews read.
pub const REVIEWS: usize = 50;
/// The changed files read.
pub const FILES: usize = 100;

/// A review, with what it says (`PullRequestReview`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Review {
    /// `None` for a deleted account.
    #[serde(deserialize_with = "lenient")]
    pub author: Option<Login>,
    #[serde(deserialize_with = "lenient")]
    pub state: ReviewState,
    #[serde(deserialize_with = "lenient")]
    pub body: String,
    #[serde(deserialize_with = "lenient")]
    pub submitted_at: String,
}

string_enum! {
    /// `PatchStatus`: what the PR does to a file.
    pub enum Change {
        Added = "ADDED",
        Changed = "CHANGED",
        Copied = "COPIED",
        Deleted = "DELETED",
        Modified = "MODIFIED",
        Renamed = "RENAMED",
    }
}

/// A file the PR changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChangedFile {
    #[serde(deserialize_with = "lenient")]
    pub path: String,
    #[serde(deserialize_with = "lenient")]
    pub additions: u64,
    #[serde(deserialize_with = "lenient")]
    pub deletions: u64,
    #[serde(deserialize_with = "lenient")]
    pub change_type: Change,
}

/// The PR as [`full_query`] reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Full {
    /// The PR's node id, which an approval names.
    pub id: String,
    /// The description, as written.
    pub body: String,
    /// `None` for a deleted account.
    pub author: Option<Login>,
    pub created_at: String,
    /// The newest comments on the PR itself, oldest first, and how many it has in all.
    pub comments: Vec<GhComment>,
    pub comment_total: usize,
    pub reviews: Vec<Review>,
    pub review_total: usize,
    pub threads: Vec<GhThread>,
    pub thread_total: usize,
    pub files: Vec<ChangedFile>,
    pub file_total: usize,
    /// The head commit's checks, as [`parse_checks`] reads them (job ids and links too).
    pub checks: Vec<CheckItem>,
}

/// The PR as GitHub sends it, before it is put together.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Raw {
    #[serde(deserialize_with = "lenient")]
    id: String,
    #[serde(deserialize_with = "lenient")]
    body: String,
    #[serde(deserialize_with = "lenient")]
    author: Option<Login>,
    #[serde(deserialize_with = "lenient")]
    created_at: String,
    #[serde(deserialize_with = "lenient")]
    comments: Connection<GhComment>,
    #[serde(deserialize_with = "lenient")]
    reviews: Connection<Review>,
    #[serde(deserialize_with = "lenient")]
    review_threads: Connection<GhThread>,
    #[serde(deserialize_with = "lenient")]
    files: Connection<ChangedFile>,
}

/// The query for one PR read whole, in the alias shape [`parse_checks`] reads its checks from.
#[must_use]
pub fn full_query(pr: &PrRef) -> String {
    let mut parts = pr.repo.split('/');
    let owner = json_string(parts.next().unwrap_or_default());
    let name = json_string(parts.next().unwrap_or_default());
    format!(
        "query {{
  p0: repository(owner: {owner}, name: {name}) {{ pullRequest(number: {}) {{
    id body createdAt author {{ login }}
    comments(last: {COMMENTS}) {{ totalCount nodes {{ author {{ login }} body createdAt url }} }}
    reviews(last: {REVIEWS}) {{ totalCount nodes {{ author {{ login }} state body submittedAt }} }}
    reviewThreads(last: {THREADS}) {{ totalCount nodes {{
      id isResolved isOutdated path line startLine originalLine originalStartLine
      comments(first: {THREAD_COMMENTS}) {{ totalCount nodes {{ author {{ login }} body createdAt url diffHunk }} }}
    }} }}
    files(first: {FILES}) {{ totalCount nodes {{ path additions deletions changeType }} }}
    {}
  }} }}
}}",
        pr.number,
        checks_asked("completedAt ")
    )
}

/// `gh api graphql`'s reply to [`full_query`].
///
/// # Errors
///
/// Why there is nothing to show: gh's stderr, GitHub's message, or a PR this account can't
/// see.
pub fn parse_full(stdout: &str, stderr: &str, pr: &PrRef) -> Result<Full, String> {
    let checks = parse_checks(stdout, stderr, pr)?;
    let raw: Raw = serde_json::from_str::<Value>(stdout)
        .ok()
        .and_then(|reply| reply.pointer("/data/p0/pullRequest").cloned())
        .and_then(read)
        .unwrap_or_default();
    let total = |count: Option<usize>, read: usize| count.unwrap_or(read).max(read);
    Ok(Full {
        id: raw.id,
        body: raw.body,
        author: raw.author,
        created_at: raw.created_at,
        comment_total: total(raw.comments.total_count, raw.comments.nodes.len()),
        comments: raw.comments.nodes,
        review_total: total(raw.reviews.total_count, raw.reviews.nodes.len()),
        reviews: raw.reviews.nodes,
        thread_total: total(
            raw.review_threads.total_count,
            raw.review_threads.nodes.len(),
        ),
        threads: raw.review_threads.nodes,
        file_total: total(raw.files.total_count, raw.files.nodes.len()),
        files: raw.files.nodes,
        checks,
    })
}

/// One item of the conversation, by its place in [`Full`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Item {
    Description,
    Comment(usize),
    Review(usize),
    Thread(usize),
}

/// Whether a review says anything its threads don't: a verdict, or words of its own. A bare
/// "commented" review only holds thread comments, and a pending one isn't sent yet.
fn says_more(review: &Review) -> bool {
    match review.state {
        ReviewState::Pending => false,
        ReviewState::Commented => !review.body.trim().is_empty(),
        _ => true,
    }
}

/// The conversation as the PR page has it: the description, then every comment, review and
/// thread in the order they were left (a thread at its first comment). A time that doesn't
/// read goes last.
#[must_use]
pub fn timeline(full: &Full) -> Vec<Item> {
    let at = |text: &str| parse_iso(text).unwrap_or(i64::MAX);
    let comments = full
        .comments
        .iter()
        .enumerate()
        .map(|(i, c)| (at(&c.created_at), Item::Comment(i)));
    let reviews = full
        .reviews
        .iter()
        .enumerate()
        .filter(|(_, r)| says_more(r))
        .map(|(i, r)| (at(&r.submitted_at), Item::Review(i)));
    let threads = full.threads.iter().enumerate().map(|(i, t)| {
        let first = t
            .comments
            .nodes
            .first()
            .map_or("", |c| c.created_at.as_str());
        (at(first), Item::Thread(i))
    });
    let mut rest: Vec<(i64, Item)> = comments.chain(reviews).chain(threads).collect();
    rest.sort_by_key(|&(at, _)| at);
    std::iter::once(Item::Description)
        .chain(rest.into_iter().map(|(_, item)| item))
        .collect()
}

/// The checks as the Checks tab lists them: the latest run of each, worst first, in rollup
/// order within each kind.
#[must_use]
pub fn checks_in_order(items: &[CheckItem]) -> Vec<&CheckItem> {
    let mut checks = latest_runs(items);
    checks.sort_by_key(|item| progress_of(item));
    checks
}

/// The first item to draw, of items `heights` rows tall, so the selected one shows on `room`
/// rows: one item above it for context where both fit, and no rows left blank at the bottom
/// that items above could fill.
#[must_use]
pub fn window(heights: &[usize], selected: usize, room: usize) -> usize {
    let Some(last) = heights.len().checked_sub(1) else {
        return 0;
    };
    let selected = selected.min(last);
    let fits = |from: usize| heights[from..=selected].iter().sum::<usize>() <= room;
    let mut top = if selected > 0 && fits(selected - 1) {
        selected - 1
    } else {
        selected
    };
    while top > 0 && heights[top - 1..].iter().sum::<usize>() <= room {
        top -= 1;
    }
    top
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn pr() -> PrRef {
        PrRef {
            url: "https://github.com/acme/app/pull/12".into(),
            repo: "acme/app".into(),
            number: 12,
        }
    }

    const REPLY: &str = r#"{"data": {"p0": {"pullRequest": {
        "id": "PR_kw12", "body": "Retries uploads.", "createdAt": "2026-10-24T08:00:00Z", "author": {"login": "ada"},
        "comments": {"totalCount": 3, "nodes": [
            {"author": {"login": "preview-bot"}, "body": "Deployed", "createdAt": "2026-10-24T09:30:00Z", "url": "u1"}
        ]},
        "reviews": {"totalCount": 2, "nodes": [
            {"author": {"login": "kai"}, "state": "COMMENTED", "body": "", "submittedAt": "2026-10-24T09:00:00Z"},
            {"author": {"login": "kai"}, "state": "APPROVED", "body": "Ship it", "submittedAt": "2026-10-24T10:00:00Z"}
        ]},
        "reviewThreads": {"totalCount": 1, "nodes": [
            {"id": "T1", "isResolved": false, "path": "src/retry.ts", "line": 18,
             "comments": {"totalCount": 1, "nodes": [
                {"author": {"login": "kai"}, "body": "Cap it?", "createdAt": "2026-10-24T09:00:00Z",
                 "url": "u2", "diffHunk": "@@ -1,2 +1,2 @@\n-a\n+b"}
             ]}}
        ]},
        "files": {"totalCount": 2, "nodes": [
            {"path": "src/retry.ts", "additions": 12, "deletions": 3, "changeType": "MODIFIED"},
            {"path": "src/new.ts", "additions": 40, "deletions": 0, "changeType": "ADDED"}
        ]},
        "commits": {"nodes": [{"commit": {"statusCheckRollup": {"contexts": {"nodes": [
            {"__typename": "CheckRun", "name": "unit", "status": "COMPLETED", "conclusion": "FAILURE",
             "databaseId": 7, "startedAt": "2026-10-24T09:00:00Z", "completedAt": "2026-10-24T09:02:09Z"}
        ]}}}}]}
    }}}}"#;

    #[test]
    fn a_reply_reads_whole() {
        let full = parse_full(REPLY, "", &pr()).expect("it reads");
        assert_eq!(
            (full.id.as_str(), full.body.as_str()),
            ("PR_kw12", "Retries uploads.")
        );
        assert_eq!(full.author.map(|a| a.login).as_deref(), Some("ada"));
        assert_eq!((full.comments.len(), full.comment_total), (1, 3));
        assert_eq!(full.threads[0].id, "T1");
        assert_eq!(
            full.threads[0].comments.nodes[0].diff_hunk,
            "@@ -1,2 +1,2 @@\n-a\n+b"
        );
        assert_eq!(full.files[1].change_type, Change::Added);
        assert_eq!(full.file_total, 2);
        assert_eq!(full.checks[0].database_id, Some(7));
        assert_eq!(
            full.checks[0].completed_at.as_deref(),
            Some("2026-10-24T09:02:09Z")
        );
    }

    #[test]
    fn a_failed_call_or_a_pr_it_cant_see_says_why() {
        assert_eq!(
            parse_full("", "HTTP 401: Bad credentials", &pr()),
            Err("HTTP 401: Bad credentials".into())
        );
        let gone = r#"{"data": {"p0": {"pullRequest": null}}}"#;
        assert!(parse_full(gone, "", &pr()).is_err());
    }

    #[test]
    fn the_conversation_runs_in_the_order_it_was_left() {
        let full = parse_full(REPLY, "", &pr()).expect("it reads");
        // The bare "commented" review only holds the thread, which shows on its own.
        assert_eq!(
            timeline(&full),
            [
                Item::Description,
                Item::Thread(0),
                Item::Comment(0),
                Item::Review(1)
            ]
        );
    }

    #[test]
    fn the_query_names_the_pr_and_asks_what_the_view_shows() {
        let query = full_query(&pr());
        assert!(
            query.contains(r#"repository(owner: "acme", name: "app") { pullRequest(number: 12)"#)
        );
        for field in [
            "diffHunk",
            "changeType",
            "completedAt",
            "databaseId",
            "isResolved",
        ] {
            assert!(query.contains(field), "{field}");
        }
    }

    fn arb_full() -> impl Strategy<Value = Full> {
        let stamp = prop_oneof![
            Just(String::new()),
            (0_u32..24, 0_u32..60).prop_map(|(h, m)| format!("2026-10-24T{h:02}:{m:02}:00Z")),
        ];
        let comment = stamp.clone().prop_map(|created_at| GhComment {
            created_at,
            ..GhComment::default()
        });
        let state = prop_oneof![
            Just(ReviewState::Approved),
            Just(ReviewState::Commented),
            Just(ReviewState::Pending),
            Just(ReviewState::ChangesRequested),
        ];
        let review =
            (state, any::<bool>(), stamp).prop_map(|(state, has_body, submitted_at)| Review {
                state,
                body: if has_body {
                    "words".into()
                } else {
                    String::new()
                },
                submitted_at,
                ..Review::default()
            });
        let thread = prop::collection::vec(comment.clone(), 0..3).prop_map(|nodes| GhThread {
            comments: crate::address::Comments {
                total_count: None,
                nodes,
            },
            ..GhThread::default()
        });
        (
            prop::collection::vec(comment, 0..6),
            prop::collection::vec(review, 0..6),
            prop::collection::vec(thread, 0..6),
        )
            .prop_map(|(comments, reviews, threads)| Full {
                comments,
                reviews,
                threads,
                ..Full::default()
            })
    }

    fn when(full: &Full, item: Item) -> i64 {
        let at = |text: &str| parse_iso(text).unwrap_or(i64::MAX);
        match item {
            Item::Description => i64::MIN,
            Item::Comment(i) => at(&full.comments[i].created_at),
            Item::Review(i) => at(&full.reviews[i].submitted_at),
            Item::Thread(i) => at(full.threads[i]
                .comments
                .nodes
                .first()
                .map_or("", |c| c.created_at.as_str())),
        }
    }

    proptest! {
        #[test]
        fn any_text_reads_or_says_why_and_never_panics(stdout in ".{0,200}", stderr in ".{0,40}") {
            let _ = parse_full(&stdout, &stderr, &pr());
        }

        #[test]
        fn the_timeline_has_everything_once_in_time_order(full in arb_full()) {
            let items = timeline(&full);
            prop_assert_eq!(items.first(), Some(&Item::Description));
            prop_assert!(items.windows(2).all(|w| when(&full, w[0]) <= when(&full, w[1])));
            let comments: Vec<usize> = items.iter().filter_map(|i| match i { Item::Comment(n) => Some(*n), _ => None }).collect();
            prop_assert_eq!(comments.len(), full.comments.len());
            let threads = items.iter().filter(|i| matches!(i, Item::Thread(_))).count();
            prop_assert_eq!(threads, full.threads.len());
            for item in &items {
                if let Item::Review(i) = item {
                    prop_assert!(says_more(&full.reviews[*i]));
                }
            }
            let reviews = items.iter().filter(|i| matches!(i, Item::Review(_))).count();
            prop_assert_eq!(reviews, full.reviews.iter().filter(|r| says_more(r)).count());
        }

        #[test]
        fn the_window_shows_the_selected_item_whenever_it_fits(
            heights in prop::collection::vec(1_usize..8, 0..20),
            selected in 0_usize..25,
            room in 0_usize..30,
        ) {
            let top = window(&heights, selected, room);
            if heights.is_empty() {
                prop_assert_eq!(top, 0);
                return Ok(());
            }
            let selected = selected.min(heights.len() - 1);
            prop_assert!(top <= selected);
            if heights[selected] <= room {
                prop_assert!(heights[top..=selected].iter().sum::<usize>() <= room);
            }
            // Nothing above could fill the rows left at the bottom.
            if top > 0 {
                prop_assert!(heights[top - 1..].iter().sum::<usize>() > room);
            }
        }
    }
}
