//! Merging a PR from its card: what `m` offers, the question it asks first, the `gh pr merge`
//! call, and what GitHub's answer means. Ports of `hooks/pr.ts` (`pickMethod`, `nextMethod`,
//! `mergeQuestion`, `mergeArgv`, `mergeError`, `isServerError`, `mergeReadArgv`,
//! `mergeReadOf`, `afterServerError`); [`crate::status`] holds what GitHub says may be done.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::gh::{GhPr, MergeStateStatus, Permission, PrState};
use crate::status::{MergeMethod, Mergeable, is_conflicted};

/// How long after a server error the PR is read again (ms): GitHub often finishes a merge
/// after the gateway has stopped waiting (the mod's `RETRY_MS`).
pub const RETRY_MS: i64 = 3_000;

/// What a merge press runs (`MergeRun`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MergeRun {
    /// Merge now, by this method.
    Merge(MergeMethod),
    /// Turn auto-merge on, by this method: GitHub merges once checks and review pass.
    Auto(MergeMethod),
    /// Turn auto-merge off.
    CancelAuto,
}

impl MergeRun {
    /// The method it merges by, where it merges.
    #[must_use]
    pub const fn method(self) -> Option<MergeMethod> {
        match self {
            Self::Merge(m) | Self::Auto(m) => Some(m),
            Self::CancelAuto => None,
        }
    }

    /// The same run by another method; cancelling has none.
    #[must_use]
    pub const fn by(self, method: MergeMethod) -> Self {
        match self {
            Self::Merge(_) => Self::Merge(method),
            Self::Auto(_) => Self::Auto(method),
            Self::CancelAuto => Self::CancelAuto,
        }
    }

    /// What the status line says while it runs (`BUSY`).
    #[must_use]
    pub const fn busy(self) -> &'static str {
        match self {
            Self::Merge(_) => "Merging",
            Self::Auto(_) => "Turning on auto-merge for",
            Self::CancelAuto => "Cancelling auto-merge on",
        }
    }
}

/// The method chosen while the repo still allows it, else the first it does (`pickMethod`).
#[must_use]
pub fn pick_method(methods: &[MergeMethod], chosen: Option<MergeMethod>) -> Option<MergeMethod> {
    chosen
        .filter(|m| methods.contains(m))
        .or_else(|| methods.first().copied())
}

/// The method after `current`, round the end (`nextMethod`).
#[must_use]
pub fn next_method(methods: &[MergeMethod], current: MergeMethod) -> MergeMethod {
    let at = methods.iter().position(|&m| m == current);
    at.map_or(current, |i| methods[(i + 1) % methods.len()])
}

/// What `m` runs on a PR, as GitHub said at the last check: merge now, else turn auto-merge
/// on, else turn it off where it is on; `None` where none may be done. `methods` are the
/// methods the repo still takes (those it refused this run left out).
#[must_use]
pub fn run_for(
    merge: &Mergeable,
    methods: &[MergeMethod],
    chosen: Option<MergeMethod>,
) -> Option<MergeRun> {
    if merge.is_auto {
        return merge.can_cancel_auto.then_some(MergeRun::CancelAuto);
    }
    let method = pick_method(methods, chosen)?;
    if merge.can_merge {
        Some(MergeRun::Merge(method))
    } else if merge.can_auto {
        Some(MergeRun::Auto(method))
    } else {
        None
    }
}

/// Where an open PR stands on merging, as the card colours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standing {
    /// Auto-merge is on.
    Auto,
    /// GitHub would merge it now, and the viewer may.
    Can,
    Conflicts,
    Draft,
    /// The viewer may only read the repo.
    ReadOnly,
    /// It waits on checks or review, and auto-merge can be turned on.
    CanAuto,
    /// Not mergeable yet, for a reason GitHub names (behind, blocked, still computing).
    Waiting,
}

/// What the card says of merging an open PR, and where that leaves it; `None` once it is
/// merged or closed.
#[must_use]
pub fn standing(pr: &GhPr, merge: &Mergeable) -> Option<(String, Standing)> {
    if pr.state != PrState::Open {
        return None;
    }
    let base = if pr.base_ref_name.is_empty() {
        "base"
    } else {
        pr.base_ref_name.as_str()
    };
    let methods: Vec<&str> = merge.methods.iter().map(|m| m.as_str()).collect();
    let read_only = matches!(
        pr.repository
            .as_ref()
            .and_then(|r| r.viewer_permission.as_ref()),
        Some(Permission::Read | Permission::Triage)
    );
    Some(if merge.is_auto {
        let how = merge
            .auto_method
            .map_or(String::new(), |m| format!(" ({})", m.as_str()));
        (format!("Auto-merge on{how}"), Standing::Auto)
    } else if merge.can_merge {
        (format!("Can merge: {}", methods.join(" · ")), Standing::Can)
    } else if is_conflicted(pr) {
        (format!("Conflicts with {base}"), Standing::Conflicts)
    } else if pr.is_draft {
        ("A draft: not mergeable yet".to_owned(), Standing::Draft)
    } else if read_only {
        (
            "Read-only here: you can't merge".to_owned(),
            Standing::ReadOnly,
        )
    } else if merge.can_auto {
        ("Auto-merge can be turned on".to_owned(), Standing::CanAuto)
    } else {
        let why = match pr.merge_state_status {
            Some(MergeStateStatus::Behind) => format!("Behind {base}"),
            Some(MergeStateStatus::Blocked) => "Blocked by branch rules".to_owned(),
            Some(MergeStateStatus::Unknown) => "GitHub is still working it out".to_owned(),
            _ => "Not mergeable yet".to_owned(),
        };
        (why, Standing::Waiting)
    })
}

/// The label of what `m` runs, as the actions list it: "Squash & merge" (`MERGE_LABEL`),
/// "Auto-merge · squash", "Cancel auto-merge".
#[must_use]
pub fn label(run: MergeRun) -> String {
    match run {
        MergeRun::Merge(MergeMethod::Squash) => "Squash & merge".to_owned(),
        MergeRun::Merge(MergeMethod::Rebase) => "Rebase & merge".to_owned(),
        MergeRun::Merge(MergeMethod::Merge) => "Merge".to_owned(),
        MergeRun::Auto(m) => format!("Auto-merge · {}", m.as_str()),
        MergeRun::CancelAuto => "Cancel auto-merge".to_owned(),
    }
}

/// What the toast says once a run went through: "Squash-merged #12".
#[must_use]
pub fn done_words(run: MergeRun, number: u64) -> String {
    match run {
        MergeRun::Merge(MergeMethod::Merge) => format!("Merged #{number}"),
        MergeRun::Merge(m) => format!("{}d #{number}", verb(m)),
        MergeRun::Auto(m) => format!("Auto-merge is on for #{number} ({})", m.as_str()),
        MergeRun::CancelAuto => format!("Auto-merge is off for #{number}"),
    }
}

const fn verb(method: MergeMethod) -> &'static str {
    match method {
        MergeMethod::Squash => "Squash-merge",
        MergeMethod::Rebase => "Rebase-merge",
        MergeMethod::Merge => "Merge",
    }
}

/// What the confirm asks (`mergeQuestion`): "Squash-merge #12 into main?".
#[must_use]
pub fn merge_question(run: MergeRun, number: u64, base: &str) -> String {
    let base = if base.is_empty() { "its base" } else { base };
    match run {
        MergeRun::Merge(m) => format!("{} #{number} into {base}?", verb(m)),
        MergeRun::Auto(m) => {
            format!(
                "{} #{number} into {base} once checks and review pass?",
                verb(m)
            )
        }
        MergeRun::CancelAuto => format!("Cancel auto-merge on #{number}?"),
    }
}

/// `gh pr merge`'s arguments (`mergeArgv`).
#[must_use]
pub fn merge_argv(url: &str, run: MergeRun) -> Vec<String> {
    let mut argv = vec!["pr".to_owned(), "merge".to_owned(), url.to_owned()];
    match run {
        MergeRun::CancelAuto => argv.push("--disable-auto".to_owned()),
        MergeRun::Merge(m) => argv.push(format!("--{}", m.as_str())),
        MergeRun::Auto(m) => argv.extend(["--auto".to_owned(), format!("--{}", m.as_str())]),
    }
    argv
}

/// A refusal of the method itself, not of this PR's state, in GitHub's words: the repo's
/// settings, a ruleset's, or linear history's (`METHOD_REFUSED`).
static METHOD_REFUSED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(?:{})",
        [
            r"\bmerge method \w+ (?:merging )?is not allowed",
            r"\b(?:squash|rebase) merg(?:es|ing) (?:are|is) not allowed",
            r"\bmerge commits are not allowed",
            r"\bis not an allowed merge method",
            r"\bmust not contain merge commits",
        ]
        .join("|")
    ))
    .expect("a valid pattern")
});

/// GitHub's side gave up, in gh's words (`SERVER_ERROR`). Such an answer says nothing of
/// whether the merge went through.
static SERVER_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(?:{})",
        [
            r"\bHTTP(?:/[\d.]+)? 5\d\d\b",
            r"\bstatus(?: code)?:? 5\d\d\b",
            r"\b(?:Bad Gateway|Service Unavailable|Gateway Time-?out|Internal Server Error)\b",
            r"\bThis may be the result of a timeout\b",
            r"\bcouldn't respond to your request in time\b",
        ]
        .join("|")
    ))
    .expect("a valid pattern")
});

static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[X✗!]\s+").expect("a valid pattern"));
static GRAPHQL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^GraphQL:\s*").expect("a valid pattern"));
static MUTATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\([a-z]+[A-Z]\w*\)$").expect("a valid pattern"));
static HINT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^To (?:have|use|merge)\b").expect("a valid pattern"));

/// Whether gh's words say GitHub's side gave up (`isServerError`).
#[must_use]
pub fn is_server_error(said: &str) -> bool {
    SERVER_ERROR.is_match(said)
}

/// What gh said, in plain words (`mergeError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeError {
    pub message: String,
    /// GitHub refused the method, not this PR: it isn't offered again this run.
    pub is_method_refused: bool,
}

/// gh's refusal without its ✗, the "GraphQL:" prefix, the mutation's name (a branch or check
/// named in brackets stays), or the hints about gh's own flags.
#[must_use]
pub fn merge_error(stderr: &str) -> MergeError {
    let lines: Vec<String> = stderr
        .split('\n')
        .map(|line| {
            let line = MARK.replace(line.trim(), "");
            let line = GRAPHQL.replace(&line, "");
            MUTATION.replace(&line, "").into_owned()
        })
        .filter(|line| !line.is_empty() && !HINT.is_match(line))
        .collect();
    let message = if lines.is_empty() {
        "gh pr merge failed".to_owned()
    } else {
        lines.join(" ")
    };
    // GitHub having a moment refuses no method, whatever its page says.
    let is_method_refused = !is_server_error(&message) && METHOD_REFUSED.is_match(&message);
    MergeError {
        message,
        is_method_refused,
    }
}

/// What a merge needs to know of a PR after GitHub stumbled: one `gh pr view`, as the same
/// login (`mergeReadArgv`).
#[must_use]
pub fn merge_read_argv(url: &str) -> Vec<String> {
    ["pr", "view", url, "--json", "state,autoMergeRequest"]
        .map(str::to_owned)
        .to_vec()
}

/// A PR as [`merge_read_argv`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRead {
    /// `OPEN`, `MERGED` or `CLOSED`.
    pub state: String,
    pub is_auto: bool,
}

/// `gh pr view --json state,autoMergeRequest`'s answer; `None` where it isn't one
/// (`mergeReadOf`).
#[must_use]
pub fn merge_read_of(stdout: &str) -> Option<MergeRead> {
    let read: Value = serde_json::from_str(stdout).ok()?;
    Some(MergeRead {
        state: read.get("state")?.as_str()?.to_owned(),
        is_auto: read.get("autoMergeRequest").is_some_and(|a| !a.is_null()),
    })
}

/// What came of a run GitHub answered with a server error, as the PR reads now
/// (`afterServerError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    /// It went through: no error.
    Done,
    /// It didn't, and the PR is as it was: run it once more.
    Retry,
    /// The PR can't be read, or has moved on otherwise: the first error stands.
    Failed,
}

#[must_use]
pub fn after_server_error(run: MergeRun, read: Option<&MergeRead>) -> After {
    let Some(read) = read else {
        return After::Failed;
    };
    let is_open = read.state == "OPEN";
    let is_merged = read.state == "MERGED";
    match run {
        MergeRun::Merge(_) if is_merged => After::Done,
        // gh merges at once where nothing is left to wait for.
        MergeRun::Auto(_) if is_merged || (is_open && read.is_auto) => After::Done,
        MergeRun::Merge(_) | MergeRun::Auto(_) if is_open => After::Retry,
        MergeRun::CancelAuto if is_open && read.is_auto => After::Retry,
        MergeRun::CancelAuto if is_open => After::Done,
        _ => After::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use MergeMethod::{Merge, Rebase, Squash};
    use proptest::prelude::*;

    const URL: &str = "https://github.com/acme/app/pull/7";

    #[test]
    fn the_method_chosen_while_allowed_else_the_first() {
        assert_eq!(pick_method(&[Rebase], Some(Squash)), Some(Rebase));
        assert_eq!(pick_method(&[Squash, Rebase], Some(Rebase)), Some(Rebase));
        assert_eq!(pick_method(&[], Some(Rebase)), None);
        assert_eq!(next_method(&[Squash, Rebase, Merge], Merge), Squash);
        assert_eq!(next_method(&[Squash], Rebase), Rebase);
    }

    #[test]
    fn m_merges_now_else_turns_auto_merge_on_else_off() {
        let methods = [Squash, Rebase];
        let now = Mergeable {
            can_merge: true,
            ..Mergeable::default()
        };
        assert_eq!(run_for(&now, &methods, None), Some(MergeRun::Merge(Squash)));
        let waiting = Mergeable {
            can_auto: true,
            ..Mergeable::default()
        };
        assert_eq!(
            run_for(&waiting, &methods, Some(Rebase)),
            Some(MergeRun::Auto(Rebase))
        );
        let on = Mergeable {
            is_auto: true,
            can_cancel_auto: true,
            ..Mergeable::default()
        };
        assert_eq!(run_for(&on, &[], None), Some(MergeRun::CancelAuto));
        assert_eq!(run_for(&Mergeable::default(), &methods, None), None);
        // Every method refused: nothing to merge by.
        assert_eq!(run_for(&now, &[], None), None);
    }

    #[test]
    fn the_labels_and_what_a_run_did() {
        assert_eq!(label(MergeRun::Merge(Squash)), "Squash & merge");
        assert_eq!(label(MergeRun::Auto(Rebase)), "Auto-merge · rebase");
        assert_eq!(done_words(MergeRun::Merge(Squash), 12), "Squash-merged #12");
        assert_eq!(done_words(MergeRun::Merge(Merge), 12), "Merged #12");
        assert_eq!(
            done_words(MergeRun::CancelAuto, 12),
            "Auto-merge is off for #12"
        );
    }

    #[test]
    fn where_an_open_pr_stands() {
        let pr = |v: serde_json::Value| -> GhPr { serde_json::from_value(v).expect("a PR") };
        let open = |status: &str| {
            pr(
                serde_json::json!({"state": "OPEN", "mergeStateStatus": status, "baseRefName": "main"}),
            )
        };
        let none = Mergeable::default();
        assert_eq!(
            standing(&open("DIRTY"), &none),
            Some(("Conflicts with main".to_owned(), Standing::Conflicts))
        );
        assert_eq!(
            standing(&open("BEHIND"), &none),
            Some(("Behind main".to_owned(), Standing::Waiting))
        );
        let can = Mergeable {
            can_merge: true,
            methods: vec![Squash, Rebase],
            ..none.clone()
        };
        assert_eq!(
            standing(&open("CLEAN"), &can),
            Some(("Can merge: squash · rebase".to_owned(), Standing::Can))
        );
        assert_eq!(
            standing(&pr(serde_json::json!({"state": "MERGED"})), &none),
            None
        );
    }

    #[test]
    fn gh_pr_merge_argv() {
        assert_eq!(
            merge_argv(URL, MergeRun::Merge(Rebase)),
            ["pr", "merge", URL, "--rebase"]
        );
        assert_eq!(
            merge_argv(URL, MergeRun::Auto(Squash)),
            ["pr", "merge", URL, "--auto", "--squash"]
        );
        assert_eq!(
            merge_argv(URL, MergeRun::CancelAuto),
            ["pr", "merge", URL, "--disable-auto"]
        );
    }

    #[test]
    fn the_question_names_the_method_the_pr_and_its_base() {
        assert_eq!(
            merge_question(MergeRun::Merge(Squash), 12, "main"),
            "Squash-merge #12 into main?"
        );
        assert_eq!(
            merge_question(MergeRun::Auto(Merge), 12, "main"),
            "Merge #12 into main once checks and review pass?"
        );
        assert_eq!(
            merge_question(MergeRun::CancelAuto, 12, ""),
            "Cancel auto-merge on #12?"
        );
        assert_eq!(
            merge_question(MergeRun::Merge(Rebase), 3, ""),
            "Rebase-merge #3 into its base?"
        );
    }

    #[test]
    fn ghs_refusal_in_plain_words_and_whether_it_refused_the_method() {
        assert_eq!(
            merge_error(
                "GraphQL: Merge method squash merging is not allowed on this repository (mergePullRequest)\n"
            ),
            MergeError {
                message: "Merge method squash merging is not allowed on this repository".to_owned(),
                is_method_refused: true
            }
        );
        assert_eq!(
            merge_error(
                "X Pull request acme/app#7 is not mergeable: the base branch policy prohibits the merge.\n\
                 To have the pull request merged after all the requirements have been met, add the `--auto` flag.\n"
            ),
            MergeError {
                message: "Pull request acme/app#7 is not mergeable: the base branch policy prohibits the merge.".to_owned(),
                is_method_refused: false
            }
        );
        assert_eq!(merge_error("").message, "gh pr merge failed");
    }

    #[test]
    fn every_wording_github_refuses_a_method_with_counts() {
        for said in [
            "GraphQL: Merge method rebase merging is not allowed on this repository (enablePullRequestAutoMerge)",
            "GraphQL: Squash merges are not allowed on this repository. (mergePullRequest)",
            "Rebase merges are not allowed on this repository.",
            "Merge commits are not allowed on this repository.",
            "Rebase is not an allowed merge method",
            "Repository rule violations found\n\nThis branch must not contain merge commits.",
        ] {
            assert!(merge_error(said).is_method_refused, "{said}");
        }
        for said in [
            "GraphQL: Pull request is in clean status, but the merge method squash could not be applied (mergePullRequest)",
            "GraphQL: Base branch was modified. Review and try the merge again. (mergePullRequest)",
            "X Pull request acme/app#7 is not mergeable: 2 of 3 required status checks are expected.",
            "GraphQL: Merging is blocked: the merge queue is required (mergePullRequest)",
        ] {
            assert!(!merge_error(said).is_method_refused, "{said}");
        }
    }

    #[test]
    fn only_the_mutations_name_is_dropped_from_the_end() {
        assert_eq!(
            merge_error(
                r#"GraphQL: Required status check "test" is failing (build) (mergePullRequest)"#
            )
            .message,
            r#"Required status check "test" is failing (build)"#
        );
        assert_eq!(
            merge_error("X Could not merge: protected branch rules apply (main)").message,
            "Could not merge: protected branch rules apply (main)"
        );
    }

    const SERVER_ERRORS: [&str; 8] = [
        "HTTP 502: Bad Gateway (https://api.github.com/graphql)",
        "HTTP 503: Service Unavailable (https://api.github.com/graphql)",
        "HTTP 504: Gateway Timeout (https://api.github.com/graphql)",
        "gh: Server Error (HTTP 502)",
        r#"non-200 OK status code: 502 Bad Gateway body: "<html><body><h1>502 Bad Gateway</h1></body></html>""#,
        "GraphQL: Something went wrong while executing your query. This may be the result of a timeout, or it could be a GitHub bug.",
        "GraphQL: We couldn't respond to your request in time. Sorry about that.",
        "X Pull request acme/app#7 could not be merged: 500 Internal Server Error",
    ];

    #[test]
    fn githubs_side_giving_up_is_a_server_error_and_refuses_no_method() {
        for said in SERVER_ERRORS {
            assert!(is_server_error(said), "{said}");
            assert!(!merge_error(said).is_method_refused, "{said}");
        }
        for said in [
            "GraphQL: Squash merges are not allowed on this repository. (mergePullRequest)",
            "HTTP 401: Bad credentials (https://api.github.com/graphql)",
            "HTTP 404: Not Found (https://api.github.com/graphql)",
            r#"GraphQL: Required status check "fix-502" is failing (mergePullRequest)"#,
        ] {
            assert!(!is_server_error(said), "{said}");
        }
        assert!(!merge_error("HTTP 502: Squash merges are not allowed on this repository (https://api.github.com/graphql)").is_method_refused);
    }

    #[test]
    fn the_pr_read_again_and_what_it_means() {
        assert_eq!(
            merge_read_of("{\"autoMergeRequest\":null,\"state\":\"MERGED\"}\n"),
            Some(MergeRead {
                state: "MERGED".to_owned(),
                is_auto: false
            })
        );
        assert_eq!(
            merge_read_of(r#"{"autoMergeRequest":{"mergeMethod":"SQUASH"},"state":"OPEN"}"#),
            Some(MergeRead {
                state: "OPEN".to_owned(),
                is_auto: true
            })
        );
        for stdout in [
            "",
            "null",
            r#"{"autoMergeRequest":null}"#,
            "<html>502</html>",
        ] {
            assert_eq!(merge_read_of(stdout), None, "{stdout}");
        }
        let read = |state: &str, is_auto| MergeRead {
            state: state.to_owned(),
            is_auto,
        };
        let reads = [
            read("MERGED", false),
            read("OPEN", false),
            read("OPEN", true),
            read("CLOSED", false),
        ];
        let after = |run| {
            reads
                .iter()
                .map(|r| after_server_error(run, Some(r)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            after(MergeRun::Merge(Squash)),
            [After::Done, After::Retry, After::Retry, After::Failed]
        );
        assert_eq!(
            after(MergeRun::Auto(Squash)),
            [After::Done, After::Retry, After::Done, After::Failed]
        );
        assert_eq!(
            after(MergeRun::CancelAuto),
            [After::Failed, After::Done, After::Retry, After::Failed]
        );
        assert_eq!(
            after_server_error(MergeRun::Merge(Squash), None),
            After::Failed
        );
    }

    fn method() -> impl Strategy<Value = MergeMethod> {
        prop_oneof![Just(Squash), Just(Rebase), Just(Merge)]
    }

    proptest! {
        #[test]
        fn next_method_cycles_through_every_method_once(methods in proptest::sample::subsequence(vec![Squash, Rebase, Merge], 1..=3), start in 0usize..3) {
            let first = methods[start % methods.len()];
            let mut seen = vec![first];
            let mut at = first;
            for _ in 1..methods.len() {
                at = next_method(&methods, at);
                prop_assert!(!seen.contains(&at));
                seen.push(at);
            }
            prop_assert_eq!(next_method(&methods, at), first);
        }

        #[test]
        fn a_picked_method_is_always_one_the_repo_takes(methods in proptest::sample::subsequence(vec![Squash, Rebase, Merge], 0..=3), chosen in proptest::option::of(method())) {
            let picked = pick_method(&methods, chosen);
            prop_assert_eq!(picked.is_some(), !methods.is_empty());
            prop_assert!(picked.is_none_or(|m| methods.contains(&m)));
        }
    }
}
