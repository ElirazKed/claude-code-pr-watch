//! The full view's state: the PR open, the tab showing, what is selected in each tab, and what
//! its reads came to. A read is a [`Read`] for the binary to run off the UI thread; what came
//! of it comes back as a [`ReadDone`]. No I/O and no clock here, as in [`ui`](crate::ui).

use std::collections::BTreeMap;

use crate::address::{GhThread, Login};
use crate::batch::ref_from_url;
use crate::fix::{LOG_CHARS, LOG_LINES, Trimmed, failing_checks, to_fetch, trim_log};
use crate::full::{Full, Item, REFRESH_MS, checks_in_order, timeline};
use crate::gh::CheckItem;
use crate::status::check_name;

/// A tab of the full view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Tab {
    /// The description, comments, reviews and threads.
    #[default]
    Conversation,
    Checks,
    Files,
}

impl Tab {
    /// In the order the tab row shows them, keys `1` to `3`.
    pub const ALL: [Self; 3] = [Self::Conversation, Self::Checks, Self::Files];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Conversation => "Conversation",
            Self::Checks => "Checks",
            Self::Files => "Files",
        }
    }

    /// Its key: `1`, `2` or `3`.
    #[must_use]
    pub const fn key(self) -> char {
        match self {
            Self::Conversation => '1',
            Self::Checks => '2',
            Self::Files => '3',
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Conversation => 0,
            Self::Checks => 1,
            Self::Files => 2,
        }
    }

    /// The tab a key shows, where it is one's.
    #[must_use]
    pub fn of_key(key: char) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.key() == key)
    }

    /// The tab `by` along (round the end).
    #[must_use]
    pub const fn along(self, by: isize) -> Self {
        Self::ALL[(self.index().cast_signed() + by.rem_euclid(3))
            .rem_euclid(3)
            .unsigned_abs()]
    }
}

/// A read for the binary's worker: never on the UI thread, as a log may take a minute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// The PR whole ([`full_query`](crate::full::full_query)).
    Pr {
        url: String,
        repo: String,
        number: u64,
    },
    /// Failed jobs' logs, of `repo` (`log_argv`).
    Logs {
        url: String,
        repo: String,
        jobs: Vec<i64>,
    },
}

impl Read {
    /// The PR it reads.
    #[must_use]
    pub fn url(&self) -> &str {
        match self {
            Self::Pr { url, .. } | Self::Logs { url, .. } => url,
        }
    }
}

/// What came of a [`Read`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadDone {
    pub url: String,
    pub got: Got,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "one a read, moved once: boxing the PR buys nothing"
)]
pub enum Got {
    /// The PR, or why it didn't read.
    Pr(Result<Full, String>),
    /// Each job's log as gh printed it, or why it didn't come.
    Logs(Vec<(i64, Result<String, String>)>),
}

/// A failed job's log, as far as it came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Log {
    Reading,
    /// Its end, where the error is.
    Read(Trimmed),
    Failed(String),
}

/// A read out longer than this is taken for lost, and asked again (ms). Every call has a
/// timeout well inside it.
const LOST_MS: i64 = 4 * REFRESH_MS;

/// The PR the full view shows. Made by [`Opened::open`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    url: String,
    repo: String,
    number: u64,
    tab: Tab,
    /// Each tab's selected item, as last moved to (clamped where it is read).
    selected: [usize; 3],
    full: Option<Full>,
    /// The conversation, worked out on each read.
    items: Vec<Item>,
    /// The checks as the Checks tab lists them, worked out on each read.
    checks: Vec<CheckItem>,
    /// When the last good read came (ms).
    read_at: Option<i64>,
    /// Why the last read failed, and when: only while no good read has come since.
    failed: Option<(String, i64)>,
    /// When the read out now was asked, while one is.
    asked_at: Option<i64>,
    /// When the last read was asked (ms): the next is due [`REFRESH_MS`] after.
    last_asked: i64,
    /// Whether the PR changed after the read out now was asked, so it is asked again once
    /// that one comes.
    is_stale: bool,
    /// Threads resolved or reopened from here (by id), as GitHub said they now are: kept over
    /// a read asked before the change, so it doesn't undo them on screen.
    marked: BTreeMap<String, bool>,
    /// By job id.
    logs: BTreeMap<i64, Log>,
    /// Whether a read has put the selection where it starts (the first open thread).
    is_placed: bool,
}

impl Opened {
    /// `url`'s PR opened on `tab` at `now`, and the read that fills it; `None` where the URL
    /// isn't a PR's.
    #[must_use]
    pub fn open(url: &str, tab: Tab, now: i64) -> Option<(Self, Read)> {
        let pr = ref_from_url(url)?;
        let mut opened = Self {
            url: pr.url,
            repo: pr.repo,
            number: pr.number,
            tab,
            selected: [0; 3],
            full: None,
            items: vec![Item::Description],
            checks: Vec::new(),
            read_at: None,
            failed: None,
            asked_at: None,
            last_asked: now,
            is_stale: false,
            marked: BTreeMap::new(),
            logs: BTreeMap::new(),
            is_placed: false,
        };
        let read = opened.ask(now);
        Some((opened, read))
    }

    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    #[must_use]
    pub const fn number(&self) -> u64 {
        self.number
    }

    #[must_use]
    pub const fn tab(&self) -> Tab {
        self.tab
    }

    /// What the last good read said.
    #[must_use]
    pub const fn full(&self) -> Option<&Full> {
        self.full.as_ref()
    }

    /// The conversation's items: the description alone until a read comes.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The checks as the Checks tab lists them.
    #[must_use]
    pub fn checks(&self) -> &[CheckItem] {
        &self.checks
    }

    /// The check selected, where the Checks tab shows and lists one.
    #[must_use]
    pub fn check(&self) -> Option<&CheckItem> {
        if self.tab != Tab::Checks {
            return None;
        }
        self.checks.get(self.selected(Tab::Checks))
    }

    /// The review thread selected, where the conversation shows and one is.
    #[must_use]
    pub fn thread(&self) -> Option<&GhThread> {
        if self.tab != Tab::Conversation {
            return None;
        }
        match self.items.get(self.selected(Tab::Conversation))? {
            Item::Thread(i) => self.full.as_ref()?.threads.get(*i),
            _ => None,
        }
    }

    /// The thread `id` resolved (or reopened), as GitHub said it now is, before the read that
    /// follows comes.
    pub fn mark_resolved(&mut self, id: &str, resolved: bool) {
        self.marked.insert(id.to_owned(), resolved);
        let threads = self.full.iter_mut().flat_map(|f| &mut f.threads);
        for thread in threads.filter(|t| t.id == id) {
            thread.is_resolved = resolved;
        }
    }

    /// The threads on a file, by their place in [`Full::threads`].
    #[must_use]
    pub fn threads_on(&self, path: &str) -> Vec<&GhThread> {
        self.full
            .iter()
            .flat_map(|f| &f.threads)
            .filter(|t| t.path == path)
            .collect()
    }

    /// When the last good read came (ms).
    #[must_use]
    pub const fn read_at(&self) -> Option<i64> {
        self.read_at
    }

    /// Why the last read failed, and when, where none has come good since.
    #[must_use]
    pub fn failed(&self) -> Option<(&str, i64)> {
        self.failed.as_ref().map(|(why, at)| (why.as_str(), *at))
    }

    /// Whether a read of the PR is out.
    #[must_use]
    pub const fn is_reading(&self) -> bool {
        self.asked_at.is_some()
    }

    /// Whether a failed job's log is still coming.
    #[must_use]
    pub fn is_reading_logs(&self) -> bool {
        self.logs.values().any(|log| *log == Log::Reading)
    }

    /// A job's log, where one was asked.
    #[must_use]
    pub fn log(&self, job: i64) -> Option<&Log> {
        self.logs.get(&job)
    }

    /// How many items `tab` lists.
    #[must_use]
    pub fn len(&self, tab: Tab) -> usize {
        match tab {
            Tab::Conversation => self.items.len(),
            Tab::Checks => self.checks.len(),
            Tab::Files => self.full.as_ref().map_or(0, |f| f.files.len()),
        }
    }

    /// The item selected in `tab`: always one it lists, or 0 where it lists none.
    #[must_use]
    pub fn selected(&self, tab: Tab) -> usize {
        self.selected[tab.index()].min(self.len(tab).saturating_sub(1))
    }

    pub const fn show(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// Moves the selection in the tab showing by `by` items (clamped at the ends).
    pub fn step(&mut self, by: isize) {
        let at = self.selected(self.tab).saturating_add_signed(by);
        self.select(at);
    }

    /// Selects item `at` of the tab showing (clamped).
    pub fn select(&mut self, at: usize) {
        self.selected[self.tab.index()] = at.min(self.len(self.tab).saturating_sub(1));
    }

    fn ask(&mut self, now: i64) -> Read {
        self.asked_at = Some(now);
        self.last_asked = now;
        Read::Pr {
            url: self.url.clone(),
            repo: self.repo.clone(),
            number: self.number,
        }
    }

    /// Whether a read is out, as far as `now` goes: one out past [`LOST_MS`] is lost.
    fn is_out(&self, now: i64) -> bool {
        self.asked_at.is_some_and(|at| now - at < LOST_MS)
    }

    /// The read due at `now`: [`REFRESH_MS`] after the last was asked, none while one is out,
    /// and none while GitHub has said to wait (`held_until`, ms).
    pub fn due(&mut self, now: i64, held_until: Option<i64>) -> Option<Read> {
        let is_held = held_until.is_some_and(|until| until > now);
        let is_due = now - self.last_asked >= REFRESH_MS;
        (is_due && !is_held && !self.is_out(now)).then(|| self.ask(now))
    }

    /// A read now, after something changed the PR (a merge, a comment); where one is out,
    /// asked before the change, another once it comes.
    pub fn again(&mut self, now: i64) -> Option<Read> {
        if self.is_out(now) {
            self.is_stale = true;
            return None;
        }
        Some(self.ask(now))
    }

    /// The read a change while one was out calls for, once it comes.
    fn stale_read(&mut self, now: i64) -> Option<Read> {
        std::mem::take(&mut self.is_stale).then(|| self.ask(now))
    }

    /// What a read came to, at `now`; the logs it calls for, each job's once.
    pub fn got(&mut self, got: Got, now: i64) -> Vec<Read> {
        match got {
            Got::Pr(Ok(mut full)) => {
                if self.is_stale {
                    // Asked before a change: what a resolve or reopen made of a thread holds.
                    for thread in &mut full.threads {
                        if let Some(&resolved) = self.marked.get(&thread.id) {
                            thread.is_resolved = resolved;
                        }
                    }
                } else {
                    self.marked.clear();
                }
                // Each tab stays on the item it was on, wherever the read puts it.
                let kept = Tab::ALL.map(|tab| self.key_of(tab));
                self.asked_at = None;
                self.items = timeline(&full);
                self.checks = checks_in_order(&full.checks).into_iter().cloned().collect();
                self.full = Some(full);
                self.read_at = Some(now);
                self.failed = None;
                for (tab, key) in Tab::ALL.into_iter().zip(kept) {
                    if let Some(at) = key.and_then(|key| self.position_of(tab, &key)) {
                        self.selected[tab.index()] = at;
                    }
                }
                if !self.is_placed {
                    self.is_placed = true;
                    self.place();
                }
                self.logs_due()
                    .into_iter()
                    .chain(self.stale_read(now))
                    .collect()
            }
            Got::Pr(Err(why)) => {
                self.asked_at = None;
                self.failed = Some((why, now));
                self.stale_read(now).into_iter().collect()
            }
            Got::Logs(logs) => {
                for (job, log) in logs {
                    let log = match log {
                        Ok(raw) => Log::Read(trim_log(&raw, LOG_LINES, LOG_CHARS)),
                        Err(why) => Log::Failed(why),
                    };
                    self.logs.insert(job, log);
                }
                Vec::new()
            }
        }
    }

    /// What the item selected in `tab` is, across reads, where a read has come.
    fn key_of(&self, tab: Tab) -> Option<String> {
        let full = self.full.as_ref()?;
        let at = self.selected(tab);
        match tab {
            Tab::Conversation => self.items.get(at).map(|&item| item_key(full, item)),
            Tab::Checks => self.checks.get(at).map(check_key),
            Tab::Files => full.files.get(at).map(|f| f.path.clone()),
        }
    }

    /// Where the item `key` names is in `tab` now, where it still is.
    fn position_of(&self, tab: Tab, key: &str) -> Option<usize> {
        let full = self.full.as_ref()?;
        match tab {
            Tab::Conversation => self.items.iter().position(|&i| item_key(full, i) == key),
            Tab::Checks => self.checks.iter().position(|c| check_key(c) == key),
            Tab::Files => full.files.iter().position(|f| f.path == key),
        }
    }

    /// The conversation opens on its first open thread, where it has one.
    fn place(&mut self) {
        let threads = self.full.as_ref().map(|f| &f.threads);
        let first_open = self.items.iter().position(|item| match item {
            Item::Thread(i) => threads.is_some_and(|t| !t[*i].is_resolved),
            _ => false,
        });
        if let Some(at) = first_open {
            self.selected[Tab::Conversation.index()] = at;
        }
    }

    /// The failed jobs whose logs haven't been asked: the first few, as a hand-off reads them,
    /// a read each, so one log's minute holds up none of the others.
    fn logs_due(&mut self) -> Vec<Read> {
        let Some(full) = self.full.as_ref() else {
            return Vec::new();
        };
        let failures = failing_checks(&full.checks, &self.repo);
        let jobs: Vec<i64> = to_fetch(&failures)
            .into_iter()
            .filter_map(|f| f.job_id)
            .filter(|job| !self.logs.contains_key(job))
            .collect();
        jobs.into_iter()
            .map(|job| {
                self.logs.insert(job, Log::Reading);
                Read::Logs {
                    url: self.url.clone(),
                    repo: self.repo.clone(),
                    jobs: vec![job],
                }
            })
            .collect()
    }
}

/// What an item of the conversation is, across reads.
fn item_key(full: &Full, item: Item) -> String {
    let login =
        |author: &Option<Login>| author.as_ref().map_or("", |a| a.login.as_str()).to_owned();
    match item {
        Item::Description => "description".to_owned(),
        Item::Comment(i) => format!("comment {}", full.comments[i].url),
        Item::Review(i) => {
            let r = &full.reviews[i];
            format!("review {} {}", login(&r.author), r.submitted_at)
        }
        Item::Thread(i) => format!("thread {}", full.threads[i].id),
    }
}

/// What a check is, across reads: its kind, workflow and name.
fn check_key(check: &CheckItem) -> String {
    format!(
        "{:?} {:?} {}",
        check.typename,
        check.workflow,
        check_name(check)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::Comments;
    use crate::full::ChangedFile;
    use crate::gh::{CheckKind, CheckStatus, Conclusion};
    use proptest::prelude::*;

    const URL: &str = "https://github.com/acme/app/pull/12";

    fn failed(job: i64) -> CheckItem {
        CheckItem {
            typename: Some(CheckKind::CheckRun),
            name: Some(format!("job {job}")),
            // In a workflow: an Actions job, whose log gh reads.
            workflow: Some("ci".into()),
            status: Some(CheckStatus::Completed),
            conclusion: Some(Conclusion::Failure),
            database_id: Some(job),
            ..CheckItem::default()
        }
    }

    fn thread(is_resolved: bool, at: &str) -> GhThread {
        GhThread {
            is_resolved,
            comments: Comments {
                total_count: None,
                nodes: vec![crate::address::GhComment {
                    created_at: at.into(),
                    ..Default::default()
                }],
            },
            ..GhThread::default()
        }
    }

    fn opened() -> Opened {
        Opened::open(URL, Tab::Conversation, 0).expect("a PR URL").0
    }

    #[test]
    fn it_reads_on_open_and_again_every_thirty_seconds_while_none_is_out() {
        let (mut o, read) = Opened::open(URL, Tab::Conversation, 1_000).expect("a PR URL");
        assert_eq!(
            read,
            Read::Pr {
                url: URL.into(),
                repo: "acme/app".into(),
                number: 12
            }
        );
        assert!(o.is_reading());
        // Out: nothing more is asked, even past its time.
        assert_eq!(o.due(40_000, None), None);
        o.got(Got::Pr(Ok(Full::default())), 2_000);
        assert_eq!(o.due(30_999, None), None);
        assert!(o.due(31_000, None).is_some());
        o.got(Got::Pr(Err("HTTP 502".into())), 32_000);
        assert_eq!(o.failed(), Some(("HTTP 502", 32_000)));
        // GitHub said to wait.
        assert_eq!(o.due(70_000, Some(80_000)), None);
        assert!(o.due(80_000, Some(80_000)).is_some());
        // A read that never came back is taken for lost.
        assert!(o.due(80_000 + LOST_MS, None).is_some());
        assert_eq!(Opened::open("https://example.com/x", Tab::Checks, 0), None);
    }

    #[test]
    fn each_failed_jobs_log_is_asked_once() {
        let mut o = opened();
        let full = Full {
            checks: vec![failed(7), failed(8)],
            ..Full::default()
        };
        let asked = o.got(Got::Pr(Ok(full.clone())), 1);
        assert_eq!(
            asked,
            [7, 8].map(|job| Read::Logs {
                url: URL.into(),
                repo: "acme/app".into(),
                jobs: vec![job]
            })
        );
        assert_eq!(o.log(7), Some(&Log::Reading));
        assert_eq!(o.got(Got::Pr(Ok(full)), 2), []);
        o.got(
            Got::Logs(vec![
                (7, Ok("job\tstep\tboom".into())),
                (8, Err("gone".into())),
            ]),
            3,
        );
        assert!(matches!(o.log(7), Some(Log::Read(t)) if t.text == "boom"));
        assert_eq!(o.log(8), Some(&Log::Failed("gone".into())));
        // A run again is a new job, whose log is asked.
        let rerun = Full {
            checks: vec![failed(9)],
            ..Full::default()
        };
        assert_eq!(o.got(Got::Pr(Ok(rerun)), 4).len(), 1);
    }

    #[test]
    fn the_conversation_opens_on_the_first_open_thread_once() {
        let mut o = opened();
        let full = Full {
            threads: vec![
                thread(true, "2026-10-24T09:00:00Z"),
                thread(false, "2026-10-24T10:00:00Z"),
            ],
            ..Full::default()
        };
        o.got(Got::Pr(Ok(full.clone())), 1);
        assert_eq!(o.items()[o.selected(Tab::Conversation)], Item::Thread(1));
        o.select(0);
        o.got(Got::Pr(Ok(full)), 2);
        assert_eq!(o.selected(Tab::Conversation), 0);
    }

    #[test]
    fn a_read_that_reorders_a_tab_keeps_it_on_the_item_it_was_on() {
        let mut o = opened();
        let passed = |job: i64| CheckItem {
            conclusion: Some(Conclusion::Success),
            ..failed(job)
        };
        o.got(
            Got::Pr(Ok(Full {
                checks: vec![failed(1), passed(2)],
                ..Full::default()
            })),
            1,
        );
        o.show(Tab::Checks);
        o.select(1);
        assert_eq!(
            o.checks()[o.selected(Tab::Checks)].name.as_deref(),
            Some("job 2")
        );
        // Job 2 fails now: worst first, it goes to the top, and the selection with it.
        o.got(
            Got::Pr(Ok(Full {
                checks: vec![passed(1), failed(2)],
                ..Full::default()
            })),
            2,
        );
        assert_eq!(
            o.checks()[o.selected(Tab::Checks)].name.as_deref(),
            Some("job 2")
        );
    }

    #[test]
    fn a_change_while_a_read_is_out_is_read_once_it_comes() {
        let mut o = opened();
        // Asked before the merge: what it brings may not have it.
        assert_eq!(o.again(5), None);
        let asked = o.got(Got::Pr(Ok(Full::default())), 6);
        assert!(matches!(asked.as_slice(), [Read::Pr { .. }]), "{asked:?}");
        assert!(o.is_reading());
        assert_eq!(o.got(Got::Pr(Ok(Full::default())), 7), []);
    }

    #[derive(Debug, Clone)]
    enum Move {
        Step(isize),
        Select(usize),
        Show(Tab),
        Read(usize, usize, usize),
    }

    fn arb_move() -> impl Strategy<Value = Move> {
        prop_oneof![
            (-5_isize..5).prop_map(Move::Step),
            Just(Move::Step(isize::MIN)),
            Just(Move::Step(isize::MAX)),
            (0_usize..30).prop_map(Move::Select),
            prop::sample::select(Tab::ALL.to_vec()).prop_map(Move::Show),
            (0_usize..6, 0_usize..6, 0_usize..6).prop_map(|(t, c, f)| Move::Read(t, c, f)),
        ]
    }

    proptest! {
        #[test]
        fn the_selection_is_always_an_item_the_tab_lists(moves in prop::collection::vec(arb_move(), 0..40)) {
            let mut o = opened();
            for (n, m) in moves.into_iter().enumerate() {
                match m {
                    Move::Step(by) => o.step(by),
                    Move::Select(at) => o.select(at),
                    Move::Show(tab) => o.show(tab),
                    Move::Read(threads, checks, files) => {
                        let full = Full {
                            threads: (0..threads).map(|_| thread(false, "")).collect(),
                            checks: (0..checks).map(|i| failed(i64::try_from(i).unwrap_or(0))).collect(),
                            files: (0..files).map(|i| ChangedFile { path: format!("f{i}"), ..ChangedFile::default() }).collect(),
                            ..Full::default()
                        };
                        o.got(Got::Pr(Ok(full)), i64::try_from(n).unwrap_or(0));
                    }
                }
                for tab in Tab::ALL {
                    let (at, len) = (o.selected(tab), o.len(tab));
                    prop_assert!(at < len || (len == 0 && at == 0), "{tab:?}: {at} of {len}");
                }
            }
        }

        #[test]
        fn tabs_go_round(by in -10_isize..10) {
            for tab in Tab::ALL {
                prop_assert_eq!(tab.along(by).along(-by), tab);
            }
            prop_assert_eq!(Tab::Files.along(1), Tab::Conversation);
        }
    }
}
