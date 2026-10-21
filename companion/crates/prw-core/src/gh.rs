//! A PR as `results.json` stores it (`prs[url].pr`): GitHub's answer to the mod's batched
//! GraphQL query, as `parseReply` (`hooks/batch.ts`) leaves it. The types are `GhPr`,
//! `RepoMerge`, `CheckItem` (`hooks/pr.ts`) and `ReviewFacts` (`hooks/review.ts`).
//!
//! Every field reads leniently (see the crate's `lenient` module): missing, `null` or of
//! the wrong type, it is absent. GitHub's enum values are Rust enums with an `Other`
//! escape, so a value GitHub adds reads as `Other` (which the status logic treats as the
//! TypeScript treats an unknown string: as none of the values it checks for).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::lenient::{lenient, lenient_vec, lenient_vec_opt, string_enum};

string_enum! {
    /// `PullRequestState`.
    pub enum PrState {
        Open = "OPEN",
        Merged = "MERGED",
        Closed = "CLOSED",
    }
}

string_enum! {
    /// `PullRequestReviewDecision`.
    pub enum ReviewDecision {
        Approved = "APPROVED",
        ChangesRequested = "CHANGES_REQUESTED",
        ReviewRequired = "REVIEW_REQUIRED",
    }
}

string_enum! {
    /// `MergeStateStatus`: whether GitHub would merge the head now, and if not, why.
    pub enum MergeStateStatus {
        Behind = "BEHIND",
        Blocked = "BLOCKED",
        Clean = "CLEAN",
        Dirty = "DIRTY",
        Draft = "DRAFT",
        HasHooks = "HAS_HOOKS",
        Unknown = "UNKNOWN",
        Unstable = "UNSTABLE",
    }
}

string_enum! {
    /// `MergeableState`: the direct conflict signal, set while `DIRTY` is still being computed.
    pub enum MergeableState {
        Mergeable = "MERGEABLE",
        Conflicting = "CONFLICTING",
        Unknown = "UNKNOWN",
    }
}

string_enum! {
    /// `PullRequestMergeMethod`, as GitHub spells it (`hooks/pr.ts` lowercases it: `asMethod`).
    pub enum GhMergeMethod {
        Merge = "MERGE",
        Squash = "SQUASH",
        Rebase = "REBASE",
    }
}

string_enum! {
    /// `RepositoryPermission`: merging takes `WRITE` or more.
    pub enum Permission {
        Read = "READ",
        Triage = "TRIAGE",
        Write = "WRITE",
        Maintain = "MAINTAIN",
        Admin = "ADMIN",
    }
}

string_enum! {
    /// A rollup item's `__typename`.
    pub enum CheckKind {
        CheckRun = "CheckRun",
        StatusContext = "StatusContext",
    }
}

string_enum! {
    /// `CheckStatusState`: where a check run is.
    pub enum CheckStatus {
        Requested = "REQUESTED",
        Queued = "QUEUED",
        InProgress = "IN_PROGRESS",
        Completed = "COMPLETED",
        Waiting = "WAITING",
        Pending = "PENDING",
    }
}

string_enum! {
    /// `CheckConclusionState`: how a completed check run ended.
    pub enum Conclusion {
        ActionRequired = "ACTION_REQUIRED",
        Cancelled = "CANCELLED",
        Failure = "FAILURE",
        Neutral = "NEUTRAL",
        Skipped = "SKIPPED",
        Stale = "STALE",
        StartupFailure = "STARTUP_FAILURE",
        Success = "SUCCESS",
        TimedOut = "TIMED_OUT",
    }
}

string_enum! {
    /// `StatusState`: a commit status context's state.
    pub enum StatusState {
        Error = "ERROR",
        Expected = "EXPECTED",
        Failure = "FAILURE",
        Pending = "PENDING",
        Success = "SUCCESS",
    }
}

string_enum! {
    /// `PullRequestReviewState`.
    pub enum ReviewState {
        Approved = "APPROVED",
        ChangesRequested = "CHANGES_REQUESTED",
        Commented = "COMMENTED",
        Dismissed = "DISMISSED",
        Pending = "PENDING",
    }
}

string_enum! {
    /// What kind of account an actor is (its `__typename`).
    pub enum ActorKind {
        User = "User",
        Bot = "Bot",
        Team = "Team",
        Mannequin = "Mannequin",
        Organization = "Organization",
    }
}

/// A PR as the poll stores it (`GhPr`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GhPr {
    #[serde(deserialize_with = "lenient")]
    pub number: u64,
    #[serde(deserialize_with = "lenient")]
    pub title: String,
    #[serde(deserialize_with = "lenient")]
    pub url: String,
    /// `Other("")` where the entry has none: neither open, merged nor closed.
    #[serde(deserialize_with = "lenient")]
    pub state: PrState,
    #[serde(deserialize_with = "lenient")]
    pub is_draft: bool,
    #[serde(deserialize_with = "lenient")]
    pub review_decision: Option<ReviewDecision>,
    #[serde(deserialize_with = "lenient")]
    pub merge_state_status: Option<MergeStateStatus>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<MergeableState>,
    /// Present (whatever its fields say) while auto-merge is on.
    #[serde(deserialize_with = "lenient")]
    pub auto_merge_request: Option<AutoMergeRequest>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub viewer_can_enable_auto_merge: Option<bool>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub viewer_can_disable_auto_merge: Option<bool>,
    /// The repository's merge settings; absent in an entry an older mod wrote.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub repository: Option<RepoMerge>,
    /// The head commit's checks, flattened; `null` where none has reported (as GitHub says).
    #[serde(deserialize_with = "lenient_vec_opt")]
    pub status_check_rollup: Option<Vec<CheckItem>>,
    #[serde(deserialize_with = "lenient")]
    pub merged_at: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub additions: u64,
    #[serde(deserialize_with = "lenient")]
    pub deletions: u64,
    #[serde(deserialize_with = "lenient")]
    pub head_ref_name: String,
    #[serde(deserialize_with = "lenient")]
    pub base_ref_name: String,
    /// `null` for a deleted account.
    #[serde(deserialize_with = "lenient")]
    pub author: Option<Author>,
    /// Each reviewer's last review of any kind (`ReviewFacts`); absent in an older mod's entry.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub latest_reviews: Option<Connection<GhReview>>,
    /// Each reviewer's last approval or change request.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub latest_opinionated_reviews: Option<Connection<GhReview>>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub review_requests: Option<Connection<ReviewRequest>>,
    /// The newest threads, enough to count; never their comments.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub review_threads: Option<Connection<ThreadState>>,
}

impl GhPr {
    /// The checks, none where GitHub said `null`.
    #[must_use]
    pub fn checks(&self) -> &[CheckItem] {
        self.status_check_rollup.as_deref().unwrap_or_default()
    }
}

/// A PR's author.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Author {
    #[serde(deserialize_with = "lenient")]
    pub login: String,
}

/// Auto-merge, while it is on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoMergeRequest {
    #[serde(deserialize_with = "lenient")]
    pub enabled_at: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub merge_method: Option<GhMergeMethod>,
}

/// The repository's merge settings, as the viewer sees them (`RepoMerge`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RepoMerge {
    #[serde(deserialize_with = "lenient")]
    pub squash_merge_allowed: bool,
    #[serde(deserialize_with = "lenient")]
    pub rebase_merge_allowed: bool,
    #[serde(deserialize_with = "lenient")]
    pub merge_commit_allowed: bool,
    #[serde(deserialize_with = "lenient")]
    pub viewer_default_merge_method: Option<GhMergeMethod>,
    #[serde(deserialize_with = "lenient")]
    pub viewer_permission: Option<Permission>,
}

/// One item of the head commit's rollup: a check run (with its workflow's name lifted out of
/// its suite) or a commit status context (`CheckItem`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheckItem {
    #[serde(
        rename = "__typename",
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub typename: Option<CheckKind>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub status: Option<CheckStatus>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<Conclusion>,
    /// Whether the item has a `state` at all (the outer `Option`), and what it says. The
    /// TypeScript takes any item with a `state` key, even `null`, for a status context
    /// (`isStatusContext`: `item.state !== undefined`), so presence is kept apart.
    #[serde(deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub state: Option<Option<StatusState>>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    /// Asked only when a failure is handed off (`fix.ts`), never by the poll.
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub database_id: Option<i64>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub details_url: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub target_url: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Called only where the key is present: `Some`, with the state where it is a string.
#[expect(
    clippy::option_option,
    reason = "a `state` key's presence means something apart from its value"
)]
fn present<'de, D>(d: D) -> Result<Option<Option<StatusState>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(match Value::deserialize(d)? {
        Value::String(state) => Some(StatusState::from(state)),
        _ => None,
    }))
}

/// A GraphQL connection as the poll asks for it: a page of nodes, and how many there are in
/// all where it asked (`latestOpinionatedReviews` doesn't).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[serde(bound(deserialize = "T: DeserializeOwned + Default"))]
pub struct Connection<T> {
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
    #[serde(deserialize_with = "lenient_vec")]
    pub nodes: Vec<T>,
}

impl<T> Default for Connection<T> {
    fn default() -> Self {
        Self {
            total_count: None,
            nodes: Vec::new(),
        }
    }
}

/// A user, bot, team or other account, as far as the query asks: `login`, or a team's `slug`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Actor {
    #[serde(
        rename = "__typename",
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub typename: Option<ActorKind>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
}

/// A review (`GhReview`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GhReview {
    /// `null` for a deleted account.
    #[serde(deserialize_with = "lenient")]
    pub author: Option<Actor>,
    #[serde(deserialize_with = "lenient")]
    pub state: ReviewState,
    #[serde(deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<String>,
}

/// A review asked of someone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReviewRequest {
    /// `null` where GitHub can't say who (a deleted account).
    #[serde(deserialize_with = "lenient")]
    pub requested_reviewer: Option<Actor>,
}

/// What the poll knows of a review thread: never its comments.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThreadState {
    #[serde(deserialize_with = "lenient")]
    pub is_resolved: bool,
    #[serde(deserialize_with = "lenient")]
    pub is_outdated: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_new_github_value_reads_as_other_and_writes_back() {
        let pr: GhPr = serde_json::from_value(
            json!({"state": "QUEUED_FOR_MERGE", "mergeStateStatus": "CLEAN"}),
        )
        .unwrap();
        assert_eq!(pr.state, PrState::Other("QUEUED_FOR_MERGE".into()));
        assert_eq!(pr.merge_state_status, Some(MergeStateStatus::Clean));
        assert_eq!(
            serde_json::to_value(&pr).unwrap()["state"],
            "QUEUED_FOR_MERGE"
        );
    }

    #[test]
    fn a_field_of_the_wrong_type_is_absent_not_a_failed_parse() {
        let pr: GhPr = serde_json::from_value(json!({
            "number": "12", "isDraft": 1, "author": "octocat", "additions": -3,
            "statusCheckRollup": [5, {"name": "build", "status": 7}],
            "reviewThreads": {"nodes": "none", "totalCount": 3},
        }))
        .unwrap();
        assert_eq!(pr.number, 0);
        assert!(!pr.is_draft);
        assert_eq!(pr.author, None);
        assert_eq!(pr.additions, 0);
        let checks = pr.status_check_rollup.unwrap();
        assert_eq!(checks[0], CheckItem::default());
        assert_eq!(checks[1].name.as_deref(), Some("build"));
        assert_eq!(checks[1].status, None);
        let threads = pr.review_threads.unwrap();
        assert_eq!(threads.nodes, Vec::new());
        assert_eq!(threads.total_count, Some(3));
    }

    #[test]
    fn a_state_key_is_kept_apart_from_its_value() {
        let items: Vec<CheckItem> =
            serde_json::from_value(json!([{"name": "a"}, {"state": null}, {"state": "PENDING"}]))
                .unwrap();
        assert_eq!(items[0].state, None);
        assert_eq!(items[1].state, Some(None));
        assert_eq!(items[2].state, Some(Some(StatusState::Pending)));
        let back: Vec<CheckItem> =
            serde_json::from_value(serde_json::to_value(&items).unwrap()).unwrap();
        assert_eq!(back, items);
    }
}
