//! What a card says of a PR: its checks, its merge options, its stage, pill and headline.
//! A port of `hooks/pr.ts` (`latestRuns`, `verdictOf`, `summarizeChecks`, `activity`,
//! `metaOf`, `methodsOf`, `mergeableOf`, `mergeOf`, `PILL`, `derive`, `stepIndex`).
//!
//! The words are the mod's, character for character: `companion/fixtures/status.ts` holds
//! the two to the same cards (`tests/fixtures.rs`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::gh::{
    CheckItem, CheckKind, CheckStatus, Conclusion, GhMergeMethod, GhPr, MergeStateStatus,
    MergeableState, Permission, PrState, RepoMerge, ReviewDecision, StatusState,
};
use crate::review::{Reviews, Verdict, reviews_of, to_address, unresolved};
use crate::time::parse_iso;

// ---- Checks ---------------------------------------------------------------------------

/// Whether an item is a commit status context rather than a check run (`isStatusContext`):
/// its `__typename` says so, or it has a `state` at all (even `null`).
#[must_use]
pub fn is_status_context(item: &CheckItem) -> bool {
    item.typename == Some(CheckKind::StatusContext) || item.state.is_some()
}

/// The latest run of each check, in the order of those runs (`latestRuns`). GitHub keeps
/// every run of a check on the head commit (each `pull_request` event starts another) and the
/// rollup lists them all; the PR page shows the latest of each.
///
/// A check run is keyed by `run:<workflow>/<name>`, a status by `status:<context>`, as the
/// TypeScript joins them. A run not started yet is the newest of its check; a stamp that
/// doesn't parse counts as the epoch; on a tie the later item wins.
#[must_use]
pub fn latest_runs(items: &[CheckItem]) -> Vec<&CheckItem> {
    let mut latest: HashMap<String, (usize, i64)> = HashMap::new();
    for (order, item) in items.iter().enumerate() {
        let is_context = is_status_context(item);
        let key = if is_context {
            format!("status:{}", item.context.as_deref().unwrap_or(""))
        } else {
            format!(
                "run:{}/{}",
                item.workflow.as_deref().unwrap_or(""),
                item.name.as_deref().unwrap_or("")
            )
        };
        let stamp = if is_context {
            &item.created_at
        } else {
            &item.started_at
        };
        // `Date.parse` never reaches i64::MAX, so it stands for +Infinity exactly.
        let at = match stamp {
            None if is_context => 0,
            None => i64::MAX,
            Some(text) => parse_iso(text).unwrap_or(0),
        };
        let is_newer = latest.get(&key).is_none_or(|&(held_order, held_at)| {
            at > held_at || (at == held_at && order > held_order)
        });
        if is_newer {
            latest.insert(key, (order, at));
        }
    }
    let mut kept: Vec<usize> = latest.into_values().map(|(order, _)| order).collect();
    kept.sort_unstable();

    kept.into_iter().map(|order| &items[order]).collect()
}

/// A check's verdict (`verdictOf`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckVerdict {
    Passed,
    Pending,
    Failed,
}

/// Where one check stands (`verdictOf`): a status by its state (anything but success,
/// pending or expected fails); a run is pending until completed, then passed by a success,
/// neutral or skipped conclusion, else failed.
#[must_use]
pub fn verdict_of(item: &CheckItem) -> CheckVerdict {
    if is_status_context(item) {
        return match item.state.as_ref().and_then(Option::as_ref) {
            Some(StatusState::Success) => CheckVerdict::Passed,
            Some(StatusState::Pending | StatusState::Expected) => CheckVerdict::Pending,
            _ => CheckVerdict::Failed,
        };
    }
    if item.status != Some(CheckStatus::Completed) {
        return CheckVerdict::Pending;
    }
    match item.conclusion {
        Some(Conclusion::Success | Conclusion::Neutral | Conclusion::Skipped) => {
            CheckVerdict::Passed
        }
        _ => CheckVerdict::Failed,
    }
}

/// The checks as a card counts them (`Checks`): the latest run of each.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checks {
    pub passed: usize,
    pub failed: usize,
    pub pending: usize,
    pub total: usize,
    /// The failed checks' names, in rollup order.
    pub failing: Vec<String>,
    /// Pending checks started on a runner (or a status CI has picked up), by name.
    pub running: Vec<String>,
    /// Pending checks still waiting for one.
    pub queued: usize,
}

/// The rollup counted (`summarizeChecks`), the latest run of each check only.
#[must_use]
pub fn summarize_checks(items: &[CheckItem]) -> Checks {
    let mut checks = Checks::default();
    for item in latest_runs(items) {
        checks.total += 1;
        let is_context = is_status_context(item);
        let verdict = verdict_of(item);
        let name = || {
            item.name
                .clone()
                .or_else(|| item.context.clone())
                .unwrap_or_else(|| "check".to_owned())
        };
        match verdict {
            CheckVerdict::Passed => checks.passed += 1,
            CheckVerdict::Failed => {
                checks.failed += 1;
                checks.failing.push(name());
            }
            CheckVerdict::Pending => {
                checks.pending += 1;
                // A status only says PENDING once its CI has picked it up; EXPECTED still waits.
                let is_started = if is_context {
                    item.state == Some(Some(StatusState::Pending))
                } else {
                    item.status == Some(CheckStatus::InProgress)
                };
                if is_started {
                    checks.running.push(name());
                } else {
                    checks.queued += 1;
                }
            }
        }
    }

    checks
}

/// "Running build, e2e +1 · 2 queued": what CI is doing right now (`activity`). Empty when
/// nothing is pending.
#[must_use]
pub fn activity(checks: &Checks) -> String {
    let running = if checks.running.is_empty() {
        String::new()
    } else {
        let names = checks.running[..checks.running.len().min(2)].join(", ");
        let more = if checks.running.len() > 2 {
            format!(" +{}", checks.running.len() - 2)
        } else {
            String::new()
        };
        format!("Running {names}{more}")
    };
    let queued = if checks.queued > 0 {
        format!("{} queued", checks.queued)
    } else {
        String::new()
    };

    [running, queued]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

// ---- Merging ----------------------------------------------------------------------------

/// A merge method, as a card offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeMethod {
    Squash,
    Rebase,
    Merge,
}

impl MergeMethod {
    /// Every method, in the order they are offered where the viewer has no default (`METHODS`).
    pub const ALL: [Self; 3] = [Self::Squash, Self::Rebase, Self::Merge];

    /// Its name, as `gh pr merge --<name>` takes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Squash => "squash",
            Self::Rebase => "rebase",
            Self::Merge => "merge",
        }
    }

    /// Whether the repo allows it (`ALLOWS`).
    const fn is_allowed(self, repo: &RepoMerge) -> bool {
        match self {
            Self::Squash => repo.squash_merge_allowed,
            Self::Rebase => repo.rebase_merge_allowed,
            Self::Merge => repo.merge_commit_allowed,
        }
    }

    /// GitHub's name for a method, in any case (`asMethod`).
    fn of(value: Option<&GhMergeMethod>) -> Option<Self> {
        let lower = value?.as_str().to_lowercase();
        Self::ALL.into_iter().find(|m| m.as_str() == lower)
    }
}

/// What the viewer may do about merging, as GitHub said at the last check (`Mergeable`).
/// [`Mergeable::default`] is `noMerge()`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the mod's `Mergeable`, field for field"
)]
pub struct Mergeable {
    /// The methods the repo allows, the viewer's default first.
    pub methods: Vec<MergeMethod>,
    /// GitHub would merge it now, and the viewer may.
    pub can_merge: bool,
    /// It waits on checks or review, and the viewer may turn auto-merge on.
    pub can_auto: bool,
    pub is_auto: bool,
    /// The method auto-merge will use, where GitHub said.
    pub auto_method: Option<MergeMethod>,
    pub can_cancel_auto: bool,
}

/// The methods the repo allows, the viewer's default first while the repo still allows it
/// (`methodsOf`).
#[must_use]
pub fn methods_of(repo: Option<&RepoMerge>) -> Vec<MergeMethod> {
    let allowed: Vec<MergeMethod> = MergeMethod::ALL
        .into_iter()
        .filter(|m| repo.is_some_and(|r| m.is_allowed(r)))
        .collect();
    match MergeMethod::of(repo.and_then(|r| r.viewer_default_merge_method.as_ref())) {
        Some(preferred) if allowed.contains(&preferred) => std::iter::once(preferred)
            .chain(allowed.into_iter().filter(|&m| m != preferred))
            .collect(),
        _ => allowed,
    }
}

/// GitHub can't merge the head into the base cleanly, draft or not (`isConflicted`).
#[must_use]
pub fn is_conflicted(pr: &GhPr) -> bool {
    pr.merge_state_status == Some(MergeStateStatus::Dirty)
        || pr.mergeable == Some(MergeableState::Conflicting)
}

/// GitHub merges these now: `HAS_HOOKS` and `UNSTABLE` (an optional check failing) only warn
/// (`MERGES_NOW`).
fn merges_now(pr: &GhPr) -> bool {
    matches!(
        pr.merge_state_status,
        Some(MergeStateStatus::Clean | MergeStateStatus::HasHooks | MergeStateStatus::Unstable)
    )
}

/// Merging takes `WRITE` (`MAY_MERGE`).
fn may_merge(repo: Option<&RepoMerge>) -> bool {
    matches!(
        repo.and_then(|r| r.viewer_permission.as_ref()),
        Some(Permission::Write | Permission::Maintain | Permission::Admin)
    )
}

/// What the card offers for merging (`mergeableOf`).
#[must_use]
pub fn mergeable_of(pr: &GhPr) -> Mergeable {
    if pr.state != PrState::Open {
        return Mergeable::default();
    }
    let repo = pr.repository.as_ref();
    let methods = methods_of(repo);
    let is_auto = pr.auto_merge_request.is_some();
    let is_eligible = !pr.is_draft && !is_conflicted(pr) && !is_auto && !methods.is_empty();
    let can_merge = is_eligible && may_merge(repo) && merges_now(pr);
    // Held back by checks or review, not by a stale branch.
    let is_waiting = pr.merge_state_status == Some(MergeStateStatus::Blocked)
        || (pr.merge_state_status != Some(MergeStateStatus::Behind)
            && summarize_checks(pr.checks()).pending > 0);

    Mergeable {
        methods,
        can_merge,
        // False already where the repo doesn't allow auto-merge.
        can_auto: is_eligible
            && !can_merge
            && is_waiting
            && pr.viewer_can_enable_auto_merge == Some(true),
        is_auto,
        auto_method: MergeMethod::of(
            pr.auto_merge_request
                .as_ref()
                .and_then(|a| a.merge_method.as_ref()),
        ),
        can_cancel_auto: is_auto && pr.viewer_can_disable_auto_merge == Some(true),
    }
}

/// As [`mergeable_of`], where an older mod's entry has no merge facts (`mergeOf`): the card
/// keeps what it last knew (`kept`) rather than lose its merge row for a round. What the
/// entry says of the PR itself still counts: no button on a draft or a conflicted branch, and
/// no merge now unless GitHub would merge it now.
#[must_use]
pub fn merge_of(pr: &GhPr, kept: &Mergeable) -> Mergeable {
    if pr.state != PrState::Open || pr.repository.is_some() {
        return mergeable_of(pr);
    }
    if pr.is_draft || is_conflicted(pr) {
        return Mergeable {
            can_merge: false,
            can_auto: false,
            ..kept.clone()
        };
    }

    Mergeable {
        can_merge: kept.can_merge && merges_now(pr),
        ..kept.clone()
    }
}

/// The PR's own facts a card shows beside its status (`metaOf`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub branch: String,
    pub base: String,
    pub additions: u64,
    pub deletions: u64,
    /// Empty for a deleted account.
    pub author: String,
    pub merged_at: Option<String>,
    pub is_conflicted: bool,
}

/// The PR's own facts (`metaOf`).
#[must_use]
pub fn meta_of(pr: &GhPr) -> Meta {
    Meta {
        branch: pr.head_ref_name.clone(),
        base: pr.base_ref_name.clone(),
        additions: pr.additions,
        deletions: pr.deletions,
        author: pr
            .author
            .as_ref()
            .map(|a| a.login.clone())
            .unwrap_or_default(),
        merged_at: pr.merged_at.clone(),
        is_conflicted: is_conflicted(pr),
    }
}

// ---- The card ---------------------------------------------------------------------------

/// Where a PR is on its way to merging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Draft,
    Checks,
    Review,
    Ready,
    Merged,
    Closed,
    /// Not read yet.
    Unknown,
}

/// The steps a card's stepper shows (`STEPS`).
pub const STEPS: [&str; 4] = ["Open", "CI", "Review", "Merge"];

/// Which step of [`STEPS`] the PR is on: steps before it are done, it carries the tone
/// (`stepIndex`). 4: every step done.
#[must_use]
pub const fn step_index(stage: Stage) -> u8 {
    match stage {
        Stage::Draft | Stage::Unknown => 0,
        Stage::Checks => 1,
        Stage::Review => 2,
        Stage::Ready | Stage::Closed => 3,
        Stage::Merged => 4,
    }
}

/// A card's colour, by meaning; the theme picks the colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tone {
    Success,
    Error,
    Warning,
    Merged,
    Subtle,
    Suggestion,
}

/// The state a card's pill names (`PillKind`, a key of `PILL`). Serialized by its key
/// (`ciFailing`), as the fixtures name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PillKind {
    Merged,
    Closed,
    Draft,
    Conflicts,
    CiFailing,
    CiFailed,
    CiRunning,
    Changes,
    Comments,
    InReview,
    Behind,
    Blocked,
    Checking,
    AutoMerging,
    Ready,
    Loading,
}

impl PillKind {
    /// Every kind, in `PILL`'s order.
    pub const ALL: [Self; 16] = [
        Self::Merged,
        Self::Closed,
        Self::Draft,
        Self::Conflicts,
        Self::CiFailing,
        Self::CiFailed,
        Self::CiRunning,
        Self::Changes,
        Self::Comments,
        Self::InReview,
        Self::Behind,
        Self::Blocked,
        Self::Checking,
        Self::AutoMerging,
        Self::Ready,
        Self::Loading,
    ];

    /// What the pill says (`PILL`).
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::Merged => "✓ MERGED",
            Self::Closed => "✕ CLOSED",
            Self::Draft => "✎ DRAFT",
            Self::Conflicts => "⚠ CONFLICTS",
            Self::CiFailing => "✗ CI FAILING",
            Self::CiFailed => "✗ CI FAILED",
            Self::CiRunning => "● CI RUNNING",
            Self::Changes => "↺ CHANGES REQUESTED",
            Self::Comments => "💬 COMMENTS",
            Self::InReview => "◷ IN REVIEW",
            Self::Behind => "↓ BEHIND",
            Self::Blocked => "⏸ BLOCKED",
            Self::Checking => "◌ CHECKING",
            Self::AutoMerging => "⇢ AUTO-MERGING",
            Self::Ready => "✓ READY",
            Self::Loading => "◌ LOADING",
        }
    }

    /// Its key in `PILL` (`ciFailing`).
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Closed => "closed",
            Self::Draft => "draft",
            Self::Conflicts => "conflicts",
            Self::CiFailing => "ciFailing",
            Self::CiFailed => "ciFailed",
            Self::CiRunning => "ciRunning",
            Self::Changes => "changes",
            Self::Comments => "comments",
            Self::InReview => "inReview",
            Self::Behind => "behind",
            Self::Blocked => "blocked",
            Self::Checking => "checking",
            Self::AutoMerging => "autoMerging",
            Self::Ready => "ready",
            Self::Loading => "loading",
        }
    }

    /// The kind a pill's text names, where it names one (band.ts's `KIND`).
    #[must_use]
    pub fn of_text(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.text() == text)
    }
}

/// What a card says of a PR's status (`Derived`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub stage: Stage,
    pub headline: String,
    pub pill: PillKind,
    pub tone: Tone,
    /// Something is under way (CI, GitHub computing, auto-merge): the spinner and the fast poll.
    pub is_moving: bool,
    pub checks: Checks,
}

impl Derived {
    /// A card before any data (`placeholder`).
    #[must_use]
    pub fn loading() -> Self {
        Self {
            stage: Stage::Unknown,
            headline: "Fetching from GitHub…".to_owned(),
            pill: PillKind::Loading,
            tone: Tone::Subtle,
            is_moving: true,
            checks: Checks::default(),
        }
    }
}

/// A PR's stage, tone, pill and headline (`derive`). `kept`: the card's last review facts,
/// for an entry an older mod wrote (none of its own), so the headline doesn't flip back to
/// "Waiting for review" for a round.
#[must_use]
pub fn derive(pr: &GhPr, kept: Option<&Reviews>) -> Derived {
    let checks = summarize_checks(pr.checks());
    let Said(stage, tone, pill, headline, is_moving) = status_of(pr, &checks, kept);

    Derived {
        stage,
        headline,
        pill,
        tone,
        is_moving,
        checks,
    }
}

/// What `derive`'s `at(…)` builds: all of a [`Derived`] but its checks.
struct Said(Stage, Tone, PillKind, String, bool);

/// A settled state (`at`'s `isMoving` default).
fn still(stage: Stage, tone: Tone, pill: PillKind, headline: impl Into<String>) -> Said {
    Said(stage, tone, pill, headline.into(), false)
}

/// Merged, closed, draft, conflicted, or CI: what outranks review.
fn status_of(pr: &GhPr, checks: &Checks, kept: Option<&Reviews>) -> Said {
    let (passed, total, pending) = (checks.passed, checks.total, checks.pending);
    // Only a draft or a running CI says it.
    let ci = || format!("{} · {passed}/{total} passed", activity(checks));

    match pr.state {
        PrState::Merged => return still(Stage::Merged, Tone::Merged, PillKind::Merged, "Merged"),
        PrState::Closed => {
            let headline = "Closed without merging";
            return still(Stage::Closed, Tone::Subtle, PillKind::Closed, headline);
        }
        PrState::Open | PrState::Other(_) => {}
    }
    let names = checks.failing[..checks.failing.len().min(2)].join(", ");
    let failing = match checks.failing.len() {
        n if n > 2 => format!("{names} +{}", n - 2),
        _ => names,
    };
    if pr.is_draft {
        let draft = if is_conflicted(pr) {
            "Draft · merge conflicts"
        } else {
            "Draft"
        };
        let headline = if pending > 0 {
            format!("{draft} · {}", ci())
        } else {
            draft.to_owned()
        };
        return Said(
            Stage::Draft,
            Tone::Subtle,
            PillKind::Draft,
            headline,
            pending > 0,
        );
    }
    // A conflict blocks the merge whatever CI says, and checks on a conflicted head are often
    // stale (GitHub cannot build the merge commit), so it outranks CI and review. CI stays in
    // the line, and on the stepper, as the second thing to know.
    if is_conflicted(pr) {
        let stage = if checks.failed > 0 || pending > 0 {
            Stage::Checks
        } else {
            Stage::Ready
        };
        let status = if checks.failed > 0 {
            format!(" · failing: {failing}")
        } else if pending > 0 {
            format!(" · {}", activity(checks))
        } else {
            String::new()
        };
        let headline = format!("Merge conflicts with base{status}");
        return Said(
            stage,
            Tone::Error,
            PillKind::Conflicts,
            headline,
            pending > 0,
        );
    }
    if checks.failed > 0 {
        // Still running checks keep the card live: the spinner, the shimmer and the fast poll.
        return if pending > 0 {
            let headline = format!("Failing: {failing} · {pending} still running");
            Said(
                Stage::Checks,
                Tone::Error,
                PillKind::CiFailing,
                headline,
                true,
            )
        } else {
            still(
                Stage::Checks,
                Tone::Error,
                PillKind::CiFailed,
                format!("Failing: {failing}"),
            )
        };
    }
    if pending > 0 {
        return Said(
            Stage::Checks,
            Tone::Warning,
            PillKind::CiRunning,
            ci(),
            true,
        );
    }

    settled(pr, kept)
}

/// CI green: review, then what GitHub says of merging.
fn settled(pr: &GhPr, kept: Option<&Reviews>) -> Said {
    let own = reviews_of(pr);
    let reviews = own.as_ref().or(kept);
    // "2 unresolved threads", or nothing: open conversations, which a repo may require resolved.
    let threads = reviews.map(unresolved).unwrap_or_default();
    let also = if threads.is_empty() {
        String::new()
    } else {
        format!(" · {threads}")
    };
    let decision = pr.review_decision.as_ref();
    if decision == Some(&ReviewDecision::ChangesRequested) {
        return still(
            Stage::Review,
            Tone::Error,
            PillKind::Changes,
            "Changes requested",
        );
    }
    // Comments waiting for the author, where nobody has approved yet: a PR still in review, or
    // one held back (BLOCKED) in a repo that needs no approval.
    let is_blocked = pr.merge_state_status == Some(MergeStateStatus::Blocked);
    let is_waiting_on_author = decision == Some(&ReviewDecision::ReviewRequired)
        || (decision != Some(&ReviewDecision::Approved) && is_blocked);
    if let Some(reviews) = reviews
        && !threads.is_empty()
        && is_waiting_on_author
    {
        return still(
            Stage::Review,
            Tone::Warning,
            PillKind::Comments,
            to_address(reviews),
        );
    }
    if decision == Some(&ReviewDecision::ReviewRequired) {
        let is_looked_at =
            reviews.is_some_and(|r| r.reviewers.iter().any(|r| r.verdict != Verdict::Requested));
        let headline = if is_looked_at {
            "Reviewed · waiting for approval"
        } else {
            "Waiting for review"
        };
        return still(Stage::Review, Tone::Warning, PillKind::InReview, headline);
    }

    let is_auto = pr.auto_merge_request.is_some();
    match pr.merge_state_status {
        Some(MergeStateStatus::Behind) => {
            let headline = if is_auto {
                "Behind base · auto-merge on"
            } else {
                "Branch is behind base"
            };
            still(Stage::Ready, Tone::Warning, PillKind::Behind, headline)
        }
        Some(MergeStateStatus::Blocked) => {
            // Approved, held back, threads open: the likely cause where the repo requires
            // conversations resolved (GitHub says which rule only to admins).
            let why = match (threads.is_empty(), is_auto) {
                (false, _) => format!("Blocked{also}"),
                (true, true) => "Blocked".to_owned(),
                (true, false) => "Blocked by branch rules".to_owned(),
            };
            let headline = if is_auto {
                format!("{why} · auto-merge on")
            } else {
                why
            };
            still(Stage::Ready, Tone::Warning, PillKind::Blocked, headline)
        }
        Some(MergeStateStatus::Unknown) => {
            let headline = "Computing mergeability…".to_owned();
            Said(
                Stage::Ready,
                Tone::Subtle,
                PillKind::Checking,
                headline,
                true,
            )
        }
        _ if is_auto => {
            let headline = "Merging as soon as GitHub allows".to_owned();
            Said(
                Stage::Ready,
                Tone::Suggestion,
                PillKind::AutoMerging,
                headline,
                true,
            )
        }
        _ => {
            let ready = if decision == Some(&ReviewDecision::Approved) {
                "Approved"
            } else {
                "Ready to merge"
            };
            let headline = if threads.is_empty() {
                "Ready to merge".to_owned()
            } else {
                format!("{ready}{also}")
            };
            still(Stage::Ready, Tone::Success, PillKind::Ready, headline)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept() -> Mergeable {
        Mergeable {
            methods: vec![MergeMethod::Squash],
            can_merge: true,
            can_auto: true,
            is_auto: false,
            auto_method: None,
            can_cancel_auto: false,
        }
    }

    fn open(merge_state_status: MergeStateStatus) -> GhPr {
        GhPr {
            state: PrState::Open,
            merge_state_status: Some(merge_state_status),
            ..GhPr::default()
        }
    }

    #[test]
    fn an_older_mods_entry_keeps_the_merge_row_it_had() {
        assert_eq!(merge_of(&open(MergeStateStatus::Clean), &kept()), kept());
        let behind = merge_of(&open(MergeStateStatus::Behind), &kept());
        assert_eq!(
            behind,
            Mergeable {
                can_merge: false,
                ..kept()
            }
        );
        let conflicted = merge_of(&open(MergeStateStatus::Dirty), &kept());
        assert!(!conflicted.can_merge && !conflicted.can_auto);
        assert_eq!(conflicted.methods, kept().methods);
        let draft = GhPr {
            is_draft: true,
            ..open(MergeStateStatus::Clean)
        };
        assert!(!merge_of(&draft, &kept()).can_merge);
    }

    #[test]
    fn its_own_merge_facts_or_a_finished_pr_ignore_what_was_kept() {
        let fresh = GhPr {
            repository: Some(RepoMerge::default()),
            ..open(MergeStateStatus::Clean)
        };
        assert_eq!(merge_of(&fresh, &kept()), mergeable_of(&fresh));
        let merged = GhPr {
            state: PrState::Merged,
            ..GhPr::default()
        };
        assert_eq!(merge_of(&merged, &kept()), Mergeable::default());
    }
}
