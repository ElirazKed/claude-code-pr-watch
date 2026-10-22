//! `prw`: the terminal companion for the pr-watch mod.
//!
//! A TUI over the mod's shared cache. It reads `results.json` and the session files
//! (`cache`), feeds what it read to the model (`prw_core::ui::update`) as messages, and
//! draws the model (`view`) after each one. Beside it, unless `--no-poll`, a poller thread
//! (`poller`) watches every gh login's own open PRs and takes the poll when no mod is
//! polling, so `prw` is of use without Claude running at all.

mod cache;
mod gh;
mod poller;
mod round;
mod theme;
mod view;

#[cfg(test)]
mod render_tests;

use std::io;
use std::path::PathBuf;
use std::process::{self, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use prw_core::parse_shared;
use prw_core::ui::{Cmd, Key, Model, Msg, PollerSaid, update};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::cache::{Reader, default_dir, ms_of};
use crate::poller::{Files, Poller};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often the files are looked at, and ages move on.
const TICK: Duration = Duration::from_secs(1);

const HELP: &str = "\
prw: every PR your agents touch, in one terminal

Usage: prw [OPTIONS]

Shows the PRs the pr-watch mod polls, from its shared cache, and every open PR of each
gh login (\"My PRs\"). When no Claude session's mod is polling, prw polls GitHub itself,
sharing the cache's lease and rate limits with the mod as one more session.

Options:
      --cache <DIR>  The cache to use [env: PRW_CACHE] [default: ~/.cache/pr-watch]
      --no-poll      Only read the cache: never write to it, never ask GitHub anything
      --demo         Show a cache as if it were live: every session file counts as
                     just written, and the clock starts at results.json's last round
                     (for companion/demo, whose files never change). Implies --no-poll
  -h, --help         Print this help
  -V, --version      Print the version

Keys: j/k move, tab next list, h/l panes or lists, g/G first/last, ? help, q quit.
";

/// How to run.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    cache: Option<PathBuf>,
    is_demo: bool,
    is_no_poll: bool,
}

impl Options {
    /// Whether the poller runs: a demo's cache is a fixture, never written.
    const fn is_polling(&self) -> bool {
        !self.is_demo && !self.is_no_poll
    }
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Run(Options),
    Help,
    Version,
    Unknown(String),
    /// A flag that takes a value, without one.
    Missing(&'static str),
}

fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Command {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Command::Help,
            "-V" | "--version" => return Command::Version,
            "--demo" => options.is_demo = true,
            "--no-poll" => options.is_no_poll = true,
            "--cache" => match args.next() {
                Some(dir) if !dir.is_empty() => options.cache = Some(dir.into()),
                _ => return Command::Missing("--cache"),
            },
            other => match other.strip_prefix("--cache=") {
                Some("") => return Command::Missing("--cache"),
                Some(dir) => options.cache = Some(dir.into()),
                None => return Command::Unknown(other.to_owned()),
            },
        }
    }
    Command::Run(options)
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Command::Run(options) => match run(&options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("prw: {err}");
                ExitCode::FAILURE
            }
        },
        Command::Help => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Command::Version => {
            println!("prw {VERSION}");
            ExitCode::SUCCESS
        }
        Command::Unknown(arg) => {
            eprintln!("prw: unknown argument '{arg}'\n\n{HELP}");
            ExitCode::from(2)
        }
        Command::Missing(flag) => {
            eprintln!("prw: {flag} needs a value\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}

/// The cache to read: `--cache`, else `$PRW_CACHE`, else the mod's.
fn cache_dir(options: &Options) -> PathBuf {
    let env = std::env::var("PRW_CACHE").ok().filter(|d| !d.is_empty());
    options
        .cache
        .clone()
        .or_else(|| env.map(PathBuf::from))
        .unwrap_or_else(|| default_dir(std::env::var("HOME").ok().as_deref()))
}

/// The time the model sees (ms): the wall clock, or, in a demo, the wall clock moved so it
/// starts just after the cache's last round.
struct Clock {
    offset: i64,
}

impl Clock {
    fn new(dir: &std::path::Path, is_demo: bool) -> Self {
        let wall = ms_of(SystemTime::now());
        let fetched_at = std::fs::read_to_string(dir.join("results.json"))
            .ok()
            .and_then(|text| parse_shared(&text).ok())
            .map_or(0, |shared| shared.fetched_at);
        let offset = if is_demo && fetched_at > 0 {
            fetched_at + 12_000 - wall
        } else {
            0
        };
        Self { offset }
    }

    fn now(&self) -> i64 {
        ms_of(SystemTime::now()).saturating_add(self.offset)
    }
}

/// Takes over the terminal, runs until a quit key, and gives it back.
/// `ratatui::try_init` also installs a panic hook that restores the terminal first.
/// A failed init (say, stdout is not a terminal) is reported, not a panic, and
/// undoes whatever part of the setup had already happened (raw mode).
fn run(options: &Options) -> io::Result<()> {
    let dir = cache_dir(options);
    let clock = Clock::new(&dir, options.is_demo);
    let files = options
        .is_polling()
        .then(|| Files::new(dir.clone(), own_id()));
    let reader = Reader::new(dir, options.is_demo);
    let mut terminal = ratatui::try_init().inspect_err(|_| ratatui::restore())?;
    let stop = Arc::new(AtomicBool::new(false));
    let said = files
        .clone()
        .map(|files| poller::spawn(Poller::new(files, gh::System, gh::Wall), Arc::clone(&stop)));
    let result = event_loop(&mut terminal, reader, &clock, said.as_ref());
    // Its PRs are no longer watched: the session file goes, and a held lease is given back.
    // A tick still running dies with the process.
    stop.store(true, Ordering::Relaxed);
    if let Some(files) = &files {
        files.leave();
    }
    ratatui::restore();
    result
}

/// This run's name in the cache (`prw-<id>`): its session file, and the lease's owner.
fn own_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("prw-{:x}{nanos:08x}", process::id())
}

fn feed(model: Model, msgs: impl IntoIterator<Item = Msg>) -> (Model, bool) {
    let mut quit = false;
    let model = msgs.into_iter().fold(model, |model, msg| {
        let (model, cmds) = update(model, msg);
        quit |= cmds.contains(&Cmd::Quit);
        model
    });
    (model, quit)
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    mut reader: Reader,
    clock: &Clock,
    said: Option<&Receiver<PollerSaid>>,
) -> io::Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let shown = view::tilde(&reader.dir().display().to_string(), &home);
    let ctx = view::Ctx {
        dir: &shown,
        home: &home,
    };
    let size = terminal.size()?;
    let (mut model, _) = feed(
        Model::new(size.width, size.height),
        reader.poll(clock.now()),
    );
    let mut last_tick = Instant::now();
    loop {
        terminal.draw(|frame| view::render(frame, &model, ctx))?;
        // Wait for a key or a resize until the next tick; only a message redraws.
        loop {
            let timeout = (last_tick + TICK).saturating_duration_since(Instant::now());
            // A tick that is due comes first, so a stream of keys can't hold it off.
            if !timeout.is_zero() && event::poll(timeout)? {
                let msg = match event::read()? {
                    Event::Key(key) => key_of(key).map(Msg::Key),
                    Event::Resize(width, height) => Some(Msg::Resize(width, height)),
                    _ => None,
                };
                let Some(msg) = msg else { continue };
                let (next, quit) = feed(model, [msg]);
                if quit {
                    return Ok(());
                }
                model = next;
            } else {
                last_tick = Instant::now();
                let now = clock.now();
                let polled = said
                    .into_iter()
                    .flat_map(Receiver::try_iter)
                    .map(Msg::Poller);
                let read = reader.poll(now);
                (model, _) = feed(model, polled.chain(read).chain([Msg::Tick(now)]));
            }
            break;
        }
    }
}

/// A terminal key as the model takes it; `None` for a release or a key it doesn't use.
fn key_of(key: KeyEvent) -> Option<Key> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let is_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char('c') if is_ctrl => Key::CtrlC,
        KeyCode::Char(_) if is_ctrl => return None,
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_flags() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|&s| s.to_owned()));
        assert_eq!(parse(&[]), Command::Run(Options::default()));
        assert_eq!(parse(&["--version"]), Command::Version);
        assert_eq!(parse(&["-V"]), Command::Version);
        assert_eq!(parse(&["--help"]), Command::Help);
        assert_eq!(parse(&["-h"]), Command::Help);
        assert_eq!(parse(&["watch"]), Command::Unknown("watch".into()));
        assert_eq!(parse(&["--version", "extra"]), Command::Version);
        let demo = Options {
            cache: Some("companion/demo".into()),
            is_demo: true,
            ..Options::default()
        };
        assert!(!demo.is_polling());
        assert_eq!(
            parse(&["--cache", "companion/demo", "--demo"]),
            Command::Run(demo)
        );
        assert_eq!(
            parse(&["--cache=/tmp/c"]),
            Command::Run(Options {
                cache: Some("/tmp/c".into()),
                ..Options::default()
            })
        );
        let Command::Run(read_only) = parse(&["--no-poll"]) else {
            panic!("--no-poll runs")
        };
        assert!(read_only.is_no_poll && !read_only.is_polling());
        assert!(Options::default().is_polling());
        assert_eq!(parse(&["--cache"]), Command::Missing("--cache"));
        assert_eq!(parse(&["--cache="]), Command::Missing("--cache"));
    }

    #[test]
    fn maps_the_keys_and_ignores_releases() {
        let press = |code, modifiers| KeyEvent::new(code, modifiers);
        assert_eq!(
            key_of(press(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Key::Char('q'))
        );
        assert_eq!(
            key_of(press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Key::Char('G'))
        );
        assert_eq!(
            key_of(press(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Key::Esc)
        );
        assert_eq!(
            key_of(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Key::CtrlC)
        );
        assert_eq!(
            key_of(press(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            None
        );
        assert_eq!(
            key_of(press(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Key::BackTab)
        );
        let mut release = press(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(key_of(release), None);
    }
}
