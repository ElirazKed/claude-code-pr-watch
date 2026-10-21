//! The screen as drawn into a `TestBackend`, from the demo cache (`companion/demo`), at the
//! mockup's sizes and at every size in between: what it says, and that it never panics.

use proptest::prelude::*;
use prw_core::ui::{Key, Model, Msg, Session, update};
use prw_core::{parse_session, parse_shared};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_width::UnicodeWidthStr;

use crate::view::{Ctx, render};

const RESULTS: &str = include_str!("../../../demo/results.json");
const SESSIONS: [(&str, &str); 4] = [
    (
        "mh0w2b-k3p9x1aq",
        include_str!("../../../demo/sessions/mh0w2b-k3p9x1aq.json"),
    ),
    (
        "mh0w5c-q8r2m4zt",
        include_str!("../../../demo/sessions/mh0w5c-q8r2m4zt.json"),
    ),
    (
        "prw-7f3a9c",
        include_str!("../../../demo/sessions/prw-7f3a9c.json"),
    ),
    (
        "mgx1aa-0ld5e55n",
        include_str!("../../../demo/sessions/mgx1aa-0ld5e55n.json"),
    ),
];

const CTX: Ctx<'static> = Ctx {
    dir: "companion/demo",
    home: "/home/ada",
};

/// The demo, 12 s after its round, every session live.
fn demo(width: u16, height: u16) -> Model {
    let shared = parse_shared(RESULTS).expect("the demo's results.json reads");
    let now = shared.fetched_at + 12_000;
    let sessions = SESSIONS
        .iter()
        .map(|&(id, text)| Session {
            id: id.to_owned(),
            file: parse_session(text).expect("a demo session file reads"),
            mtime: now - 5_000,
        })
        .collect();
    feed(
        Model::new(width, height),
        [
            Msg::SharedRead(Ok(shared), now),
            Msg::SessionsRead(sessions, now),
        ],
    )
}

fn feed(model: Model, msgs: impl IntoIterator<Item = Msg>) -> Model {
    msgs.into_iter()
        .fold(model, |model, msg| update(model, msg).0)
}

/// The screen's text, one line per row.
fn screen(model: &Model, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| render(frame, model, CTX))
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..height {
        // A wide character (💬) takes two cells; the second holds nothing to print.
        let mut skip = 0;
        for x in 0..width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let symbol = buffer[(x, y)].symbol();
            out.push_str(symbol);
            skip = UnicodeWidthStr::width(symbol).saturating_sub(1);
        }
        out.push('\n');
    }
    out
}

fn assert_says(text: &str, words: &[&str]) {
    for word in words {
        assert!(text.contains(word), "missing {word:?} in\n{text}");
    }
}

#[test]
fn three_panes_at_120() {
    let text = screen(&demo(120, 30), 120, 30);
    assert_says(
        &text,
        &[
            "◆ pr-watch companion",
            "polled by mod · 12s ago · 4 live",
            "Lists",
            "All",
            " ⚠ 11",
            "app-retry",
            "web-checkout",
            "billing",
            "Claude session",
            "#57",
            "Prorate plan changes",
            "acme/billing #57",
            "⚠ CONFLICTS",
            "Merge conflicts with base",
            "CHECKS",
            "github.com/acme/billing/pull/57",
            "AGENTS",
            "codex",
            "GITHUB",
            "octocat",
            "next round in 8s",
            "j/k move",
        ],
    );
    // Down to #88, CI failed: its card names the failing checks.
    let model = feed(demo(120, 30), [Msg::Key(Key::Char('j'))]);
    let text = screen(&model, 120, 30);
    assert_says(
        &text,
        &[
            "acme/web #88",
            "✗ CI FAILED",
            "Failing: build, e2e +1",
            "✗ build",
        ],
    );
}

#[test]
fn strip_at_84() {
    let text = screen(&demo(84, 30), 84, 30);
    assert_says(
        &text,
        &[
            "polled by mod",
            " All 11 ",
            "◆ app-retry 2",
            "#57",
            "Prorate plan",
            "⚠ CONFLICTS",
            "acme/billing #57",
        ],
    );
    // Tab to the next list: the strip marks it, the table is its PRs.
    let model = feed(demo(84, 30), [Msg::Key(Key::Tab)]);
    let text = screen(&model, 84, 30);
    assert_says(&text, &["╭ app-retry ─", "#412", "#409"]);
    let text = screen(&feed(demo(120, 30), [Msg::Key(Key::Tab)]), 120, 30);
    assert_says(&text, &["app-retry  ·  claude · /work/app", "▌◆ app-retry"]);
}

#[test]
fn side_at_49() {
    let text = screen(&demo(49, 30), 49, 30);
    assert_says(
        &text,
        &[
            "polled by mod",
            "◂ All · 11 ▸",
            "1/5 lists",
            "#57",
            "⚠ CONFLICTS",
            "acme/billing #57",
        ],
    );
    // Lists by name: All, app-retry, billing, web-checkout, then the unnamed.
    let model = feed(
        demo(49, 30),
        [Msg::Key(Key::BackTab), Msg::Key(Key::BackTab)],
    );
    let text = screen(&model, 49, 30);
    assert_says(&text, &["◆ web-checkout · 4 ▸", "4/5 lists", "#88"]);
}

#[test]
fn too_narrow_at_30() {
    let text = screen(&demo(30, 10), 30, 10);
    assert_says(&text, &["prw needs 40 columns", "this one has 30"]);
}

#[test]
fn errors_and_placeholders_show_as_the_mod_shows_them() {
    // #97: no gh account could see it. #301: watched, not read yet. #140: data, then 502s.
    let rows = |model: &Model| {
        model.lists()[0]
            .rows
            .iter()
            .map(|r| r.pr.number)
            .collect::<Vec<_>>()
    };
    let model = demo(120, 40);
    let at = |n: u64| rows(&model).iter().position(|&m| m == n).expect("listed");
    let to = |n: u64| {
        let mut model = demo(120, 40);
        for _ in 0..at(n) {
            model = feed(model, [Msg::Key(Key::Down)]);
        }
        screen(&model, 120, 40)
    };
    assert_says(
        &to(97),
        &[
            "can't read",
            "⚠ Not found, or no gh account can",
            "◌ LOADING",
        ],
    );
    assert_says(&to(301), &["Loading…", "Fetching from GitHub…"]);
    assert_says(&to(140), &["⚠ gh: HTTP 502", "💬 COMMENTS"]);
}

#[test]
fn waiting_for_pr_watch_before_any_results() {
    let model = feed(
        Model::new(120, 30),
        [Msg::SharedRead(Err(prw_core::ui::ReadFailure::Missing), 0)],
    );
    let text = screen(&model, 120, 30);
    assert_says(
        &text,
        &[
            "waiting for pr-watch",
            "Waiting for pr-watch",
            "Start a Claude session with the pr-watch mod",
            "Reading companion/demo",
        ],
    );
}

#[test]
fn the_help_shows_and_hides() {
    let model = feed(demo(84, 30), [Msg::Key(Key::Char('?'))]);
    assert_says(
        &screen(&model, 84, 30),
        &["Keys", "next, previous list", "Read-only"],
    );
    let model = feed(model, [Msg::Key(Key::Char('?'))]);
    assert!(!screen(&model, 84, 30).contains("next, previous list"));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn render_never_panics(
        width in 0_u16..=300,
        height in 0_u16..=80,
        keys in prop::collection::vec(prop::sample::select(vec![
            Key::Down, Key::Up, Key::Tab, Key::BackTab, Key::Char('h'), Key::Char('l'),
            Key::Char('G'), Key::Char('?'),
        ]), 0..6),
    ) {
        let model = feed(demo(width, height), keys.into_iter().map(Msg::Key));
        let _ = screen(&model, width, height);
        // And an empty model, before anything was read.
        let _ = screen(&Model::new(width, height), width, height);
    }
}
