//! `prw`: the terminal companion for the pr-watch mod.
//!
//! Skeleton (step 1 of the plan): a bordered box that shows the terminal's
//! width and the layout `prw_core::layout` picks for it, redrawn on resize.

use std::io;
use std::process::ExitCode;

use prw_core::layout;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
prw: every PR your agents touch, in one terminal

Usage: prw [OPTIONS]

Options:
  -h, --help     Print this help
  -V, --version  Print the version

Keys: q, Esc or Ctrl-C quits.
";

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Run,
    Help,
    Version,
    Unknown(String),
}

fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Command {
    // The first argument decides; there are no subcommands yet.
    match args.into_iter().next().as_deref() {
        None => Command::Run,
        Some("-h" | "--help") => Command::Help,
        Some("-V" | "--version") => Command::Version,
        Some(other) => Command::Unknown(other.to_owned()),
    }
}

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Command::Run => match run() {
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
    }
}

/// Takes over the terminal, runs until a quit key, and gives it back.
/// `ratatui::init` also installs a panic hook that restores the terminal first.
fn run() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal) -> io::Result<()> {
    loop {
        terminal.draw(render)?;
        // Blocks until something happens; a resize (or any other event)
        // falls through to the next draw, which reads the new size.
        if let Event::Key(key) = event::read()?
            && is_quit(key)
        {
            return Ok(());
        }
    }
}

fn is_quit(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && match key.code {
            KeyCode::Char('q') | KeyCode::Esc => true,
            KeyCode::Char('c') => key.modifiers.contains(KeyModifiers::CONTROL),
            _ => false,
        }
}

fn render(frame: &mut Frame) {
    let area = frame.area();
    let width = area.width;
    let chosen = layout(width);
    let block = Block::bordered()
        .title(" prw ")
        .title_bottom(Line::from(" q quit ").right_aligned());
    let body = Paragraph::new(Line::from(format!(
        "width {width} · layout {}",
        chosen.name()
    )))
    .block(block);
    frame.render_widget(body, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen(width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        terminal.draw(render).expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn shows_the_width_and_the_layout_it_chose() {
        for (width, name) in [
            (120, "three panes"),
            (84, "strip"),
            (49, "side"),
            (30, "too narrow"),
        ] {
            let text = screen(width, 4);
            assert!(text.contains(" prw "), "{width}: {text}");
            assert!(
                text.contains(&format!("width {width} · layout {name}")),
                "{width}: {text}"
            );
        }
    }

    #[test]
    fn parses_the_flags() {
        let parse = |args: &[&str]| parse_args(args.iter().map(|&s| s.to_owned()));
        assert_eq!(parse(&[]), Command::Run);
        assert_eq!(parse(&["--version"]), Command::Version);
        assert_eq!(parse(&["-V"]), Command::Version);
        assert_eq!(parse(&["--help"]), Command::Help);
        assert_eq!(parse(&["-h"]), Command::Help);
        assert_eq!(parse(&["watch"]), Command::Unknown("watch".into()));
    }

    #[test]
    fn quits_on_q_esc_and_ctrl_c_only() {
        let press = |code, modifiers| KeyEvent::new(code, modifiers);
        assert!(is_quit(press(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert!(is_quit(press(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(is_quit(press(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!is_quit(press(KeyCode::Char('c'), KeyModifiers::NONE)));
        assert!(!is_quit(press(KeyCode::Char('x'), KeyModifiers::NONE)));
        let mut release = press(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert!(!is_quit(release));
    }
}
