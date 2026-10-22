//! The TUI's state and what each message does to it, Elm-style: [`update`] takes a
//! [`Model`] and a [`Msg`] and gives the next model and the [`Cmd`]s to run. No I/O and no
//! clock: the binary reads the files and the time, and says what it read as messages.
//!
//! What the screen shows is worked out here too, once per message rather than per frame:
//! the [`List`]s (All, then one per live session), each list's [`Row`]s in urgency order
//! ([`order`]), and the [`Header`]. The view only draws them.
//!
//! The selection is a PR's URL, not an index, so it stays on its PR when a round reorders
//! the list. After every message it points at a row of the selected list, or is `None`
//! exactly when that list is empty (a property test holds it to that).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::act::{Acts, Ask, Done, Job, Offer, Target};
use crate::layout::{Layout, layout};
use crate::order::{Tracked, order};
use crate::protocol::{Entry, SessionFile, Shared, Writer};
use crate::status::{PillKind, derive};

/// A session file is live while its mtime is less than this old (the mod's `ALIVE_MS`).
pub const ALIVE_MS: i64 = 60_000;

/// A mod is live but no round has run for this long past `nextAt`: the results are stale.
/// The give-way rule's threshold: past it, `prw` takes the turn ([`FREE_MS`](crate::poll::FREE_MS)).
pub const STALE_MS: i64 = crate::poll::FREE_MS;

/// What a card says of an entry no gh account could see, where it has no error of its own
/// (the mod's `NOT_FOUND`, `hooks/cache.ts`).
pub use crate::entries::NOT_FOUND;

/// What the "Claude session" list is called: the live session files an older mod wrote,
/// with no name (and no writer) to tell them apart.
pub const UNNAMED: &str = "Claude session";

/// Whether a session file with this mtime is live at `now` (both ms). Pure: liveness is
/// only ever a function of the two.
#[must_use]
pub const fn is_live(mtime: i64, now: i64) -> bool {
    now.saturating_sub(mtime) < ALIVE_MS
}

/// How long ago, as the mod words it (`ago`, `hooks/pr.ts`): "just now" under 10 s, then
/// "12s ago", "3m ago", "5h ago", "2d ago". Negative is "just now".
#[must_use]
pub fn ago(ms: i64) -> String {
    // Math.round(ms / 1000): halves round up, toward +∞.
    let s = ms.saturating_add(500).div_euclid(1000).max(0);
    if s < 10 {
        "just now".to_owned()
    } else if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

/// One session file as the binary read it: its `<id>` (the file name without `.json`), what
/// it says (its last good parse; one that has never parsed isn't read as a session), and its
/// mtime (ms), the heartbeat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub file: SessionFile,
    pub mtime: i64,
}

impl Session {
    /// The agent it runs: its `agent`, or `claude` where a mod (any version) wrote it.
    #[must_use]
    pub fn agent(&self) -> Option<&str> {
        match (&self.file.agent, &self.file.writer) {
            (Some(agent), _) => Some(agent),
            (None, None | Some(Writer::Mod)) => Some("claude"),
            (None, Some(_)) => None,
        }
    }

    /// Whether a mod wrote it: `writer: "mod"`, or no writer (a mod before 0.14.1).
    #[must_use]
    pub const fn is_mod(&self) -> bool {
        matches!(self.file.writer, None | Some(Writer::Mod))
    }

    /// Whether it goes in the shared "Claude session" list: an older mod's, with no name.
    #[must_use]
    pub const fn is_unnamed(&self) -> bool {
        self.file.name.is_none() && self.file.writer.is_none()
    }

    /// What a list calls it: its name, else its id.
    #[must_use]
    pub fn label(&self) -> &str {
        self.file.name.as_deref().unwrap_or(&self.id)
    }
}

/// Why `results.json` gave no [`Shared`] this time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// No such file (or no cache directory): nothing fetched yet.
    Missing,
    /// There, but it didn't read or parse (torn mid-write, say): keep the last good copy.
    Unreadable(String),
}

/// A key, as the model sees it (the binary maps the terminal's keys to these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Char(char),
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
    Home,
    End,
    Enter,
    /// Alt-Enter: a new line in the comment field.
    Newline,
    Backspace,
    Esc,
    CtrlC,
}

/// Something that happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    /// `results.json` was read at this time (ms): what it said, or why it didn't.
    SharedRead(Result<Shared, ReadFailure>, i64),
    /// The session files, all of them (live or not), as read at this time (ms).
    SessionsRead(Vec<Session>, i64),
    Key(Key),
    /// The terminal's new width and height.
    Resize(u16, u16),
    /// The clock, about once a second: ages and liveness move on.
    Tick(i64),
    /// `prw`'s own poller, after a tick.
    Poller(PollerSaid),
    /// Text pasted into the terminal.
    Paste(String),
    /// What came of a [`Job`].
    Done(Done),
    /// The copy a [`Cmd::Copy`] asked for: by which means ("pbcopy"), or why it failed.
    Copied(Result<String, String>),
    /// The browser a [`Cmd::Open`] asked for, for this URL: opened, or why not.
    Opened(String, Result<(), String>),
}

/// What `prw`'s own poller said after a tick.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PollerSaid {
    /// Its session file's `<id>`: its list ("My PRs") comes first, after All.
    pub own: String,
    /// When the last round it ran started (that round's `fetchedAt`).
    pub polled_at: Option<i64>,
    /// Why it couldn't find a login's PRs, where it couldn't (no token, nobody logged in).
    pub error: Option<String>,
}

/// What the binary should do after a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Quit,
    /// Ask GitHub, off the UI thread; a [`Msg::Done`] says what came of it.
    Run(Job),
    /// Put this text on the clipboard; a [`Msg::Copied`] says how.
    Copy(String),
    /// Open this URL in the browser; a [`Msg::Opened`] says whether it did.
    Open(String),
}

/// Which pane the movement keys move in. Only the three-pane layout has a lists pane to
/// focus; elsewhere it is always the PRs ([`Model::focus`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Focus {
    Lists,
    #[default]
    Prs,
}

/// Which list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum ListId {
    /// Every PR a live session watches (or, with none live, every PR in `results.json`).
    #[default]
    All,
    /// One live session file, by its id.
    Session(String),
    /// The live older-mod files with no name, together.
    Unnamed,
}

/// One PR in a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Its card: from `derive`, or the loading placeholder where it has no data yet.
    pub pr: Tracked,
    /// `owner/repo`, from the URL.
    pub repo: String,
    /// The agent of the first live session that watches it, if any does.
    pub agent: Option<String>,
    /// The live sessions that watch it, by label.
    pub watchers: Vec<String>,
    /// When its data was fetched (ms), where it has any.
    pub checked_at: Option<i64>,
}

/// A list of PRs, as the lists pane or strip names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub id: ListId,
    pub label: String,
    /// The agent its session runs (session lists only).
    pub agent: Option<String>,
    /// Where its session runs (session lists only).
    pub cwd: Option<String>,
    /// Most urgent first.
    pub rows: Vec<Row>,
}

impl List {
    /// The most urgent row's pill, for the list's mark: the first row, as rows are ordered.
    #[must_use]
    pub fn worst(&self) -> Option<PillKind> {
        self.rows.first().map(|row| row.pr.derived.pill)
    }
}

/// Who keeps `results.json` fresh, as the header says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Poll {
    /// A mod session is live and the results are fresh.
    Mod,
    /// `prw` ran the last round, and the results are fresh.
    Prw,
    /// No mod session is live, and `prw` doesn't poll (or hasn't yet): nothing updates them.
    Nobody,
    /// A round is long overdue: whoever polls has stopped.
    Stale,
    /// No `results.json` yet.
    #[default]
    Waiting,
}

impl Poll {
    /// The header's words.
    #[must_use]
    pub const fn words(self) -> &'static str {
        match self {
            Self::Mod => "polled by mod",
            Self::Prw => "polled by prw",
            Self::Nobody => "no poller",
            Self::Stale => "stale",
            Self::Waiting => "waiting for pr-watch",
        }
    }
}

/// How many open PRs in All are in each state the header counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tally {
    pub failing: usize,
    pub conflicts: usize,
    pub changes: usize,
    pub running: usize,
    pub ready: usize,
    pub review: usize,
}

/// What the header says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    pub poll: Poll,
    /// How long ago the results' round ran (ms), where one has.
    pub age: Option<i64>,
    /// Live session files.
    pub live: usize,
    /// The last read of `results.json` failed (the header shows the last good copy's age).
    pub is_read_failing: bool,
    pub tally: Tally,
}

/// The whole state of the screen. Made by [`Model::new`], changed only by [`update`].
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    shared: Option<Shared>,
    read_ok_at: Option<i64>,
    read_failed_at: Option<i64>,
    sessions: Vec<Session>,
    now: i64,
    width: u16,
    height: u16,
    list: ListId,
    selected: Option<String>,
    focus: Focus,
    is_help: bool,
    poller: Option<PollerSaid>,
    acts: Acts,
    // Worked out from the rest on each message that changes it. Never empty: All is first.
    lists: Vec<List>,
    header: Header,
}

impl Model {
    /// The model before anything was read, for a `width` × `height` terminal.
    #[must_use]
    pub fn new(width: u16, height: u16) -> Self {
        let mut model = Self {
            shared: None,
            read_ok_at: None,
            read_failed_at: None,
            sessions: Vec::new(),
            now: 0,
            width,
            height,
            list: ListId::All,
            selected: None,
            focus: Focus::Prs,
            is_help: false,
            poller: None,
            acts: Acts::default(),
            lists: Vec::new(),
            header: Header::default(),
        };
        model.rebuild();
        model
    }

    /// The last good `results.json`, if any read.
    #[must_use]
    pub const fn shared(&self) -> Option<&Shared> {
        self.shared.as_ref()
    }

    /// The entry `results.json` holds for a URL.
    #[must_use]
    pub fn entry(&self, url: &str) -> Option<&Entry> {
        self.shared.as_ref()?.prs.get(url)
    }

    /// The last read found no `results.json` (and none read before).
    #[must_use]
    pub const fn is_waiting(&self) -> bool {
        self.shared.is_none()
    }

    /// When `results.json` last read well, and when a read last failed (ms).
    #[must_use]
    pub const fn reads(&self) -> (Option<i64>, Option<i64>) {
        (self.read_ok_at, self.read_failed_at)
    }

    /// What `prw`'s own poller last said; `None` where none runs.
    #[must_use]
    pub const fn poller(&self) -> Option<&PollerSaid> {
        self.poller.as_ref()
    }

    /// Every session file read, live or not.
    #[must_use]
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    /// The live session files.
    pub fn live_sessions(&self) -> impl Iterator<Item = &Session> {
        self.sessions.iter().filter(|s| is_live(s.mtime, self.now))
    }

    /// The time of the last message that carried one (ms).
    #[must_use]
    pub const fn now(&self) -> i64 {
        self.now
    }

    /// The terminal's size.
    #[must_use]
    pub const fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// The layout the width gets.
    #[must_use]
    pub const fn layout(&self) -> Layout {
        layout(self.width)
    }

    /// The lists: All first, then one per live session.
    #[must_use]
    pub fn lists(&self) -> &[List] {
        &self.lists
    }

    /// The selected list's id.
    #[must_use]
    pub const fn list_id(&self) -> &ListId {
        &self.list
    }

    /// The selected list's index in [`Model::lists`].
    #[must_use]
    pub fn list_index(&self) -> usize {
        self.lists
            .iter()
            .position(|l| l.id == self.list)
            .unwrap_or(0)
    }

    /// The selected list.
    #[must_use]
    pub fn list(&self) -> &List {
        &self.lists[self.list_index()]
    }

    /// The selected PR's URL; `None` exactly when the selected list is empty.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The selected PR's index in the selected list's rows.
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        let url = self.selected.as_deref()?;
        self.list().rows.iter().position(|r| r.pr.url == url)
    }

    /// The selected PR's row.
    #[must_use]
    pub fn selected_row(&self) -> Option<&Row> {
        self.selected_index().map(|i| &self.list().rows[i])
    }

    /// The pane the movement keys move in: the lists only in the three-pane layout.
    #[must_use]
    pub fn focus(&self) -> Focus {
        if self.layout() == Layout::Three {
            self.focus
        } else {
            Focus::Prs
        }
    }

    /// Whether the key help shows.
    #[must_use]
    pub const fn is_help(&self) -> bool {
        self.is_help
    }

    /// What the header says.
    #[must_use]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// What the action keys left on the screen: the question, the toast, what runs.
    #[must_use]
    pub const fn acts(&self) -> &Acts {
        &self.acts
    }

    /// The PR an action key acts on: the one an open question is about, else the selected
    /// one.
    fn target(&self, ask: Option<&Ask>) -> Option<Target<'_>> {
        let row = match ask {
            Some(ask) => self
                .list()
                .rows
                .iter()
                .chain(&self.lists[0].rows)
                .find(|r| r.pr.url == ask.url())?,
            None => self.selected_row()?,
        };
        Some(self.target_of(row))
    }

    fn target_of<'a>(&'a self, row: &'a Row) -> Target<'a> {
        let owner = row.repo.split('/').next().unwrap_or_default();
        Target {
            row,
            pr: self.entry(&row.pr.url).and_then(|e| e.pr.as_ref()),
            login: self
                .shared
                .as_ref()
                .and_then(|s| s.accounts.get(owner))
                .map_or("", String::as_str),
        }
    }

    /// What the selected PR offers, for its actions list.
    #[must_use]
    pub fn offers(&self) -> Vec<Offer> {
        self.selected_row()
            .map(|row| self.acts.offers(self.target_of(row)))
            .unwrap_or_default()
    }

    /// Works out the lists and the header again, and puts the selection back on a row: the
    /// same PR where it is still listed, else the row where it was (or the last).
    fn rebuild(&mut self) {
        let mut was = self.selected_index();
        let own = self.poller.as_ref().map(|p| p.own.as_str());
        self.lists = lists_of(self.shared.as_ref(), &self.sessions, self.now, own);
        if !self.lists.iter().any(|l| l.id == self.list) {
            // A different list: its first row, not the old list's place.
            self.list = ListId::All;
            was = None;
        }
        self.header = header_of(self);
        let rows = &self.list().rows;
        let is_listed = self
            .selected
            .as_deref()
            .is_some_and(|url| rows.iter().any(|r| r.pr.url == url));
        if !is_listed {
            self.selected = if rows.is_empty() {
                None
            } else {
                Some(rows[was.unwrap_or(0).min(rows.len() - 1)].pr.url.clone())
            };
        }
    }

    /// Selects list `index` (clamped) and its first PR; the list already selected keeps its PR.
    fn switch_to(&mut self, index: usize) {
        let list = &self.lists[index.min(self.lists.len() - 1)];
        if list.id == self.list {
            return;
        }
        self.list = list.id.clone();
        self.selected = list.rows.first().map(|r| r.pr.url.clone());
    }

    /// Moves the selection in the focused pane by `by` rows (clamped at the ends).
    fn step(&mut self, by: isize) {
        match self.focus() {
            Focus::Lists => {
                let to = self.list_index().saturating_add_signed(by);
                self.switch_to(to.min(self.lists.len() - 1));
            }
            Focus::Prs => {
                let rows = &self.list().rows;
                if rows.is_empty() {
                    return;
                }
                let from = self.selected_index().unwrap_or(0);
                let to = from.saturating_add_signed(by).min(rows.len() - 1);
                self.selected = Some(rows[to].pr.url.clone());
            }
        }
    }

    /// The next (`by` 1) or previous (-1) list, round the end.
    fn cycle(&mut self, by: isize) {
        let n = self.lists.len();
        let to = (self.list_index() + n).saturating_add_signed(by) % n;
        self.switch_to(to);
    }

    fn key(&mut self, key: Key) -> Vec<Cmd> {
        if !self.is_help {
            // Taken out while they act, so the target borrows the rest without a copy.
            let mut acts = std::mem::take(&mut self.acts);
            let target = self.target(acts.ask());
            let cmds = acts.key(key, target, self.now);
            acts.clamp_view(usize::from(self.width), usize::from(self.height));
            self.acts = acts;
            if let Some(cmds) = cmds {
                return cmds;
            }
        }
        let is_three = self.layout() == Layout::Three;
        match key {
            // Esc closes the help first; with none open, it quits as q does.
            Key::Esc if self.is_help => self.is_help = false,
            Key::CtrlC | Key::Char('q') | Key::Esc => return vec![Cmd::Quit],
            Key::Char('?') => self.is_help = !self.is_help,
            Key::Char('j') | Key::Down => self.step(1),
            Key::Char('k') | Key::Up => self.step(-1),
            Key::Char('g') | Key::Home => self.step(isize::MIN),
            Key::Char('G') | Key::End => self.step(isize::MAX),
            // Three panes: h and l move between the lists pane and the PRs. Elsewhere the
            // lists are a strip, and h and l step through it as Tab does.
            Key::Char('h') | Key::Left if is_three => self.focus = Focus::Lists,
            Key::Char('l') | Key::Right if is_three => self.focus = Focus::Prs,
            Key::Tab | Key::Char('l') | Key::Right => self.cycle(1),
            Key::BackTab | Key::Char('h') | Key::Left => self.cycle(-1),
            // Enter opens the full PR view, a later step; every other key does nothing.
            Key::Enter | Key::Newline | Key::Backspace | Key::Char(_) => {}
        }
        Vec::new()
    }
}

/// The next model, and what to do, after `msg` (Elm's `update`).
#[must_use]
pub fn update(mut model: Model, msg: Msg) -> (Model, Vec<Cmd>) {
    let cmds = match msg {
        Msg::SharedRead(read, at) => {
            model.now = at;
            match read {
                Ok(shared) => {
                    model.shared = Some(shared);
                    model.read_ok_at = Some(at);
                    model.read_failed_at = None;
                }
                // Gone: nothing fetched (any more). The header waits for pr-watch again.
                // A failed read before it went is no longer news.
                Err(ReadFailure::Missing) => {
                    model.shared = None;
                    model.read_failed_at = None;
                }
                // Torn or bad: the last good copy stands, and its age shows.
                Err(ReadFailure::Unreadable(_)) => model.read_failed_at = Some(at),
            }
            model.rebuild();
            Vec::new()
        }
        Msg::SessionsRead(sessions, at) => {
            model.sessions = sessions;
            model.now = at;
            model.rebuild();
            Vec::new()
        }
        Msg::Tick(at) => {
            model.now = at;
            model.acts.tick(at);
            model.rebuild();
            Vec::new()
        }
        Msg::Paste(text) => {
            model.acts.paste(&text);
            Vec::new()
        }
        Msg::Done(done) => model.acts.done(done, model.now),
        Msg::Copied(by) => {
            model.acts.copied_by(by, model.now);
            Vec::new()
        }
        Msg::Opened(url, by) => {
            model.acts.opened(&url, by, model.now);
            Vec::new()
        }
        Msg::Poller(said) => {
            model.poller = Some(said);
            model.rebuild();
            Vec::new()
        }
        Msg::Resize(width, height) => {
            model.width = width;
            model.height = height;
            model
                .acts
                .clamp_view(usize::from(width), usize::from(height));
            Vec::new()
        }
        Msg::Key(key) => model.key(key),
    };

    (model, cmds)
}

/// The PR number at the end of a PR URL (`…/pull/12`), or 0.
#[must_use]
pub fn number_of(url: &str) -> u64 {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// `owner/repo` from a PR URL (`https://github.com/<owner>/<repo>/pull/<n>`), or "".
#[must_use]
pub fn repo_of(url: &str) -> String {
    let path = url.split_once("://").map_or(url, |(_, rest)| rest);
    let mut parts = path.split('/').skip(1);
    match (parts.next(), parts.next()) {
        (Some(owner), Some(repo)) if !owner.is_empty() && !repo.is_empty() => {
            format!("{owner}/{repo}")
        }
        _ => String::new(),
    }
}

/// A PR's card from its entry: `derive` over its data, the entry's error beside it; the
/// loading placeholder where it has no data (with the error, if a fetch failed).
#[must_use]
pub fn tracked_of(url: &str, entry: Option<&Entry>) -> Tracked {
    let mut placeholder = Tracked::placeholder(url.to_owned(), number_of(url));
    let Some(entry) = entry else {
        return placeholder;
    };
    let error = entry
        .error
        .clone()
        .or_else(|| entry.is_unreachable.then(|| NOT_FOUND.to_owned()));
    let Some(pr) = &entry.pr else {
        placeholder.error = error;
        return placeholder;
    };

    Tracked {
        url: url.to_owned(),
        number: if pr.number > 0 {
            pr.number
        } else {
            placeholder.number
        },
        title: pr.title.clone(),
        state: Some(pr.state.clone()),
        derived: derive(pr, None),
        error,
    }
}

/// The lists for these files at `now`: All, then `own`'s (this `prw`'s session file, its
/// "My PRs"), then each live session (by label, then id), then the unnamed older-mod
/// sessions together. Each list's rows in urgency order.
#[must_use]
pub fn lists_of(
    shared: Option<&Shared>,
    sessions: &[Session],
    now: i64,
    own: Option<&str>,
) -> Vec<List> {
    let mut live: Vec<&Session> = sessions.iter().filter(|s| is_live(s.mtime, now)).collect();
    live.sort_by_cached_key(|&s| {
        let is_own = own == Some(s.id.as_str());
        (
            !is_own,
            s.is_unnamed(),
            s.label().to_lowercase(),
            s.id.as_str(),
        )
    });
    let names = names_of(&live);
    let prs = shared.map(|s| &s.prs);
    // Who watches each URL, in list order.
    let mut watchers: HashMap<&str, Vec<&Session>> = HashMap::new();
    for session in &live {
        for url in unique(&session.file.urls) {
            watchers.entry(url).or_default().push(session);
        }
    }
    let row = |url: &str| {
        let entry = prs.and_then(|prs| prs.get(url));
        let by = watchers.get(url).map(Vec::as_slice).unwrap_or_default();
        Row {
            pr: tracked_of(url, entry),
            repo: repo_of(url),
            agent: by.iter().find_map(|s| s.agent()).map(str::to_owned),
            watchers: by
                .iter()
                .map(|s| {
                    names
                        .get(s.id.as_str())
                        .map_or(UNNAMED, String::as_str)
                        .to_owned()
                })
                .collect(),
            checked_at: entry.filter(|e| e.pr.is_some()).map(|e| e.at),
        }
    };
    let all_urls: Vec<&str> = if live.is_empty() {
        prs.map(|prs| prs.keys().map(String::as_str).collect())
            .unwrap_or_default()
    } else {
        unique(live.iter().flat_map(|s| &s.file.urls))
    };
    // Each card derived once: every list's URLs are among All's.
    let by_url: HashMap<&str, Row> = all_urls.iter().map(|&url| (url, row(url))).collect();
    // `order` over the cards, each row following its card (URLs are unique in a list).
    let rows_of = |urls: Vec<&str>| -> Vec<Row> {
        let mut rows: Vec<Option<Row>> = urls
            .into_iter()
            .map(|url| Some(by_url[url].clone()))
            .collect();
        let at: HashMap<String, usize> = rows
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, r)| (r.pr.url.clone(), i))
            .collect();
        let cards = rows.iter().flatten().map(|r| r.pr.clone()).collect();
        order(cards)
            .iter()
            .filter_map(|pr| rows[at[&pr.url]].take())
            .collect()
    };

    let mut lists = vec![List {
        id: ListId::All,
        label: "All".to_owned(),
        agent: None,
        cwd: None,
        rows: rows_of(all_urls),
    }];
    for session in live.iter().filter(|s| !s.is_unnamed()) {
        lists.push(List {
            id: ListId::Session(session.id.clone()),
            label: names[session.id.as_str()].clone(),
            agent: session.agent().map(str::to_owned),
            cwd: session.file.cwd.clone(),
            rows: rows_of(unique(&session.file.urls)),
        });
    }
    let unnamed: Vec<&&Session> = live.iter().filter(|s| s.is_unnamed()).collect();
    if !unnamed.is_empty() {
        lists.push(List {
            id: ListId::Unnamed,
            label: UNNAMED.to_owned(),
            agent: Some("claude".to_owned()),
            cwd: None,
            rows: rows_of(unique(unnamed.iter().flat_map(|s| &s.file.urls))),
        });
    }

    lists
}

/// What each named session's list is called: its label, and where two live sessions share one
/// (two sessions in one project), the last 4 characters of its id beside it, `web · k3x9`: the
/// random end of the mod's `INSTANCE` and of `prw-<id>`, where the start is a time they share.
fn names_of<'a>(live: &[&'a Session]) -> HashMap<&'a str, String> {
    let named: Vec<&Session> = live.iter().copied().filter(|s| !s.is_unnamed()).collect();
    let mut uses: HashMap<String, usize> = HashMap::new();
    for session in &named {
        *uses.entry(session.label().to_lowercase()).or_default() += 1;
    }
    named
        .into_iter()
        .map(|s| {
            let name = if uses[&s.label().to_lowercase()] > 1 {
                let skip = s.id.chars().count().saturating_sub(4);
                let tail: String = s.id.chars().skip(skip).collect();
                format!("{} · {tail}", s.label())
            } else {
                s.label().to_owned()
            };
            (s.id.as_str(), name)
        })
        .collect()
}

/// Each URL once, in the order first seen.
fn unique<'a>(urls: impl IntoIterator<Item = &'a String>) -> Vec<&'a str> {
    let mut seen = HashSet::new();
    urls.into_iter()
        .map(String::as_str)
        .filter(|url| seen.insert(*url))
        .collect()
}

/// What the header says of a model whose lists are built.
fn header_of(model: &Model) -> Header {
    let now = model.now;
    let live: Vec<&Session> = model.live_sessions().collect();
    let is_mod_live = live.iter().any(|s| s.is_mod());
    // This prw ran the round the file holds.
    let is_ours = model
        .poller
        .as_ref()
        .and_then(|p| p.polled_at)
        .zip(model.shared.as_ref())
        .is_some_and(|(at, shared)| shared.fetched_at == at);
    let poll = match &model.shared {
        None => Poll::Waiting,
        Some(shared) if is_ours && now.saturating_sub(shared.next_at) <= STALE_MS => Poll::Prw,
        Some(_) if is_ours => Poll::Stale,
        Some(_) if !is_mod_live => Poll::Nobody,
        Some(shared) if shared.fetched_at > 0 && now.saturating_sub(shared.next_at) <= STALE_MS => {
            Poll::Mod
        }
        Some(_) => Poll::Stale,
    };
    let age = model
        .shared
        .as_ref()
        .filter(|s| s.fetched_at > 0)
        .map(|s| now.saturating_sub(s.fetched_at));
    let mut tally = Tally::default();
    let open = model.lists[0]
        .rows
        .iter()
        .filter(|r| r.pr.state == Some(crate::gh::PrState::Open));
    for row in open {
        let count = match row.pr.derived.pill {
            PillKind::CiFailed | PillKind::CiFailing => &mut tally.failing,
            PillKind::Conflicts => &mut tally.conflicts,
            PillKind::Changes | PillKind::Comments => &mut tally.changes,
            PillKind::CiRunning => &mut tally.running,
            PillKind::Ready => &mut tally.ready,
            PillKind::InReview => &mut tally.review,
            _ => continue,
        };
        *count += 1;
    }

    Header {
        poll,
        age,
        live: live.len(),
        // A good read clears the failure, so any failure on record is since the last one.
        is_read_failing: model.read_failed_at.is_some(),
        tally,
    }
}

/// Live sessions per agent, for the lists pane: `(agent, count)`, by agent name. A session
/// with no agent (`prw`'s own) is left out.
#[must_use]
pub fn agents_live(model: &Model) -> Vec<(String, usize)> {
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for agent in model.live_sessions().filter_map(Session::agent) {
        *by.entry(agent.to_owned()).or_default() += 1;
    }
    by.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::{GhPr, PrState};

    #[test]
    fn ago_words_as_the_mod_does() {
        for (ms, said) in [
            (-5_000, "just now"),
            (0, "just now"),
            (9_499, "just now"),
            (9_500, "10s ago"),
            (12_000, "12s ago"),
            (59_499, "59s ago"),
            (59_500, "1m ago"),
            (180_000, "3m ago"),
            (3_599_000, "59m ago"),
            (3_600_000, "1h ago"),
            (86_399_000, "23h ago"),
            (86_400_000, "1d ago"),
            (3 * 86_400_000, "3d ago"),
            (i64::MAX, "106751991167d ago"),
        ] {
            assert_eq!(ago(ms), said, "{ms}");
        }
    }

    #[test]
    fn live_while_younger_than_a_minute() {
        assert!(is_live(1_000, 1_000));
        assert!(is_live(1_000, 60_999));
        assert!(!is_live(1_000, 61_000));
        // A clock that moved back (or a file from the future) is live.
        assert!(is_live(100_000, 0));
        assert!(!is_live(i64::MIN, i64::MAX));
    }

    #[test]
    fn urls_name_their_repo_and_number() {
        let url = "https://github.com/acme/app/pull/412";
        assert_eq!(repo_of(url), "acme/app");
        assert_eq!(number_of(url), 412);
        assert_eq!(number_of("https://github.com/acme/app/pull/7/"), 7);
        assert_eq!(repo_of("nonsense"), "");
        assert_eq!(number_of("nonsense"), 0);
    }

    fn session(id: &str, file: SessionFile, mtime: i64) -> Session {
        Session {
            id: id.into(),
            file,
            mtime,
        }
    }

    fn watching(urls: &[&str]) -> SessionFile {
        SessionFile {
            urls: urls.iter().map(|&u| u.to_owned()).collect(),
            ..SessionFile::default()
        }
    }

    fn by_mod(urls: &[&str]) -> SessionFile {
        SessionFile {
            writer: Some(Writer::Mod),
            ..watching(urls)
        }
    }

    fn open(number: u64) -> Entry {
        Entry {
            at: 1,
            pr: Some(GhPr {
                number,
                title: format!("PR {number}"),
                state: PrState::Open,
                ..GhPr::default()
            }),
            ..Entry::default()
        }
    }

    const A: &str = "https://github.com/acme/app/pull/1";
    const B: &str = "https://github.com/acme/app/pull/2";
    const C: &str = "https://github.com/acme/web/pull/3";

    #[test]
    fn all_then_a_list_per_live_session_and_the_unnamed_together() {
        let shared = Shared {
            prs: [(A, open(1)), (B, open(2)), (C, open(3))]
                .into_iter()
                .map(|(u, e)| (u.to_owned(), e))
                .collect(),
            ..Shared::default()
        };
        let named = SessionFile {
            name: Some("web".into()),
            writer: Some(Writer::Mod),
            ..watching(&[C, B])
        };
        let codex = SessionFile {
            writer: Some(Writer::Prw),
            agent: Some("codex".into()),
            ..watching(&[A])
        };
        let sessions = [
            session("old-1", watching(&[A]), 100_000),
            session("old-2", watching(&[B, A]), 100_000),
            session("s-web", named, 100_000),
            session("prw-x", codex, 100_000),
            session(
                "dead",
                watching(&["https://github.com/acme/gone/pull/9"]),
                0,
            ),
        ];
        let lists = lists_of(Some(&shared), &sessions, 120_000, None);
        let said: Vec<(&ListId, &str, Option<&str>, usize)> = lists
            .iter()
            .map(|l| (&l.id, l.label.as_str(), l.agent.as_deref(), l.rows.len()))
            .collect();
        assert_eq!(
            said,
            [
                (&ListId::All, "All", None, 3),
                (&ListId::Session("prw-x".into()), "prw-x", Some("codex"), 1),
                (&ListId::Session("s-web".into()), "web", Some("claude"), 2),
                (&ListId::Unnamed, UNNAMED, Some("claude"), 2),
            ]
        );
        let a = lists[0].rows.iter().find(|r| r.pr.url == A).unwrap();
        assert_eq!(a.agent.as_deref(), Some("codex"));
        assert_eq!(a.watchers, ["prw-x", UNNAMED, UNNAMED]);

        // Nobody live: every PR in results.json, and no session lists.
        let lists = lists_of(Some(&shared), &sessions, 10_000_000, None);
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0].rows.len(), 3);
    }

    #[test]
    fn sessions_sharing_a_name_carry_the_end_of_their_ids() {
        let in_web = |id: &str, name: &str| {
            let file = SessionFile {
                name: Some(name.into()),
                writer: Some(Writer::Mod),
                ..watching(&[A])
            };
            session(id, file, 100_000)
        };
        let sessions = [
            in_web("mgx1a2b-k3x9", "web"),
            in_web("mgx1a2c-p7qd", "Web"),
            in_web("mgx1a2d-zz01", "docs"),
        ];
        let lists = lists_of(None, &sessions, 120_000, None);
        let labels: Vec<&str> = lists.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["All", "docs", "web · k3x9", "Web · p7qd"]);
        assert_eq!(
            lists[0].rows[0].watchers,
            ["docs", "web · k3x9", "Web · p7qd"]
        );
    }

    #[test]
    fn a_url_with_no_entry_is_loading_and_a_miss_says_why() {
        let pr = tracked_of(A, None);
        assert_eq!(pr.derived.pill, PillKind::Loading);
        assert_eq!(pr.number, 1);
        assert_eq!(pr.error, None);
        let miss = Entry {
            at: 5,
            is_unreachable: true,
            ..Entry::default()
        };
        assert_eq!(tracked_of(A, Some(&miss)).error.as_deref(), Some(NOT_FOUND));
        let failed = Entry {
            error: Some("gh: HTTP 502".into()),
            ..open(1)
        };
        let pr = tracked_of(A, Some(&failed));
        assert_eq!(pr.error.as_deref(), Some("gh: HTTP 502"));
        assert_eq!(pr.title, "PR 1");
    }

    fn read(model: Model, msgs: impl IntoIterator<Item = Msg>) -> Model {
        msgs.into_iter().fold(model, |m, msg| update(m, msg).0)
    }

    #[test]
    fn the_selection_follows_its_pr_and_falls_back_to_its_place() {
        let shared = |urls: &[&str]| Shared {
            fetched_at: 1,
            next_at: 1,
            prs: urls
                .iter()
                .map(|&u| (u.to_owned(), open(number_of(u))))
                .collect(),
            ..Shared::default()
        };
        let model = read(
            Model::new(120, 30),
            [Msg::SharedRead(Ok(shared(&[A, B, C])), 0)],
        );
        assert_eq!(model.selected(), Some(A));
        let model = read(model, [Msg::Key(Key::Char('j'))]);
        assert_eq!(model.selected(), Some(B));
        // B goes: the selection takes its place.
        let model = read(model, [Msg::SharedRead(Ok(shared(&[A, C])), 1)]);
        assert_eq!(model.selected(), Some(C));
        let model = read(model, [Msg::Key(Key::Char('g'))]);
        assert_eq!(model.selected(), Some(A));
        let model = read(model, [Msg::Key(Key::End)]);
        assert_eq!(model.selected(), Some(C));
        // Nothing left.
        let model = read(model, [Msg::SharedRead(Ok(shared(&[])), 2)]);
        assert_eq!(model.selected(), None);
    }

    #[test]
    fn a_torn_read_keeps_the_last_good_copy_and_says_so() {
        let good = Shared {
            fetched_at: 1_000,
            next_at: 21_000,
            ..Shared::default()
        };
        let model = read(
            Model::new(120, 30),
            [Msg::SharedRead(Ok(good.clone()), 2_000)],
        );
        assert!(!model.header().is_read_failing);
        let model = read(
            model,
            [Msg::SharedRead(
                Err(ReadFailure::Unreadable("torn".into())),
                13_000,
            )],
        );
        assert_eq!(model.shared(), Some(&good));
        assert!(model.header().is_read_failing);
        assert_eq!(model.header().age, Some(12_000));
        assert_eq!(model.reads(), (Some(2_000), Some(13_000)));
    }

    #[test]
    fn the_header_says_who_polls() {
        let shared = Shared {
            fetched_at: 1_000,
            next_at: 21_000,
            ..Shared::default()
        };
        let poll = |sessions: Vec<Session>, now: i64| {
            read(
                Model::new(120, 30),
                [
                    Msg::SharedRead(Ok(shared.clone()), now),
                    Msg::SessionsRead(sessions, now),
                ],
            )
            .header()
            .poll
        };
        let mod_at = |mtime| session("m", watching(&[]), mtime);
        let prw_at = |mtime| {
            session(
                "prw-1",
                SessionFile {
                    writer: Some(Writer::Prw),
                    ..SessionFile::default()
                },
                mtime,
            )
        };
        assert_eq!(poll(vec![mod_at(5_000)], 10_000), Poll::Mod);
        assert_eq!(poll(vec![prw_at(5_000)], 10_000), Poll::Nobody);
        assert_eq!(poll(vec![], 10_000), Poll::Nobody);
        assert_eq!(poll(vec![mod_at(200_000)], 200_000), Poll::Stale);
        // This prw ran the round in the file: polled by prw, until it is long overdue.
        let ours = |now| {
            let shared = Shared {
                fetched_at: 1_000,
                next_at: 61_000,
                ..Shared::default()
            };
            let said = PollerSaid {
                own: "prw-1".into(),
                polled_at: Some(1_000),
                error: None,
            };
            read(
                Model::new(120, 30),
                [
                    Msg::SharedRead(Ok(shared), now),
                    Msg::SessionsRead(vec![prw_at(now)], now),
                    Msg::Poller(said),
                ],
            )
        };
        assert_eq!(ours(10_000).header().poll, Poll::Prw);
        assert_eq!(ours(61_000 + STALE_MS + 1).header().poll, Poll::Stale);
        // Its own list comes first after All.
        assert_eq!(ours(10_000).lists()[1].id, ListId::Session("prw-1".into()));
        assert_eq!(agents_live(&ours(10_000)), []);
        assert_eq!(
            read(Model::new(120, 30), [Msg::Tick(5)]).header().poll,
            Poll::Waiting
        );
    }

    #[test]
    fn keys_move_per_layout_and_quit() {
        let sessions = vec![
            session("s1", by_mod(&[A]), 0),
            session("s2", by_mod(&[B, C]), 0),
        ];
        let model = read(Model::new(120, 30), [Msg::SessionsRead(sessions, 0)]);
        assert_eq!(model.lists().len(), 3);
        assert_eq!(model.list_id(), &ListId::All);
        let model = read(model, [Msg::Key(Key::Tab)]);
        assert_eq!(model.list_id(), &ListId::Session("s1".into()));
        assert_eq!(model.selected(), Some(A));
        let model = read(model, [Msg::Key(Key::BackTab), Msg::Key(Key::BackTab)]);
        assert_eq!(model.list_id(), &ListId::Session("s2".into()));
        // Three panes: h focuses the lists, where j moves between them.
        let model = read(model, [Msg::Key(Key::Char('h')), Msg::Key(Key::Char('k'))]);
        assert_eq!(model.focus(), Focus::Lists);
        assert_eq!(model.list_id(), &ListId::Session("s1".into()));
        // Narrower: the lists are a strip, h and l step through them.
        let model = read(model, [Msg::Resize(84, 30), Msg::Key(Key::Char('l'))]);
        assert_eq!(model.focus(), Focus::Prs);
        assert_eq!(model.list_id(), &ListId::Session("s2".into()));
        let model = read(model, [Msg::Key(Key::Char('?'))]);
        assert!(model.is_help());
        let (model, cmds) = update(model, Msg::Key(Key::Esc));
        assert!(!model.is_help() && cmds.is_empty());
        assert_eq!(update(model.clone(), Msg::Key(Key::Esc)).1, [Cmd::Quit]);
        assert_eq!(
            update(model.clone(), Msg::Key(Key::Char('q'))).1,
            [Cmd::Quit]
        );
        assert_eq!(update(model, Msg::Key(Key::CtrlC)).1, [Cmd::Quit]);
    }

    #[test]
    fn a_move_that_stays_on_its_list_keeps_the_pr() {
        let model = read(
            Model::new(84, 30),
            [Msg::SessionsRead(
                vec![session("s1", by_mod(&[A, B]), 0)],
                0,
            )],
        );
        let model = read(model, [Msg::Key(Key::Tab), Msg::Key(Key::Char('j'))]);
        assert_eq!(model.list_id(), &ListId::Session("s1".into()));
        let was = model.selected().map(str::to_owned);
        // Three panes, the lists focused: G on the last list stays put.
        let model = read(
            model,
            [
                Msg::Resize(120, 30),
                Msg::Key(Key::Char('h')),
                Msg::Key(Key::Char('G')),
            ],
        );
        assert_eq!(model.selected().map(str::to_owned), was);
    }

    #[test]
    fn a_list_whose_session_dies_gives_way_to_all() {
        let sessions = vec![session("s1", by_mod(&[A]), 0)];
        let model = read(
            Model::new(50, 30),
            [Msg::SessionsRead(sessions, 0), Msg::Key(Key::Tab)],
        );
        assert_eq!(model.list_id(), &ListId::Session("s1".into()));
        let model = read(model, [Msg::Tick(60_000)]);
        assert_eq!(model.list_id(), &ListId::All);
        assert_eq!(model.selected(), None);
    }
}
