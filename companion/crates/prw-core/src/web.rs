//! `prw web`: the model as a browser page sees it, and the rules its server keeps.
//!
//! The server (the binary's `web`) holds a [`Model`] fed as the TUI's is, and answers the
//! page's polls with [`state_of`] it. Everything the page shows about a PR is worked out here
//! rather than in the page: which group it goes in ([`bucket_of`]), where it is on its way to
//! merging ([`stations_of`]) and what to do next ([`next_of`]). The page only lays it out and
//! works out ages from the timestamps.
//!
//! The server listens on 127.0.0.1 only and asks for a secret that it printed in the page's
//! link (in the fragment, so it reaches no log), sent back as a header ([`authorize`]): a page
//! another site serves can't read your PRs, nor change your lists, even from this machine.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::act::Tone as ToastTone;
use crate::batch::ref_from_url;
use crate::gh::{Conclusion, GhPr, PrState, ReviewDecision};
use crate::repos::hue_of;
use crate::review::{Reviewer, Verdict, reviews_of};
use crate::status::{PillKind, Progress, check_name, latest_runs, mergeable_of, progress_of};
use crate::ui::{ListId, Model, Msg, Poll, Row};
use crate::watchlist::Later;

/// What the page gets on each poll.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub version: String,
    /// The model's clock (ms): ages count from it, not the browser's.
    pub now: i64,
    pub poll: PollView,
    /// The gh logins PRs are yours by.
    pub logins: Vec<String>,
    /// Focus, the Inbox, My PRs, the saved searches, Later.
    pub lists: Vec<ListView>,
    pub toast: Option<ToastView>,
    /// A `prw` newer than this one, where GitHub said there is.
    pub newer: Option<String>,
}

/// Who keeps the results fresh, and how fresh they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PollView {
    pub by: Poll,
    pub words: &'static str,
    /// When the results' round ran (ms), where one has.
    pub fetched_at: Option<i64>,
    /// Live agent sessions.
    pub live: usize,
    pub is_read_failing: bool,
    /// Why `prw`'s own poller couldn't find a login's PRs, where it couldn't.
    pub error: Option<String>,
    /// What was wrong with `config.toml`.
    pub problems: Vec<String>,
}

/// A list, as the rail names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListView {
    /// `focus`, `inbox`, `mine`, `later`, or `search:<name>`.
    pub id: String,
    pub kind: ListKind,
    pub label: String,
    /// A saved search's query, how many PRs it matches in all, and why it last failed.
    pub query: Option<String>,
    pub total: Option<u64>,
    pub error: Option<String>,
    pub rows: Vec<RowView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ListKind {
    Focus,
    Inbox,
    Mine,
    Search,
    Later,
}

/// How many repo colours the page has (`--repo-0` and on, in each theme): as many as the TUI's
/// `REPO_HUES`, so a repo is the same colour in both.
pub const REPO_HUES: usize = 8;

/// One PR in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowView {
    pub url: String,
    pub number: u64,
    /// `owner/repo`, and the name the row shows for it.
    pub repo: String,
    pub repo_name: String,
    /// Its repo's chip colour, of [`REPO_HUES`]: the TUI's pick (`repos::hue_of`).
    pub hue: usize,
    pub title: String,
    /// Empty until read, and for a deleted account.
    pub author: String,
    /// Its author is one of your logins.
    pub is_yours: bool,
    pub agent: Option<String>,
    pub watchers: Vec<String>,
    /// The login it goes as, where there is more than one to choose from.
    pub login: Option<String>,
    pub branch: String,
    pub base: String,
    pub additions: u64,
    pub deletions: u64,
    pub opened_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub checked_at: Option<i64>,
    pub pill: PillKind,
    pub pill_text: &'static str,
    pub headline: String,
    pub is_moving: bool,
    pub error: Option<String>,
    pub bucket: Bucket,
    pub checks: Vec<CheckView>,
    pub reviewers: Vec<Reviewer>,
    pub unresolved: usize,
    /// `None` until it is read.
    pub stations: Option<Stations>,
    pub next: Next,
    pub is_focus: bool,
    /// How it was when put aside, while it is in Later.
    pub later: Option<Later>,
}

/// One check, the latest run of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckView {
    pub name: String,
    pub state: CheckState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    Fail,
    Run,
    Queue,
    /// Passed by being skipped: it may need starting by hand.
    Skip,
    Pass,
}

/// Which group a list shows a PR in, most pressing first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bucket {
    /// Yours to act on: changes asked, threads open or conflicts on your PR, or your review
    /// asked for.
    You,
    Failing,
    Ready,
    /// Something is under way: CI, GitHub working out whether it merges, auto-merge.
    Running,
    /// On someone else: a review, the base moving, a draft; or not read yet.
    Waiting,
    Done,
}

/// The group a PR goes in, by its pill. `is_yours`: you wrote it; `is_asked`: your review is
/// asked for. One match, so a pill added to [`PillKind`] has to be placed here.
#[must_use]
pub const fn bucket_of(pill: PillKind, is_yours: bool, is_asked: bool) -> Bucket {
    match pill {
        PillKind::Merged | PillKind::Closed => Bucket::Done,
        PillKind::Loading => Bucket::Waiting,
        _ if is_asked => Bucket::You,
        PillKind::Conflicts | PillKind::Changes | PillKind::Comments if is_yours => Bucket::You,
        PillKind::CiFailing | PillKind::CiFailed => Bucket::Failing,
        PillKind::Ready => Bucket::Ready,
        PillKind::CiRunning | PillKind::Checking | PillKind::AutoMerging => Bucket::Running,
        PillKind::Conflicts
        | PillKind::Changes
        | PillKind::Comments
        | PillKind::InReview
        | PillKind::Behind
        | PillKind::Blocked
        | PillKind::Draft => Bucket::Waiting,
    }
}

/// Where a station on the way to merging stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stop {
    Done,
    /// Being worked on, or the next to be.
    Now,
    Blocked,
    Todo,
    Merged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Station {
    pub stop: Stop,
    pub text: String,
}

/// The three stations every PR passes on its way in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Stations {
    pub checks: Station,
    pub review: Station,
    pub merge: Station,
}

/// What to do next about a PR, in a sentence, and what does it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Next {
    pub icon: Icon,
    pub title: String,
    pub text: String,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Icon {
    Reading,
    Merged,
    Closed,
    Conflict,
    Alert,
    Thread,
    Eye,
    Failed,
    Running,
    Ready,
    Waiting,
    Draft,
}

/// A button on the next step: GitHub's page for it, or a change to your lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    Link {
        label: String,
        url: String,
        is_primary: bool,
    },
    Unpin {
        label: String,
    },
}

impl Action {
    fn link(label: &str, url: String, is_primary: bool) -> Self {
        Self::Link {
            label: label.to_owned(),
            url,
            is_primary,
        }
    }
}

/// The toast the model shows: what a change did, or a PR back from Later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToastView {
    pub text: String,
    pub tone: ToastTone,
    /// When it goes (ms): with the text, which toast it is.
    pub until: i64,
}

/// The page's view of the model.
#[must_use]
pub fn state_of(model: &Model, version: &str) -> State {
    let logins = logins_of(model);
    let lists = model
        .lists()
        .iter()
        .map(|list| {
            let (id, kind) = match &list.id {
                ListId::Focus => ("focus".to_owned(), ListKind::Focus),
                ListId::Inbox => ("inbox".to_owned(), ListKind::Inbox),
                ListId::Session(_) => ("mine".to_owned(), ListKind::Mine),
                ListId::Search(name) => (format!("search:{name}"), ListKind::Search),
                ListId::Later => ("later".to_owned(), ListKind::Later),
            };
            let search = list.search.as_ref();
            ListView {
                id,
                kind,
                label: list.label.clone(),
                query: search.map(|s| s.query.clone()),
                total: search.map(|s| s.total),
                error: search.and_then(|s| s.error.clone()),
                rows: list
                    .rows
                    .iter()
                    .map(|row| row_view(model, row, &logins))
                    .collect(),
            }
        })
        .collect();
    let header = model.header();
    let poller = model.poller();
    State {
        version: version.to_owned(),
        now: model.now(),
        poll: PollView {
            by: header.poll,
            words: header.poll.words(),
            fetched_at: model.shared().map(|s| s.fetched_at).filter(|&at| at > 0),
            live: header.live,
            is_read_failing: header.is_read_failing,
            error: poller.and_then(|p| p.error.clone()),
            problems: poller.map(|p| p.problems.clone()).unwrap_or_default(),
        },
        logins: logins.into_iter().collect(),
        lists,
        toast: model.acts().toast().map(|t| ToastView {
            text: t.text.clone(),
            tone: t.tone,
            until: t.until,
        }),
        newer: model.newer().map(str::to_owned),
    }
}

/// gh's logins as the poller last read them; before it has (or with no poller, as in the
/// demo), the logins the results say see each owner's repos.
fn logins_of(model: &Model) -> BTreeSet<String> {
    let polled: BTreeSet<String> = model
        .poller()
        .map(|p| {
            std::iter::once(p.logins.active.clone())
                .chain(p.logins.others.iter().cloned())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if !polled.is_empty() {
        return polled;
    }
    model
        .shared()
        .map(|s| s.accounts.values().cloned().collect())
        .unwrap_or_default()
}

fn row_view(model: &Model, row: &Row, logins: &BTreeSet<String>) -> RowView {
    let pr = model.entry(&row.pr.url).and_then(|e| e.pr.as_ref());
    let derived = &row.pr.derived;
    let author = pr
        .and_then(|p| p.author.as_ref())
        .map(|a| a.login.clone())
        .unwrap_or_default();
    let is_yours = logins.contains(&author);
    let reviews = pr.and_then(reviews_of).unwrap_or_default();
    let is_asked = !is_yours
        && reviews
            .reviewers
            .iter()
            .any(|r| r.verdict == Verdict::Requested && logins.contains(&r.login));
    let checks = pr.map(checks_of).unwrap_or_default();
    let watchlist = model.watchlist();
    let is_focus = watchlist.is_focus(&row.pr.url);
    let facts = Facts {
        url: &row.pr.url,
        pill: derived.pill,
        pr,
        author: &author,
        is_yours,
        is_asked,
        is_focus,
        checks: &checks,
        reviewers: &reviews.reviewers,
        unresolved: reviews.unresolved,
    };
    RowView {
        url: row.pr.url.clone(),
        number: row.pr.number,
        repo: row.repo.clone(),
        repo_name: row.repo_name.clone(),
        hue: hue_of(&row.repo, REPO_HUES),
        title: row.pr.title.clone(),
        is_yours,
        agent: row.agent.clone(),
        watchers: row.watchers.clone(),
        login: model.login_of(row),
        branch: pr.map(|p| p.head_ref_name.clone()).unwrap_or_default(),
        base: pr.map(|p| p.base_ref_name.clone()).unwrap_or_default(),
        additions: pr.map_or(0, |p| p.additions),
        deletions: pr.map_or(0, |p| p.deletions),
        opened_at: row.opened_at,
        updated_at: row.updated_at,
        checked_at: row.checked_at,
        pill: derived.pill,
        pill_text: derived.pill.text(),
        headline: derived.headline.clone(),
        is_moving: derived.is_moving,
        error: row.pr.error.clone(),
        bucket: bucket_of(derived.pill, is_yours, is_asked),
        stations: stations_of(&facts),
        next: next_of(&facts),
        checks,
        reviewers: reviews.reviewers.clone(),
        unresolved: reviews.unresolved,
        is_focus,
        later: watchlist
            .later
            .iter()
            .find(|l| l.url == row.pr.url)
            .cloned(),
        author,
    }
}

/// The latest run of each check, as the page marks it.
fn checks_of(pr: &GhPr) -> Vec<CheckView> {
    latest_runs(pr.checks())
        .into_iter()
        .map(|item| CheckView {
            name: check_name(item),
            state: match progress_of(item) {
                Progress::Failed => CheckState::Fail,
                Progress::Running => CheckState::Run,
                Progress::Queued => CheckState::Queue,
                Progress::Passed if item.conclusion == Some(Conclusion::Skipped) => {
                    CheckState::Skip
                }
                Progress::Passed => CheckState::Pass,
            },
        })
        .collect()
}

/// What the stations and the next step are worked out from.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub url: &'a str,
    pub pill: PillKind,
    /// `None` until read.
    pub pr: Option<&'a GhPr>,
    pub author: &'a str,
    pub is_yours: bool,
    pub is_asked: bool,
    pub is_focus: bool,
    pub checks: &'a [CheckView],
    pub reviewers: &'a [Reviewer],
    pub unresolved: usize,
}

impl Facts<'_> {
    fn count(&self, state: CheckState) -> usize {
        self.checks.iter().filter(|c| c.state == state).count()
    }

    fn failing(&self) -> Vec<&str> {
        self.checks
            .iter()
            .filter(|c| c.state == CheckState::Fail)
            .map(|c| c.name.as_str())
            .collect()
    }

    /// Checks not finished: running or queued.
    fn going(&self) -> usize {
        self.count(CheckState::Run) + self.count(CheckState::Queue)
    }

    fn first(&self, verdict: Verdict) -> Option<&str> {
        self.reviewers
            .iter()
            .find(|r| r.verdict == verdict)
            .map(|r| r.login.as_str())
    }

    fn base(&self) -> &str {
        self.pr.map_or("", |p| p.base_ref_name.as_str())
    }
}

/// "1 check", "2 checks".
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Checks, review and merge, each done, under way, blocked or still to come. `None` until
/// the PR is read.
#[must_use]
pub fn stations_of(facts: &Facts) -> Option<Stations> {
    let pr = facts.pr?;
    let station = |stop, text: &str| Station {
        stop,
        text: text.to_owned(),
    };
    let is_merged = pr.state == PrState::Merged;
    let is_closed = pr.state == PrState::Closed;
    let (total, failed, going, skipped) = (
        facts.checks.len(),
        facts.count(CheckState::Fail),
        facts.going(),
        facts.count(CheckState::Skip),
    );
    let checks = if total == 0 {
        station(Stop::Done, "none")
    } else if failed > 0 && !is_merged {
        station(Stop::Blocked, &format!("{failed} failed"))
    } else if going > 0 && !is_merged {
        let passed = total - failed - going;
        let doing = match facts.count(CheckState::Run) {
            0 => format!("{going} queued"),
            running => format!("{running} running"),
        };
        station(Stop::Now, &format!("{passed} of {total} · {doing}"))
    } else if skipped > 0 {
        station(Stop::Done, &format!("passed · {skipped} skipped"))
    } else {
        station(Stop::Done, &format!("{total} of {total} passed"))
    };
    let decision = pr.review_decision.as_ref();
    let review = if is_merged || decision == Some(&ReviewDecision::Approved) {
        match facts.first(Verdict::Approved) {
            Some(who) => station(Stop::Done, &format!("{who} approved")),
            None => station(Stop::Done, "approved"),
        }
    } else if decision == Some(&ReviewDecision::ChangesRequested) {
        station(Stop::Blocked, "changes asked")
    } else if pr.is_draft {
        station(Stop::Todo, "after the draft")
    } else if decision.is_none() {
        station(Stop::Done, "not required")
    } else if facts.is_asked {
        station(Stop::Now, "yours to give")
    } else {
        station(Stop::Now, "required")
    };
    let mergeable = mergeable_of(pr);
    let merge = if is_merged {
        station(Stop::Merged, "merged")
    } else if is_closed {
        station(Stop::Todo, "closed")
    } else if facts.pill == PillKind::Conflicts {
        station(Stop::Blocked, "conflicts")
    } else if mergeable.is_auto {
        station(Stop::Now, "auto-merge on")
    } else if facts.pill == PillKind::Ready {
        station(Stop::Now, "ready")
    } else if facts.pill == PillKind::Behind {
        station(Stop::Blocked, "behind")
    } else {
        station(Stop::Todo, "not yet")
    };
    Some(Stations {
        checks,
        review,
        merge,
    })
}

/// What to do next, and the GitHub page that does it (actions from the page come later).
#[must_use]
pub fn next_of(facts: &Facts) -> Next {
    let url = facts.url;
    if facts.is_asked && !matches!(facts.pill, PillKind::Merged | PillKind::Closed) {
        return review_asked(facts);
    }
    let base = facts.base();
    match facts.pill {
        PillKind::Loading => Next::new(
            Icon::Reading,
            "Reading from GitHub",
            "The first read of this PR.",
            Vec::new(),
        ),
        PillKind::Merged => {
            let text = if base.is_empty() {
                "Merged.".to_owned()
            } else {
                format!("Into {base}.")
            };
            Next::new(Icon::Merged, "Merged", &text, unpin(facts))
        }
        PillKind::Closed => Next::new(
            Icon::Closed,
            "Closed",
            "Closed without merging.",
            unpin(facts),
        ),
        PillKind::Conflicts => conflicts(facts),
        PillKind::Changes => {
            let who = facts.first(Verdict::Changes).unwrap_or("A reviewer");
            let text = match facts.unresolved {
                0 => "See their review.".to_owned(),
                n => format!("{} open.", plural(n, "thread")),
            };
            let see = Action::link("See the review", format!("{url}/files"), true);
            Next::new(
                Icon::Alert,
                &format!("{who} asked for changes"),
                &text,
                vec![see],
            )
        }
        PillKind::Comments => Next::new(
            Icon::Thread,
            &format!("{} open", plural(facts.unresolved.max(1), "thread")),
            "Reply to them, or resolve them.",
            vec![Action::link(
                "See the threads",
                format!("{url}/files"),
                true,
            )],
        ),
        PillKind::CiFailing | PillKind::CiFailed => failed(facts),
        PillKind::AutoMerging | PillKind::CiRunning | PillKind::Checking => under_way(facts),
        PillKind::Ready => {
            let skipped = facts.count(CheckState::Skip);
            let text = if skipped > 0 {
                format!("Approved and green. {} skipped.", plural(skipped, "check"))
            } else {
                "Approved, and every required check passed.".to_owned()
            };
            let merge = Action::link("Merge on GitHub", url.to_owned(), true);
            Next::new(Icon::Ready, "Ready to merge", &text, vec![merge])
        }
        PillKind::Behind => Next::new(
            Icon::Waiting,
            &format!("Behind {base}"),
            "Update the branch, and it can merge.",
            vec![on_github(url, true)],
        ),
        PillKind::Blocked => Next::new(
            Icon::Waiting,
            "Blocked",
            "A rule of the branch holds it: GitHub says which.",
            vec![on_github(url, false)],
        ),
        PillKind::Draft => Next::new(
            Icon::Draft,
            "Draft",
            "Mark it ready for review once it is.",
            vec![on_github(url, false)],
        ),
        PillKind::InReview => in_review(facts),
    }
}

impl Next {
    fn new(icon: Icon, title: &str, text: &str, actions: Vec<Action>) -> Self {
        Self {
            icon,
            title: title.to_owned(),
            text: text.to_owned(),
            actions,
        }
    }
}

fn on_github(url: &str, is_primary: bool) -> Action {
    Action::link("Open on GitHub", url.to_owned(), is_primary)
}

/// Unpinning, for a PR in Focus that is over.
fn unpin(facts: &Facts) -> Vec<Action> {
    let label = "Unpin from Focus".to_owned();
    facts
        .is_focus
        .then_some(Action::Unpin { label })
        .into_iter()
        .collect()
}

fn review_asked(facts: &Facts) -> Next {
    let author = if facts.author.is_empty() {
        "Someone"
    } else {
        facts.author
    };
    let (adds, dels) = facts.pr.map_or((0, 0), |p| (p.additions, p.deletions));
    let checks = match (facts.failing().len(), facts.going()) {
        (0, 0) => "Checks passed.".to_owned(),
        (0, going) => format!("{} still going.", plural(going, "check")),
        (failed, _) => format!("{} failed.", plural(failed, "check")),
    };
    let review = Action::link("Review on GitHub", format!("{}/files", facts.url), true);
    Next::new(
        Icon::Eye,
        &format!("{author} asked for your review"),
        &format!("+{adds} −{dels}. {checks}"),
        vec![review],
    )
}

fn conflicts(facts: &Facts) -> Next {
    let failing = facts.failing();
    let then = if failing.is_empty() {
        "CI runs again after.".to_owned()
    } else {
        format!("Also {} failed.", failing.join(", "))
    };
    let branch = facts.pr.map_or("", |p| p.head_ref_name.as_str());
    let resolve = Action::link(
        "Resolve on GitHub",
        format!("{}/conflicts", facts.url),
        true,
    );
    Next::new(
        Icon::Conflict,
        &format!("Conflicts with {}", facts.base()),
        &format!("Resolve them on {branch}. {then}"),
        vec![resolve],
    )
}

fn failed(facts: &Facts) -> Next {
    let failing = facts.failing();
    let still = match facts.going() {
        0 => String::new(),
        n => format!(" {} still going.", plural(n, "check")),
    };
    let checks = Action::link("See the checks", format!("{}/checks", facts.url), true);
    Next::new(
        Icon::Failed,
        &format!("{} failed", plural(failing.len().max(1), "check")),
        &format!("{}.{still}", failing.join(", ")),
        vec![checks],
    )
}

/// CI running, auto-merge waiting on it, or GitHub working out whether it merges.
fn under_way(facts: &Facts) -> Next {
    let going = facts.going();
    match facts.pill {
        PillKind::AutoMerging => {
            let text = match going {
                0 => "It lands once it can.".to_owned(),
                1 => "It lands once 1 check finishes.".to_owned(),
                n => format!("It lands once {n} checks finish."),
            };
            let link = on_github(facts.url, false);
            Next::new(Icon::Running, "Auto-merge is on", &text, vec![link])
        }
        PillKind::Checking => Next::new(
            Icon::Running,
            "GitHub is checking it",
            "Whether it merges cleanly, in a moment.",
            vec![on_github(facts.url, false)],
        ),
        _ => {
            let is_review_due = facts
                .pr
                .is_some_and(|p| p.review_decision == Some(ReviewDecision::ReviewRequired));
            let text = if is_review_due {
                "A review is still required after them."
            } else {
                "Then it can merge."
            };
            let checks = Action::link("Watch the checks", format!("{}/checks", facts.url), false);
            Next::new(
                Icon::Running,
                &format!("{} still going", plural(going.max(1), "check")),
                text,
                vec![checks],
            )
        }
    }
}

fn in_review(facts: &Facts) -> Next {
    let waiting: Vec<&str> = facts
        .reviewers
        .iter()
        .filter(|r| r.verdict == Verdict::Requested)
        .map(|r| r.login.as_str())
        .collect();
    let title = match waiting.as_slice() {
        [] => "Waiting on a review".to_owned(),
        [who] => format!("Waiting on {who}"),
        [who, rest @ ..] => format!("Waiting on {who} +{}", rest.len()),
    };
    let text = if facts.is_yours {
        "Nothing for you to do yet."
    } else {
        "Nothing asked of you."
    };
    Next::new(
        Icon::Waiting,
        &title,
        text,
        vec![on_github(facts.url, false)],
    )
}

// ---- The server's rules -------------------------------------------------------------------

/// What a request asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A file of the page, by its path in the build (`index.html`, `assets/…`).
    Asset(String),
    /// `GET /api/state`.
    State,
    /// `POST /api/lists`: a PR pinned or put aside.
    Lists,
    NotFound,
    /// A path there is, asked with the wrong method.
    Method,
}

/// The route of a request line's method and target (its query, if any, is dropped).
#[must_use]
pub fn route(method: &str, target: &str) -> Route {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    match (method, path) {
        ("GET", "/api/state") => Route::State,
        ("POST", "/api/lists") => Route::Lists,
        (_, "/api/state" | "/api/lists") => Route::Method,
        (_, p) if p.starts_with("/api/") || p == "/api" => Route::NotFound,
        ("GET" | "HEAD", "/") => Route::Asset("index.html".to_owned()),
        ("GET" | "HEAD", p) => match p.strip_prefix('/') {
            Some(file) if !file.is_empty() && !file.split('/').any(|s| s == "..") => {
                Route::Asset(file.to_owned())
            }
            _ => Route::NotFound,
        },
        _ => Route::Method,
    }
}

/// The parts of a request [`authorize`] looks at.
#[derive(Debug, Clone, Copy, Default)]
pub struct Asked<'a> {
    pub host: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub content_type: Option<&'a str>,
    /// `X-Prw-Key`.
    pub key: Option<&'a str>,
}

/// Why a request was turned away, as its status says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    /// Not addressed to this server by name: a DNS rebinding, say (403).
    Host,
    /// The API without the secret (401).
    Key,
    /// A change sent from another origin (403).
    Origin,
    /// A change that isn't JSON, so a plain form can't send one (415).
    Type,
}

impl Denied {
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::Host | Self::Origin => 403,
            Self::Key => 401,
            Self::Type => 415,
        }
    }
}

/// Whether to answer a request. Every request must name this server as its host; the API
/// asks for the secret; a change must come from the page's own origin, as JSON.
///
/// # Errors
///
/// Why not, where it is turned away.
pub fn authorize(route: &Route, asked: &Asked, port: u16, secret: &str) -> Result<(), Denied> {
    let hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let is_ours = |host: &str| hosts.iter().any(|h| h.eq_ignore_ascii_case(host));
    if !asked.host.is_some_and(is_ours) {
        return Err(Denied::Host);
    }
    if matches!(route, Route::State | Route::Lists)
        && !asked.key.is_some_and(|key| same_secret(key, secret))
    {
        return Err(Denied::Key);
    }
    if *route == Route::Lists {
        let origin = asked.origin.and_then(|o| o.strip_prefix("http://"));
        if !origin.is_some_and(is_ours) {
            return Err(Denied::Origin);
        }
        let mime = asked.content_type.and_then(|t| t.split(';').next());
        if !mime.is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json")) {
            return Err(Denied::Type);
        }
    }
    Ok(())
}

/// Whether a key is the secret, in a time that doesn't tell how much of it matched.
#[must_use]
pub fn same_secret(key: &str, secret: &str) -> bool {
    let (key, secret) = (key.as_bytes(), secret.as_bytes());
    !secret.is_empty()
        && key.len() == secret.len()
        && key
            .iter()
            .zip(secret)
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// A change `POST /api/lists` sends: one PR, and where it should be.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    url: String,
    focus: Option<bool>,
    later: Option<bool>,
}

/// The message a `POST /api/lists` body asks the model for: `{"url", "focus": bool}` pins or
/// unpins, `{"url", "later": bool}` puts aside or brings back. `None` for anything else.
#[must_use]
pub fn change_of(body: &str) -> Option<Msg> {
    let change: Change = serde_json::from_str(body).ok()?;
    ref_from_url(&change.url)?;
    match (change.focus, change.later) {
        (Some(is_on), None) => Some(Msg::Pin(change.url, is_on)),
        (None, Some(is_on)) => Some(Msg::Aside(change.url, is_on)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "5f2c";

    fn asked<'a>(host: &'a str, key: Option<&'a str>) -> Asked<'a> {
        Asked {
            host: Some(host),
            key,
            ..Asked::default()
        }
    }

    #[test]
    fn routes_the_page_the_api_and_nothing_else() {
        assert_eq!(route("GET", "/"), Route::Asset("index.html".into()));
        assert_eq!(route("GET", "/?x=1"), Route::Asset("index.html".into()));
        assert_eq!(
            route("GET", "/assets/index-3f.js"),
            Route::Asset("assets/index-3f.js".into())
        );
        assert_eq!(route("GET", "/api/state"), Route::State);
        assert_eq!(route("POST", "/api/lists"), Route::Lists);
        assert_eq!(route("POST", "/api/state"), Route::Method);
        assert_eq!(route("GET", "/api/lists"), Route::Method);
        assert_eq!(route("GET", "/api/other"), Route::NotFound);
        assert_eq!(route("GET", "/assets/../secret"), Route::NotFound);
        assert_eq!(route("DELETE", "/"), Route::Method);
    }

    #[test]
    fn the_api_asks_for_the_host_and_the_secret() {
        let state = Route::State;
        let page = Route::Asset("index.html".into());
        assert_eq!(
            authorize(&page, &asked("127.0.0.1:7000", None), 7000, SECRET),
            Ok(())
        );
        assert_eq!(
            authorize(&page, &asked("evil.example:7000", None), 7000, SECRET),
            Err(Denied::Host)
        );
        assert_eq!(
            authorize(&state, &asked("localhost:7000", None), 7000, SECRET),
            Err(Denied::Key)
        );
        assert_eq!(
            authorize(&state, &asked("localhost:7000", Some("5f2d")), 7000, SECRET),
            Err(Denied::Key)
        );
        assert_eq!(
            authorize(&state, &asked("LOCALHOST:7000", Some(SECRET)), 7000, SECRET),
            Ok(())
        );
        assert_eq!(
            authorize(&state, &asked("127.0.0.1:7001", Some(SECRET)), 7000, SECRET),
            Err(Denied::Host)
        );
    }

    #[test]
    fn a_change_comes_from_the_page_as_json() {
        let lists = Route::Lists;
        let from = |origin, content_type| Asked {
            host: Some("127.0.0.1:7000"),
            key: Some(SECRET),
            origin,
            content_type,
        };
        let json = Some("application/json; charset=utf-8");
        assert_eq!(
            authorize(
                &lists,
                &from(Some("http://127.0.0.1:7000"), json),
                7000,
                SECRET
            ),
            Ok(())
        );
        assert_eq!(
            authorize(&lists, &from(None, json), 7000, SECRET),
            Err(Denied::Origin)
        );
        assert_eq!(
            authorize(
                &lists,
                &from(Some("https://evil.example"), json),
                7000,
                SECRET
            ),
            Err(Denied::Origin)
        );
        let form = Some("application/x-www-form-urlencoded");
        assert_eq!(
            authorize(
                &lists,
                &from(Some("http://localhost:7000"), form),
                7000,
                SECRET
            ),
            Err(Denied::Type)
        );
    }

    #[test]
    fn an_empty_secret_lets_nothing_in() {
        assert!(!same_secret("", ""));
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abcd"));
    }

    #[test]
    fn reads_a_change() {
        let url = "https://github.com/acme/app/pull/1";
        assert_eq!(
            change_of(&format!(r#"{{"url":"{url}","focus":true}}"#)),
            Some(Msg::Pin(url.into(), true))
        );
        assert_eq!(
            change_of(&format!(r#"{{"url":"{url}","later":false}}"#)),
            Some(Msg::Aside(url.into(), false))
        );
        assert_eq!(
            change_of(&format!(r#"{{"url":"{url}","focus":true,"later":true}}"#)),
            None
        );
        assert_eq!(change_of(&format!(r#"{{"url":"{url}"}}"#)), None);
        assert_eq!(
            change_of(r#"{"url":"https://evil.example","focus":true}"#),
            None
        );
        assert_eq!(change_of("not json"), None);
    }
}
