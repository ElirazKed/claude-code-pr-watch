//! What the action keys do to the screen: `m` merges behind a question, `c` opens a comment
//! field, `f` hands a PR on (copying the prompt), `o` opens it in the browser and `v` shows
//! the last prompt copied. With the PR open full size, `r` replies to the review thread
//! selected (elsewhere it comments), `x` resolves or reopens it and `a` approves behind a
//! question, where in the lists `a` switches the login. Anywhere, `X` closes the PR (or
//! reopens a closed one) and `R` starts its failed or skipped Actions runs again, each behind
//! a question. No I/O here: a key gives [`Cmd`]s (a [`Job`] for the binary's
//! worker, a copy, a browser to open), and what came of each comes back as a message.
//!
//! One question at a time, on the bottom line ([`Ask`]); what ran says so in a [`Toast`]. A
//! PR's merge, comment and hand-offs run apart, but a second press of one while it runs
//! starts nothing ([`Slot`]).

use std::collections::BTreeMap;

use unicode_width::UnicodeWidthChar;

use crate::address::short_spot;
use crate::gh::{GhPr, PrState};
use crate::handoff::{Kind, PrFacts, kinds_of};
use crate::merge::{MergeError, MergeRun, done_words, label, next_method, run_for, standing};
use crate::opened::Opened;
use crate::rerun::rerunnable;
use crate::resolve::resolve_prompt;
use crate::status::{MergeMethod, mergeable_of, summarize_checks};
use crate::ui::{Cmd, Key, Row};

/// How long a toast shows (ms).
pub const TOAST_MS: i64 = 6_000;
/// How long a toast that says something went wrong shows (ms): long enough to read twice.
pub const ERROR_MS: i64 = 12_000;

/// A run that asks GitHub, for the binary's worker: never on the UI thread, as a log may take
/// a minute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Merge {
        url: String,
        number: u64,
        run: MergeRun,
    },
    /// The text goes to gh on stdin, never in argv.
    Comment {
        url: String,
        number: u64,
        text: String,
    },
    /// `owner`'s PRs go as `login` from now on, and this one is asked again as it.
    Switch {
        url: String,
        number: u64,
        owner: String,
        login: String,
    },
    /// A reply to a review thread; the text goes to gh on stdin, never in argv.
    Reply {
        url: String,
        number: u64,
        thread: OnThread,
        text: String,
    },
    /// The thread resolved, or reopened where `resolve` is false.
    Resolve {
        url: String,
        number: u64,
        thread: OnThread,
        resolve: bool,
    },
    /// An approval with no text, of the PR whose node id is `pr`.
    Approve {
        url: String,
        number: u64,
        pr: String,
    },
    /// The PR closed, or reopened where `close` is false.
    Close {
        url: String,
        number: u64,
        close: bool,
    },
    /// The head commit's failed or skipped Actions runs started again ([`crate::rerun`]).
    Rerun { url: String, number: u64 },
    /// Read what a Fix or Address prompt needs (the checks and logs, or the threads) and write
    /// the prompt. `failing` are the card's failing checks, for when the checks won't read.
    Collect {
        kind: Kind,
        facts: PrFacts,
        failing: Vec<String>,
    },
}

impl Job {
    /// The PR it acts on.
    #[must_use]
    pub fn url(&self) -> &str {
        match self {
            Self::Merge { url, .. }
            | Self::Comment { url, .. }
            | Self::Switch { url, .. }
            | Self::Reply { url, .. }
            | Self::Resolve { url, .. }
            | Self::Approve { url, .. }
            | Self::Close { url, .. }
            | Self::Rerun { url, .. } => url,
            Self::Collect { facts, .. } => &facts.url,
        }
    }

    fn slot(&self) -> Slot {
        match self {
            Self::Merge { .. } => Slot::Merge,
            Self::Comment { .. } => Slot::Comment,
            Self::Switch { .. } => Slot::Switch,
            Self::Reply { thread, .. } => Slot::Reply(thread.id.clone()),
            Self::Resolve { thread, .. } => Slot::Resolve(thread.id.clone()),
            Self::Approve { .. } => Slot::Approve,
            Self::Close { .. } => Slot::Close,
            Self::Rerun { .. } => Slot::Rerun,
            Self::Collect { kind, .. } => Slot::Handoff(*kind),
        }
    }
}

/// A review thread, as a reply or a resolve names it: its node id, and where it is
/// ("src/upload.ts:42").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnThread {
    pub id: String,
    pub spot: String,
}

/// What came of a [`Job`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub url: String,
    pub number: u64,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Merged(MergeRun),
    MergeFailed {
        run: MergeRun,
        error: MergeError,
    },
    /// Posted, as this login.
    Commented {
        login: String,
    },
    /// Not posted (or not known to be): what the toast says.
    CommentFailed(String),
    /// The owner's PRs go as this login now.
    Switched {
        owner: String,
        login: String,
    },
    /// `login` couldn't see the PR, so the fetch put the owner back on `kept`.
    PutBack {
        owner: String,
        login: String,
        kept: String,
    },
    SwitchFailed(String),
    /// Posted, as this login.
    Replied {
        thread: OnThread,
        login: String,
    },
    /// Not posted (or not known to be): what the toast says.
    ReplyFailed {
        thread: OnThread,
        error: String,
    },
    /// Resolved, or reopened where `resolve` is false.
    Resolved {
        thread: OnThread,
        resolve: bool,
    },
    ResolveFailed {
        thread: OnThread,
        error: String,
    },
    /// Approved, as this login.
    Approved {
        login: String,
    },
    ApproveFailed(String),
    /// Closed, or reopened where `close` is false, as this login.
    Closed {
        close: bool,
        login: String,
    },
    CloseFailed(String),
    /// The runs started again (as a toast names each: "CI (failed jobs)"), and those GitHub
    /// refused, with why. Both empty where none had a job to run again.
    Reran {
        ran: Vec<String>,
        refused: Vec<String>,
    },
    RerunFailed(String),
    /// The prompt, and what didn't come for it ("No log for build: …").
    Prompt {
        kind: Kind,
        text: String,
        note: Option<String>,
    },
    PromptFailed {
        kind: Kind,
        error: String,
    },
}

impl Outcome {
    fn slot(&self) -> Slot {
        match self {
            Self::Merged(_) | Self::MergeFailed { .. } => Slot::Merge,
            Self::Commented { .. } | Self::CommentFailed(_) => Slot::Comment,
            Self::Switched { .. } | Self::PutBack { .. } | Self::SwitchFailed(_) => Slot::Switch,
            Self::Replied { thread, .. } | Self::ReplyFailed { thread, .. } => {
                Slot::Reply(thread.id.clone())
            }
            Self::Resolved { thread, .. } | Self::ResolveFailed { thread, .. } => {
                Slot::Resolve(thread.id.clone())
            }
            Self::Approved { .. } | Self::ApproveFailed(_) => Slot::Approve,
            Self::Closed { .. } | Self::CloseFailed(_) => Slot::Close,
            Self::Reran { .. } | Self::RerunFailed(_) => Slot::Rerun,
            Self::Prompt { kind, .. } | Self::PromptFailed { kind, .. } => Slot::Handoff(*kind),
        }
    }

    /// Whether it may have changed what the full view shows, so the PR open there is read
    /// again: what a refused reply, resolve or approval found counts too, as the thread may
    /// have moved on.
    #[must_use]
    pub const fn is_change(&self) -> bool {
        match self {
            Self::Merged(_)
            | Self::Commented { .. }
            | Self::Switched { .. }
            | Self::Replied { .. }
            | Self::ReplyFailed { .. }
            | Self::Resolved { .. }
            | Self::ResolveFailed { .. }
            | Self::Approved { .. }
            | Self::ApproveFailed(_)
            | Self::Closed { .. }
            | Self::CloseFailed(_)
            | Self::Reran { .. } => true,
            Self::MergeFailed { .. }
            | Self::CommentFailed(_)
            | Self::PutBack { .. }
            | Self::SwitchFailed(_)
            | Self::RerunFailed(_)
            | Self::Prompt { .. }
            | Self::PromptFailed { .. } => false,
        }
    }
}

/// Which of a PR's runs: they run apart, each at most once at a time. A reply or a resolve
/// is a thread's (by its id), so one on a thread holds up none on another.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    Merge,
    Comment,
    Switch,
    Reply(String),
    Resolve(String),
    Approve,
    Close,
    Rerun,
    Handoff(Kind),
}

/// How a toast reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tone {
    Good,
    Bad,
    Info,
}

/// A line on the bottom row for a few seconds: what a run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub text: String,
    pub tone: Tone,
    /// When it goes (ms).
    pub until: i64,
}

/// The question on the bottom line, and the keys it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `y` runs it, `n` doesn't, `m` tries the next method.
    Merge {
        url: String,
        number: u64,
        base: String,
        run: MergeRun,
        /// The methods the repo takes, for `m`.
        methods: Vec<MergeMethod>,
    },
    /// The comment being written: Enter posts it, Esc drops it.
    Comment {
        url: String,
        number: u64,
        text: String,
        /// Who it posts as, where `results.json` says ("" where it doesn't).
        login: String,
        /// The review thread it replies on, where it is a reply (`r`).
        thread: Option<OnThread>,
    },
    /// `y` approves it, `n` doesn't.
    Approve {
        url: String,
        number: u64,
        /// The PR's node id.
        pr: String,
        /// Who it approves as ("" where nothing says).
        login: String,
    },
    /// `y` closes it (or reopens it, where `close` is false), `n` doesn't.
    Close {
        url: String,
        number: u64,
        close: bool,
    },
    /// `y` starts its failed or skipped Actions runs again, `n` doesn't.
    Rerun {
        url: String,
        number: u64,
        /// The checks that ended so, for the question.
        names: Vec<String>,
    },
    /// A PR with more than one hand-off: each one's key runs it.
    Pick {
        url: String,
        number: u64,
        kinds: Vec<Kind>,
    },
}

impl Ask {
    /// The PR it asks about.
    #[must_use]
    pub fn url(&self) -> &str {
        match self {
            Self::Merge { url, .. }
            | Self::Comment { url, .. }
            | Self::Approve { url, .. }
            | Self::Close { url, .. }
            | Self::Rerun { url, .. }
            | Self::Pick { url, .. } => url,
        }
    }
}

/// The last prompt copied, for `v`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copied {
    pub kind: Kind,
    pub number: u64,
    pub text: String,
    /// Where to paste it: "Claude (web-checkout)", or "your agent".
    pub to: String,
    /// What didn't come for it, said once it is copied.
    pub note: Option<String>,
}

/// One thing a PR offers, for the actions list: its key and what it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub key: &'static str,
    pub words: String,
    /// A word or two, where there is no room for `words` ("merge", "fix").
    pub short: String,
    pub look: Look,
}

/// How an offer is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Look {
    Merge,
    Handoff,
    Plain,
    /// Running now.
    Busy,
}

/// The selected PR, as an action key needs it.
#[derive(Debug, Clone)]
pub struct Target<'a> {
    pub row: &'a Row,
    /// Its data, where a round has read it.
    pub pr: Option<&'a GhPr>,
    /// The login that acts on its repo, as the poll picks it ([`login_for`]: "" where
    /// nothing says).
    ///
    /// [`login_for`]: crate::accounts::login_for
    pub login: String,
    /// The login a switch moves its owner to, where gh has another.
    pub next: Option<String>,
    /// The PR as the full view has it, while it is open there: what `r`, `x` and `a` act on.
    pub opened: Option<&'a Opened>,
}

impl Target<'_> {
    fn url(&self) -> &str {
        &self.row.pr.url
    }

    fn number(&self) -> u64 {
        self.row.pr.number
    }

    /// Where its prompt goes: the agent whose session watches it.
    fn to(&self) -> String {
        match (&self.row.agent, self.row.watchers.first()) {
            (Some(agent), Some(session)) => format!("{} ({session})", agent_name(agent)),
            (Some(agent), None) => agent_name(agent),
            (None, _) => "your agent".to_owned(),
        }
    }
}

/// An agent as a sentence names it: `claude` is Claude.
#[must_use]
pub fn agent_name(agent: &str) -> String {
    let mut chars = agent.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// What the action keys left on the screen.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Acts {
    ask: Option<Ask>,
    /// What runs now, by PR and slot: the status line's words for it.
    busy: BTreeMap<(String, Slot), String>,
    toast: Option<Toast>,
    /// A comment that didn't post (or wasn't known to), by PR (a reply by its thread's id):
    /// `c` (or `r`) brings it back.
    drafts: BTreeMap<String, String>,
    /// Methods GitHub refused for a repo: not offered again this run.
    refused: BTreeMap<String, Vec<MergeMethod>>,
    copied: Option<Copied>,
    /// Where each prompt being collected goes, by PR and kind.
    pending: BTreeMap<(String, Kind), String>,
    /// The `v` pane's first line, while it shows.
    view: Option<usize>,
    /// The login `a` moved each owner to that the fetch put back, so the next press goes on
    /// past it ([`switch_to`](crate::accounts::switch_to)); gone once a switch takes.
    tried: BTreeMap<String, String>,
}

impl Acts {
    /// The question on the bottom line.
    #[must_use]
    pub const fn ask(&self) -> Option<&Ask> {
        self.ask.as_ref()
    }

    /// The toast, while it shows.
    #[must_use]
    pub const fn toast(&self) -> Option<&Toast> {
        self.toast.as_ref()
    }

    /// What runs now, the status line's words for each.
    pub fn busy(&self) -> impl Iterator<Item = &str> {
        self.busy.values().map(String::as_str)
    }

    /// The last prompt copied.
    #[must_use]
    pub const fn copied(&self) -> Option<&Copied> {
        self.copied.as_ref()
    }

    /// The login `a` last moved `owner` to that the fetch put back, this run.
    #[must_use]
    pub fn tried(&self, owner: &str) -> Option<&str> {
        self.tried.get(owner).map(String::as_str)
    }

    /// The `v` pane's first line, while it shows.
    #[must_use]
    pub const fn view(&self) -> Option<usize> {
        self.view
    }

    /// Keeps the `v` pane's first line where the prompt still fills a `width` × `height`
    /// screen (the pane takes it whole, less its head and foot), so a key past the end, or
    /// `G`, leaves it at the end rather than beyond it.
    pub fn clamp_view(&mut self, width: usize, height: usize) {
        if let (Some(top), Some(copied)) = (self.view, &self.copied) {
            let body = prompt_body(&copied.text, width).len();
            self.view = Some(top.min(body.saturating_sub(height.saturating_sub(2))));
        }
    }

    /// Whether the keys are the bottom line's or the `v` pane's: then they type, answer or
    /// scroll, and no other key acts.
    #[must_use]
    pub const fn is_holding_keys(&self) -> bool {
        self.ask.is_some() || self.view.is_some()
    }

    pub(crate) fn say(&mut self, text: impl Into<String>, tone: Tone, now: i64) {
        let ms = if tone == Tone::Bad {
            ERROR_MS
        } else {
            TOAST_MS
        };
        self.toast = Some(Toast {
            text: text.into(),
            tone,
            until: now + ms,
        });
    }

    /// The time moved on: a toast past its time goes.
    pub fn tick(&mut self, now: i64) {
        if self.toast.as_ref().is_some_and(|t| t.until <= now) {
            self.toast = None;
        }
    }

    /// The methods a PR's repo still takes this run.
    fn methods(&self, repo: &str, pr: &GhPr) -> Vec<MergeMethod> {
        let refused = self
            .refused
            .get(repo)
            .map(Vec::as_slice)
            .unwrap_or_default();
        mergeable_of(pr)
            .methods
            .into_iter()
            .filter(|m| !refused.contains(m))
            .collect()
    }

    /// What the selected PR offers, for its actions list, most useful first.
    #[must_use]
    pub fn offers(&self, target: &Target<'_>) -> Vec<Offer> {
        let url = target.url();
        let is_busy = |slot| self.busy.get(&(url.to_owned(), slot));
        let offer = |key, words: String, short: &str, look| Offer {
            key,
            words,
            short: short.to_owned(),
            look,
        };
        let busy = |key, words: &String| offer(key, format!("{words}…"), "…", Look::Busy);
        let mut offers = Vec::new();
        if let Some(pr) = target.pr {
            if let Some(words) = is_busy(Slot::Merge) {
                offers.push(busy("m", words));
            } else if let Some(run) =
                run_for(&mergeable_of(pr), &self.methods(&target.row.repo, pr), None)
            {
                let short = match run {
                    MergeRun::Merge(_) => "merge",
                    MergeRun::Auto(_) => "auto-merge",
                    MergeRun::CancelAuto => "cancel auto",
                };
                offers.push(offer("m", label(run), short, Look::Merge));
            }
            let kinds = kinds_of(pr);
            if let Some(words) = kinds.iter().find_map(|&k| is_busy(Slot::Handoff(k))) {
                offers.push(busy("f", words));
            } else if !kinds.is_empty() {
                let names: Vec<&str> = kinds.iter().map(|k| k.name()).collect();
                let short = match kinds.as_slice() {
                    [kind] => kind.name().to_lowercase(),
                    _ => "hand off".to_owned(),
                };
                let words = format!("{} → copy for {}", names.join(", "), target.to());
                offers.push(offer("f", words, &short, Look::Handoff));
            }
        }
        if let Some(opened) = target.opened {
            if let Some(thread) = opened.thread() {
                let spot = short_spot(thread);
                offers.push(match is_busy(Slot::Reply(thread.id.clone())) {
                    Some(words) => busy("r", words),
                    None => offer("r", format!("Reply on {spot}"), "reply", Look::Plain),
                });
                offers.push(match is_busy(Slot::Resolve(thread.id.clone())) {
                    Some(words) => busy("x", words),
                    None if thread.is_resolved => {
                        offer("x", format!("Reopen {spot}"), "reopen", Look::Plain)
                    }
                    None => offer("x", format!("Resolve {spot}"), "resolve", Look::Plain),
                });
            }
            if let Some(words) = is_busy(Slot::Approve) {
                offers.push(busy("a", words));
            } else if approvable(target).is_ok() {
                let words = if target.login.is_empty() {
                    "Approve".to_owned()
                } else {
                    format!("Approve as {}", target.login)
                };
                offers.push(offer("a", words, "approve", Look::Merge));
            }
        }
        offers.push(match is_busy(Slot::Comment) {
            Some(words) => busy("c", words),
            None if target.login.is_empty() => {
                offer("c", "Comment".to_owned(), "comment", Look::Plain)
            }
            None => offer(
                "c",
                format!("Comment as {}", target.login),
                "comment",
                Look::Plain,
            ),
        });
        // In the full view `a` approves: the switch is the lists'.
        if target.opened.is_none() {
            if let Some(words) = is_busy(Slot::Switch) {
                offers.push(busy("a", words));
            } else if let Some(next) = &target.next {
                let words = format!("As {} → switch to {next}", target.login);
                offers.push(offer("a", words, "account", Look::Plain));
            }
        }
        offers.extend(self.state_offers(target));
        offers.push(offer(
            "o",
            "Open on GitHub ↗".to_owned(),
            "open",
            Look::Plain,
        ));
        if let Some(copied) = &self.copied {
            let words = format!(
                "Show the {} prompt for #{}",
                copied.kind.name(),
                copied.number
            );
            offers.push(offer("v", words, "view", Look::Plain));
        }
        offers
    }

    /// `R` and `X`: what the PR's checks and state offer.
    fn state_offers(&self, target: &Target<'_>) -> Vec<Offer> {
        let is_busy = |slot| self.busy.get(&(target.url().to_owned(), slot));
        let offer = |key, words: String, short: &str| Offer {
            key,
            words,
            short: short.to_owned(),
            look: Look::Plain,
        };
        let busy = |key, words: &String| Offer {
            key,
            words: format!("{words}…"),
            short: "…".to_owned(),
            look: Look::Busy,
        };
        let mut found = Vec::new();
        if let Some(pr) = target.pr.filter(|pr| pr.state == PrState::Open) {
            if let Some(words) = is_busy(Slot::Rerun) {
                found.push(busy("R", words));
            } else {
                let names = rerunnable(pr.checks());
                if !names.is_empty() {
                    let words = format!("Run {} again", counted_checks(&names));
                    found.push(offer("R", words, "re-run"));
                }
            }
        }
        if let Some(words) = is_busy(Slot::Close) {
            found.push(busy("X", words));
        } else {
            match target.pr.map(|pr| &pr.state) {
                Some(PrState::Open) => {
                    found.push(offer("X", "Close the PR".to_owned(), "close"));
                }
                Some(PrState::Closed) => {
                    found.push(offer("X", "Reopen the PR".to_owned(), "reopen"));
                }
                _ => {}
            }
        }
        found
    }

    /// What a key does, where it is an action key or the keys are held; `None` leaves it to
    /// the lists. Ctrl-C always quits.
    pub fn key(&mut self, key: Key, target: Option<Target<'_>>, now: i64) -> Option<Vec<Cmd>> {
        if key == Key::CtrlC {
            return None;
        }
        if let Some(top) = self.view {
            self.view = match key {
                Key::Esc | Key::Char('q' | 'v') => None,
                Key::Char('j') | Key::Down => Some(top.saturating_add(1)),
                Key::Char('k') | Key::Up => Some(top.saturating_sub(1)),
                Key::Char('g') | Key::Home => Some(0),
                Key::Char('G') | Key::End => Some(usize::MAX),
                _ => Some(top),
            };
            return Some(Vec::new());
        }
        if let Some(ask) = self.ask.take() {
            return Some(self.answer(ask, key, target, now));
        }
        let cmds = match key {
            Key::Char('m') => self.merge(&target?, now),
            Key::Char('c') => self.comment(target?),
            Key::Char('f') => self.hand_off(&target?, now),
            Key::Char('o') => vec![Cmd::Open(target?.url().to_owned())],
            Key::Char('r') => {
                let target = target?;
                match target.opened.and_then(Opened::thread) {
                    Some(thread) => {
                        let on = on_thread(thread);
                        self.ask = Some(Ask::Comment {
                            text: self.draft(target.url(), &on.id, Slot::Reply(on.id.clone())),
                            url: target.url().to_owned(),
                            number: target.number(),
                            login: target.login,
                            thread: Some(on),
                        });
                        Vec::new()
                    }
                    // Not on a thread, or in the lists: a reply to the PR is a comment.
                    None if target.opened.is_some() => self.comment(target),
                    None => return None,
                }
            }
            Key::Char('x') => self.resolve(&target?, now)?,
            Key::Char('X') => self.close(&target?, now),
            Key::Char('R') => self.rerun(&target?, now),
            Key::Char('a') => {
                let target = target?;
                if target.opened.is_some() {
                    self.approve(&target, now)
                } else {
                    self.switch(&target, now)
                }
            }
            Key::Char('v') => {
                if self.copied.is_some() {
                    self.view = Some(0);
                } else {
                    self.say(
                        "Nothing copied yet: f copies a PR's hand-off",
                        Tone::Info,
                        now,
                    );
                }
                Vec::new()
            }
            _ => return None,
        };
        Some(cmds)
    }

    /// Text pasted into the terminal: into the comment field, where one is open.
    pub fn paste(&mut self, text: &str) {
        if let Some(Ask::Comment { text: field, .. }) = &mut self.ask {
            field.push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        }
    }

    /// A key while a question is open.
    fn answer(&mut self, ask: Ask, key: Key, target: Option<Target<'_>>, now: i64) -> Vec<Cmd> {
        match ask {
            Ask::Merge {
                url,
                number,
                base,
                run,
                methods,
            } => match key {
                Key::Char('y') | Key::Enter => self.start(Job::Merge { url, number, run }, now),
                Key::Char('m') | Key::Tab if run.method().is_some() && methods.len() > 1 => {
                    let method = next_method(&methods, run.method().unwrap_or(methods[0]));
                    self.ask = Some(Ask::Merge {
                        url,
                        number,
                        base,
                        run: run.by(method),
                        methods,
                    });
                    Vec::new()
                }
                Key::Char('n') | Key::Esc => Vec::new(),
                _ => {
                    self.ask = Some(Ask::Merge {
                        url,
                        number,
                        base,
                        run,
                        methods,
                    });
                    Vec::new()
                }
            },
            ask @ Ask::Comment { .. } => self.write(ask, key, now),
            Ask::Approve {
                url,
                number,
                pr,
                login,
            } => match key {
                Key::Char('y') | Key::Enter => self.start(Job::Approve { url, number, pr }, now),
                Key::Char('n') | Key::Esc => Vec::new(),
                _ => {
                    self.ask = Some(Ask::Approve {
                        url,
                        number,
                        pr,
                        login,
                    });
                    Vec::new()
                }
            },
            Ask::Close { url, number, close } => match key {
                Key::Char('y') | Key::Enter => self.start(Job::Close { url, number, close }, now),
                Key::Char('n') | Key::Esc => Vec::new(),
                _ => {
                    self.ask = Some(Ask::Close { url, number, close });
                    Vec::new()
                }
            },
            Ask::Rerun { url, number, names } => match key {
                Key::Char('y') | Key::Enter => self.start(Job::Rerun { url, number }, now),
                Key::Char('n') | Key::Esc => Vec::new(),
                _ => {
                    self.ask = Some(Ask::Rerun { url, number, names });
                    Vec::new()
                }
            },
            Ask::Pick { url, number, kinds } => {
                if matches!(key, Key::Esc | Key::Char('n')) {
                    return Vec::new();
                }
                let chosen = kinds.iter().copied().find(|k| key == Key::Char(k.key()));
                match (chosen, target) {
                    (Some(kind), Some(target)) if target.url() == url => {
                        self.run_handoff(kind, &target, now)
                    }
                    // The PR is gone from the lists: so is the question.
                    (Some(_), _) => Vec::new(),
                    (None, _) => {
                        self.ask = Some(Ask::Pick { url, number, kinds });
                        Vec::new()
                    }
                }
            }
        }
    }

    /// A key in the comment (or reply) field: it types, Enter posts, Esc drops it.
    fn write(&mut self, ask: Ask, key: Key, now: i64) -> Vec<Cmd> {
        let Ask::Comment {
            url,
            number,
            mut text,
            login,
            thread,
        } = ask
        else {
            return Vec::new();
        };
        let draft = thread.as_ref().map_or(&url, |t| &t.id).clone();
        match key {
            Key::Esc => {
                self.drafts.remove(&draft);
                return Vec::new();
            }
            Key::Enter if crate::comment::is_postable(&text) => {
                let slot = thread
                    .as_ref()
                    .map_or(Slot::Comment, |t| Slot::Reply(t.id.clone()));
                if let Some(words) = self.busy.get(&(url.clone(), slot)) {
                    // The field stays open: the one posting now clears the draft once it
                    // posts, so this text would be lost with it.
                    let words = format!("Still {}…", lower_first(words));
                    self.say(words, Tone::Info, now);
                    self.ask = Some(Ask::Comment {
                        url,
                        number,
                        text,
                        login,
                        thread,
                    });
                    return Vec::new();
                }
                // Kept until it posts: a refusal brings it back.
                self.drafts.insert(draft, text.clone());
                let job = match thread {
                    Some(thread) => Job::Reply {
                        url,
                        number,
                        thread,
                        text,
                    },
                    None => Job::Comment { url, number, text },
                };
                return self.start(job, now);
            }
            Key::Newline => text.push('\n'),
            Key::Backspace => {
                text.pop();
            }
            Key::Char(c) => text.push(c),
            _ => {}
        }
        self.ask = Some(Ask::Comment {
            url,
            number,
            text,
            login,
            thread,
        });
        Vec::new()
    }

    /// Marks a job's slot busy and hands it to the worker; a slot already running starts
    /// nothing.
    fn start(&mut self, job: Job, now: i64) -> Vec<Cmd> {
        let at = (job.url().to_owned(), job.slot());
        if let Some(words) = self.busy.get(&at) {
            let words = format!("Still {}…", lower_first(words));
            self.say(words, Tone::Info, now);
            return Vec::new();
        }
        let words = match &job {
            Job::Merge { number, run, .. } => format!("{} #{number}", run.busy()),
            Job::Comment { number, .. } => format!("Posting the comment on #{number}"),
            Job::Switch { owner, login, .. } => format!("Moving {owner} to {login}"),
            Job::Reply { thread, .. } => format!("Posting the reply on {}", thread.spot),
            Job::Resolve {
                thread,
                resolve: true,
                ..
            } => format!("Resolving {}", thread.spot),
            Job::Resolve { thread, .. } => format!("Reopening {}", thread.spot),
            Job::Approve { number, .. } => format!("Approving #{number}"),
            Job::Close {
                number,
                close: true,
                ..
            } => format!("Closing #{number}"),
            Job::Close { number, .. } => format!("Reopening #{number}"),
            Job::Rerun { number, .. } => format!("Starting #{number}'s checks again"),
            Job::Collect {
                kind: Kind::Fix,
                facts,
                ..
            } => format!("Reading #{}'s failing checks and their logs", facts.number),
            Job::Collect { facts, .. } => format!("Reading #{}'s review threads", facts.number),
        };
        self.busy.insert(at, words);
        vec![Cmd::Run(job)]
    }

    fn merge(&mut self, target: &Target<'_>, now: i64) -> Vec<Cmd> {
        let number = target.number();
        let Some(pr) = target.pr else {
            self.say(format!("#{number} hasn't been read yet"), Tone::Info, now);
            return Vec::new();
        };
        let merge = mergeable_of(pr);
        let methods = self.methods(&target.row.repo, pr);
        let Some(run) = run_for(&merge, &methods, None) else {
            let why = match standing(pr, &merge) {
                None if pr.state == PrState::Merged => format!("#{number} is merged"),
                None => format!("#{number} is closed"),
                Some(_) if merge.is_auto => {
                    format!("#{number} auto-merges; this account can't turn it off")
                }
                Some(_) if !merge.methods.is_empty() && methods.is_empty() => {
                    format!(
                        "GitHub refused every merge method for {} this run",
                        target.row.repo
                    )
                }
                Some((words, _)) => format!("#{number} can't be merged now: {words}"),
            };
            self.say(why, Tone::Info, now);
            return Vec::new();
        };
        self.ask = Some(Ask::Merge {
            url: target.url().to_owned(),
            number,
            base: pr.base_ref_name.clone(),
            run,
            methods,
        });
        Vec::new()
    }

    /// The text a field opens with: what didn't post (by `key`), but not while it posts now
    /// (`url`'s `slot`), so it is never sent twice.
    fn draft(&self, url: &str, key: &str, slot: Slot) -> String {
        if self.busy.contains_key(&(url.to_owned(), slot)) {
            return String::new();
        }
        self.drafts.get(key).cloned().unwrap_or_default()
    }

    fn comment(&mut self, target: Target<'_>) -> Vec<Cmd> {
        let url = target.url().to_owned();
        self.ask = Some(Ask::Comment {
            text: self.draft(&url, &url, Slot::Comment),
            url,
            number: target.number(),
            login: target.login,
            thread: None,
        });
        Vec::new()
    }

    /// The thread selected in the full view resolved, or reopened where it is resolved;
    /// `None` in the lists, where `x` is no key.
    fn resolve(&mut self, target: &Target<'_>, now: i64) -> Option<Vec<Cmd>> {
        let opened = target.opened?;
        let Some(thread) = opened.thread() else {
            self.say(
                "Pick a review thread with j/k first: x resolves it",
                Tone::Info,
                now,
            );
            return Some(Vec::new());
        };
        let job = Job::Resolve {
            url: target.url().to_owned(),
            number: target.number(),
            thread: on_thread(thread),
            resolve: !thread.is_resolved,
        };
        Some(self.start(job, now))
    }

    /// The question before an approval, or why there is none to give.
    fn approve(&mut self, target: &Target<'_>, now: i64) -> Vec<Cmd> {
        match approvable(target) {
            Ok(pr) => {
                self.ask = Some(Ask::Approve {
                    url: target.url().to_owned(),
                    number: target.number(),
                    pr: pr.to_owned(),
                    login: target.login.clone(),
                });
            }
            Err(why) => self.say(why, Tone::Info, now),
        }
        Vec::new()
    }

    /// `X`: the PR closed, or reopened where it is closed, behind a question.
    fn close(&mut self, target: &Target<'_>, now: i64) -> Vec<Cmd> {
        let number = target.number();
        let close = match target.pr.map(|pr| &pr.state) {
            None => {
                self.say(format!("#{number} hasn't been read yet"), Tone::Info, now);
                return Vec::new();
            }
            Some(PrState::Merged) => {
                self.say(format!("#{number} is merged"), Tone::Info, now);
                return Vec::new();
            }
            Some(state) => *state == PrState::Open,
        };
        self.ask = Some(Ask::Close {
            url: target.url().to_owned(),
            number,
            close,
        });
        Vec::new()
    }

    /// `R`: the PR's failed or skipped Actions runs started again, behind a question.
    fn rerun(&mut self, target: &Target<'_>, now: i64) -> Vec<Cmd> {
        let number = target.number();
        let Some(pr) = target.pr else {
            self.say(format!("#{number} hasn't been read yet"), Tone::Info, now);
            return Vec::new();
        };
        if pr.state != PrState::Open {
            let state = if pr.state == PrState::Merged {
                "merged"
            } else {
                "closed"
            };
            self.say(format!("#{number} is {state}"), Tone::Info, now);
            return Vec::new();
        }
        let names = rerunnable(pr.checks());
        if names.is_empty() {
            self.say(
                format!("#{number} has no failed or skipped Actions check"),
                Tone::Info,
                now,
            );
            return Vec::new();
        }
        self.ask = Some(Ask::Rerun {
            url: target.url().to_owned(),
            number,
            names,
        });
        Vec::new()
    }

    /// The PR's owner moved to gh's next login, where there is one.
    fn switch(&mut self, target: &Target<'_>, now: i64) -> Vec<Cmd> {
        let Some(login) = target.next.clone() else {
            self.say("One gh login: nothing to switch to", Tone::Info, now);
            return Vec::new();
        };
        let owner = target.row.repo.split('/').next().unwrap_or_default();
        let job = Job::Switch {
            url: target.url().to_owned(),
            number: target.number(),
            owner: owner.to_owned(),
            login,
        };
        self.start(job, now)
    }

    fn hand_off(&mut self, target: &Target<'_>, now: i64) -> Vec<Cmd> {
        let number = target.number();
        let kinds = target.pr.map(kinds_of).unwrap_or_default();
        match kinds.as_slice() {
            [] => {
                let why = match target.pr.map(|pr| &pr.state) {
                    None => "it hasn't been read yet",
                    Some(PrState::Open) => "no conflict, failing check or open thread",
                    Some(_) => "it is finished",
                };
                self.say(
                    format!("Nothing to hand off on #{number}: {why}"),
                    Tone::Info,
                    now,
                );
                Vec::new()
            }
            [kind] => self.run_handoff(*kind, target, now),
            _ => {
                self.ask = Some(Ask::Pick {
                    url: target.url().to_owned(),
                    number,
                    kinds,
                });
                Vec::new()
            }
        }
    }

    fn run_handoff(&mut self, kind: Kind, target: &Target<'_>, now: i64) -> Vec<Cmd> {
        let Some(pr) = target.pr else {
            return Vec::new();
        };
        let facts = PrFacts::of(target.url(), pr);
        if kind == Kind::Resolve {
            // Nothing to read: GitHub names no conflicting file.
            return self.copy(Copied {
                kind,
                number: facts.number,
                text: resolve_prompt(&facts),
                to: target.to(),
                note: None,
            });
        }
        let failing = summarize_checks(pr.checks()).failing;
        let at = (facts.url.clone(), kind);
        let cmds = self.start(
            Job::Collect {
                kind,
                facts,
                failing,
            },
            now,
        );
        // Only a job that starts names whom its prompt is for; a refused press keeps the
        // running one's.
        if !cmds.is_empty() {
            self.pending.insert(at, target.to());
        }
        cmds
    }

    /// Keeps a prompt for `v` and asks the binary to copy it.
    fn copy(&mut self, copied: Copied) -> Vec<Cmd> {
        let text = copied.text.clone();
        self.copied = Some(copied);
        vec![Cmd::Copy(text)]
    }

    /// What came of a job.
    pub fn done(&mut self, done: Done, now: i64) -> Vec<Cmd> {
        let Done {
            url,
            number,
            outcome,
        } = done;
        self.busy.remove(&(url.clone(), outcome.slot()));
        match outcome {
            Outcome::Merged(run) => {
                self.say(format!("✓ {}", done_words(run, number)), Tone::Good, now);
            }
            Outcome::MergeFailed { run, error } => {
                if error.is_method_refused
                    && let Some(method) = run.method()
                {
                    let repo = crate::ui::repo_of(&url);
                    let refused = self.refused.entry(repo).or_default();
                    if !refused.contains(&method) {
                        refused.push(method);
                    }
                }
                self.say(format!("✗ #{number}: {}", error.message), Tone::Bad, now);
            }
            Outcome::Commented { login } => {
                self.drafts.remove(&url);
                self.say(
                    format!("✓ Commented on #{number}{}", as_login(&login)),
                    Tone::Good,
                    now,
                );
            }
            Outcome::Switched { owner, login } => {
                self.tried.remove(&owner);
                self.say(format!("✓ {owner}'s PRs go as {login}"), Tone::Good, now);
            }
            Outcome::PutBack { owner, login, kept } => {
                let text = format!("✗ {login} can't see it: {owner}'s PRs stay as {kept}");
                self.tried.insert(owner, login);
                self.say(text, Tone::Bad, now);
            }
            Outcome::Replied { thread, login } => {
                self.drafts.remove(&thread.id);
                self.say(
                    format!("✓ Replied on {}{}", thread.spot, as_login(&login)),
                    Tone::Good,
                    now,
                );
            }
            Outcome::ReplyFailed { error, .. } => {
                self.say(
                    format!("✗ {error} · r brings your text back"),
                    Tone::Bad,
                    now,
                );
            }
            Outcome::Resolved {
                thread,
                resolve: true,
            } => self.say(format!("✓ Resolved {}", thread.spot), Tone::Good, now),
            Outcome::Resolved { thread, .. } => {
                self.say(format!("● Reopened {}", thread.spot), Tone::Good, now);
            }
            outcome @ (Outcome::Approved { .. }
            | Outcome::ApproveFailed(_)
            | Outcome::Closed { .. }
            | Outcome::CloseFailed(_)
            | Outcome::Reran { .. }
            | Outcome::RerunFailed(_)) => self.say_state(number, outcome, now),
            Outcome::SwitchFailed(error) | Outcome::ResolveFailed { error, .. } => {
                self.say(format!("✗ #{number}: {error}"), Tone::Bad, now);
            }
            Outcome::CommentFailed(error) => {
                self.say(
                    format!("✗ {error} · c brings your text back"),
                    Tone::Bad,
                    now,
                );
            }
            Outcome::Prompt { kind, text, note } => {
                let to = self
                    .pending
                    .remove(&(url, kind))
                    .unwrap_or_else(|| "your agent".to_owned());
                return self.copy(Copied {
                    kind,
                    number,
                    text,
                    to,
                    note,
                });
            }
            Outcome::PromptFailed { kind, error } => {
                self.pending.remove(&(url, kind));
                self.say(format!("✗ #{number}: {error}"), Tone::Bad, now);
            }
        }
        Vec::new()
    }

    /// What the toast says of a change to the PR itself: `a`'s approval, `X`'s close or
    /// reopen, and the runs `R` started again and those GitHub refused.
    fn say_state(&mut self, number: u64, outcome: Outcome, now: i64) {
        let (text, tone) = match outcome {
            Outcome::Approved { login } => (
                format!("✓ Approved #{number}{}", as_login(&login)),
                Tone::Good,
            ),
            Outcome::Closed { close, login } => {
                let done = if close { "✓ Closed" } else { "● Reopened" };
                (format!("{done} #{number}{}", as_login(&login)), Tone::Good)
            }
            Outcome::Reran { ran, refused } => match (ran.is_empty(), refused.is_empty()) {
                (true, true) => (
                    format!("#{number} has no finished run with a job to run again"),
                    Tone::Info,
                ),
                (false, true) => (
                    format!("✓ Started again on #{number}: {}", ran.join(", ")),
                    Tone::Good,
                ),
                (true, false) => (format!("✗ #{number}: {}", refused.join("; ")), Tone::Bad),
                (false, false) => (
                    format!(
                        "Started again on #{number}: {} · ✗ {}",
                        ran.join(", "),
                        refused.join("; ")
                    ),
                    Tone::Bad,
                ),
            },
            Outcome::ApproveFailed(error)
            | Outcome::CloseFailed(error)
            | Outcome::RerunFailed(error) => (format!("✗ #{number}: {error}"), Tone::Bad),
            _ => return,
        };
        self.say(text, tone, now);
    }

    /// The copy the last prompt asked for: by which means, or why it failed.
    pub fn copied_by(&mut self, by: Result<String, String>, now: i64) {
        let Some(copied) = &self.copied else {
            return;
        };
        let what = format!("{} prompt for #{}", copied.kind.name(), copied.number);
        let note = copied
            .note
            .as_ref()
            .map_or(String::new(), |note| format!(" · {note}"));
        match by {
            Ok(how) => {
                // The note before the keys, so a narrow line cuts the keys.
                let text = format!(
                    "✓ {what} copied ({how}){note} · paste it into {} · v shows it",
                    copied.to
                );
                let tone = if note.is_empty() {
                    Tone::Good
                } else {
                    Tone::Info
                };
                self.say(text, tone, now);
            }
            Err(error) => self.say(
                format!("✗ Couldn't copy the {what}: {error} · v shows it to copy by hand"),
                Tone::Bad,
                now,
            ),
        }
    }

    /// The browser the last `o` asked for: opened, or why not.
    pub fn opened(&mut self, url: &str, by: Result<(), String>, now: i64) {
        match by {
            Ok(()) => self.say(
                format!("↗ Opened #{} in the browser", crate::ui::number_of(url)),
                Tone::Good,
                now,
            ),
            Err(error) => self.say(
                format!("✗ Couldn't open the browser ({error}): {url}"),
                Tone::Bad,
                now,
            ),
        }
    }
}

/// The prompt as the `v` pane shows it, a line a row: tabs as spaces, each line cut into
/// pieces of `width` cells so it shows whole.
#[must_use]
pub fn prompt_body(text: &str, width: usize) -> Vec<String> {
    text.replace('\t', "    ")
        .split('\n')
        .flat_map(|line| hard_wrap(line, width))
        .collect()
}

/// `line` cut into pieces of `width` cells, for text that must show whole.
fn hard_wrap(line: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut used = 0;
    for c in line.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width.max(1) {
            out.push(String::new());
            used = 0;
        }
        if let Some(last) = out.last_mut() {
            last.push(c);
        }
        used += w;
    }
    out
}

/// The PR's node id, where `a` can approve it, else why not: it must be read, open, and not
/// the login's own.
fn approvable<'a>(target: &Target<'a>) -> Result<&'a str, String> {
    let number = target.number();
    let full = target
        .opened
        .and_then(Opened::full)
        .filter(|f| !f.id.is_empty());
    let Some(full) = full else {
        return Err(format!("#{number} hasn't been read yet"));
    };
    match target.pr.map(|pr| &pr.state) {
        Some(PrState::Merged) => return Err(format!("#{number} is merged")),
        Some(PrState::Closed) => return Err(format!("#{number} is closed")),
        _ => {}
    }
    let author = full.author.as_ref().map(|a| a.login.as_str());
    // GitHub logins are case-insensitive.
    if !target.login.is_empty() && author.is_some_and(|a| a.eq_ignore_ascii_case(&target.login)) {
        return Err(format!(
            "#{number} is {}'s own: GitHub doesn't let its author approve it",
            target.login
        ));
    }
    Ok(&full.id)
}

fn on_thread(thread: &crate::address::GhThread) -> OnThread {
    OnThread {
        id: thread.id.clone(),
        spot: short_spot(thread),
    }
}

/// "build" alone, else "3 checks".
#[must_use]
pub fn counted_checks(names: &[String]) -> String {
    match names {
        [one] => one.clone(),
        _ => format!("{} checks", names.len()),
    }
}

/// " as hubot", or nothing where no login is known.
#[must_use]
pub fn as_login(login: &str) -> String {
    if login.is_empty() {
        String::new()
    } else {
        format!(" as {login}")
    }
}

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}
