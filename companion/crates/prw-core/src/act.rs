//! What the action keys do to the screen: `m` merges behind a question, `c` opens a comment
//! field, `f` hands a PR on (copying the prompt), `o` opens it in the browser and `v` shows
//! the last prompt copied. No I/O here: a key gives [`Cmd`]s (a [`Job`] for the binary's
//! worker, a copy, a browser to open), and what came of each comes back as a message.
//!
//! One question at a time, on the bottom line ([`Ask`]); what ran says so in a [`Toast`]. A
//! PR's merge, comment and hand-offs run apart, but a second press of one while it runs
//! starts nothing ([`Slot`]).

use std::collections::BTreeMap;

use crate::gh::{GhPr, PrState};
use crate::handoff::{Kind, PrFacts, kinds_of};
use crate::merge::{MergeError, MergeRun, done_words, label, next_method, run_for, standing};
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
            Self::Merge { url, .. } | Self::Comment { url, .. } => url,
            Self::Collect { facts, .. } => &facts.url,
        }
    }

    const fn slot(&self) -> Slot {
        match self {
            Self::Merge { .. } => Slot::Merge,
            Self::Comment { .. } => Slot::Comment,
            Self::Collect { kind, .. } => Slot::Handoff(*kind),
        }
    }
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
    const fn slot(&self) -> Slot {
        match self {
            Self::Merged(_) | Self::MergeFailed { .. } => Slot::Merge,
            Self::Commented { .. } | Self::CommentFailed(_) => Slot::Comment,
            Self::Prompt { kind, .. } | Self::PromptFailed { kind, .. } => Slot::Handoff(*kind),
        }
    }
}

/// Which of a PR's runs: they run apart, each at most once at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    Merge,
    Comment,
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
            Self::Merge { url, .. } | Self::Comment { url, .. } | Self::Pick { url, .. } => url,
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
#[derive(Debug, Clone, Copy)]
pub struct Target<'a> {
    pub row: &'a Row,
    /// Its data, where a round has read it.
    pub pr: Option<&'a GhPr>,
    /// The login that acts on its repo, as `results.json` maps the owner ("" where it
    /// doesn't say).
    pub login: &'a str,
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
    /// A comment that didn't post (or wasn't known to), by PR: `c` brings it back.
    drafts: BTreeMap<String, String>,
    /// Methods GitHub refused for a repo: not offered again this run.
    refused: BTreeMap<String, Vec<MergeMethod>>,
    copied: Option<Copied>,
    /// Where each prompt being collected goes, by PR and kind.
    pending: BTreeMap<(String, Kind), String>,
    /// The `v` pane's first line, while it shows.
    view: Option<usize>,
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

    /// The `v` pane's first line, while it shows.
    #[must_use]
    pub const fn view(&self) -> Option<usize> {
        self.view
    }

    /// Whether the keys are the bottom line's or the `v` pane's: then they type, answer or
    /// scroll, and no other key acts.
    #[must_use]
    pub const fn is_holding_keys(&self) -> bool {
        self.ask.is_some() || self.view.is_some()
    }

    fn say(&mut self, text: impl Into<String>, tone: Tone, now: i64) {
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
    pub fn offers(&self, target: Target<'_>) -> Vec<Offer> {
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
            Key::Char('m') => self.merge(target?, now),
            Key::Char('c') => self.comment(target?),
            Key::Char('f') => self.hand_off(target?, now),
            Key::Char('o') => vec![Cmd::Open(target?.url().to_owned())],
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
            Ask::Comment {
                url,
                number,
                mut text,
                login,
            } => {
                match key {
                    Key::Esc => {
                        self.drafts.remove(&url);
                        return Vec::new();
                    }
                    Key::Enter if crate::comment::is_postable(&text) => {
                        // Kept until it posts: a refusal brings it back.
                        self.drafts.insert(url.clone(), text.clone());
                        return self.start(Job::Comment { url, number, text }, now);
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
                });
                Vec::new()
            }
            Ask::Pick { url, number, kinds } => {
                if matches!(key, Key::Esc | Key::Char('n')) {
                    return Vec::new();
                }
                let chosen = kinds.iter().copied().find(|k| key == Key::Char(k.key()));
                match (chosen, target) {
                    (Some(kind), Some(target)) if target.url() == url => {
                        self.run_handoff(kind, target, now)
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

    fn merge(&mut self, target: Target<'_>, now: i64) -> Vec<Cmd> {
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

    fn comment(&mut self, target: Target<'_>) -> Vec<Cmd> {
        let url = target.url().to_owned();
        self.ask = Some(Ask::Comment {
            text: self.drafts.get(&url).cloned().unwrap_or_default(),
            url,
            number: target.number(),
            login: target.login.to_owned(),
        });
        Vec::new()
    }

    fn hand_off(&mut self, target: Target<'_>, now: i64) -> Vec<Cmd> {
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

    fn run_handoff(&mut self, kind: Kind, target: Target<'_>, now: i64) -> Vec<Cmd> {
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
        self.pending.insert((facts.url.clone(), kind), target.to());
        self.start(
            Job::Collect {
                kind,
                facts,
                failing,
            },
            now,
        )
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
                let by = if login.is_empty() {
                    String::new()
                } else {
                    format!(" as {login}")
                };
                self.say(format!("✓ Commented on #{number}{by}"), Tone::Good, now);
            }
            Outcome::CommentFailed(error) => {
                self.say(
                    format!("✗ {error} · c to try again: your text is kept"),
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

fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}
