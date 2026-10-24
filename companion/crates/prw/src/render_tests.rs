//! The screen as drawn into a `TestBackend`, from the demo cache (`companion/demo`), at the
//! mockup's sizes and at every size in between: what it says, and that it never panics.

use proptest::prelude::*;
use prw_core::act::{Done, Outcome};
use prw_core::batch::ref_from_url;
use prw_core::full::parse_full;
use prw_core::gh::PrState;
use prw_core::handoff::Kind;
use prw_core::opened::{Got, Opened, ReadDone, Tab};
use prw_core::ui::{Focus, Hit, Key, LIT_MS, Model, Msg, PollerSaid, Session, update};
use prw_core::{parse_session, parse_shared};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use unicode_width::UnicodeWidthStr;

use crate::theme::SEL;
use crate::view::{Ctx, Hits, is_animated, render};

const RESULTS: &str = include_str!("../../../demo/results.json");
const SEARCHES: &str = include_str!("../../../demo/searches.json");
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
    beat: 0,
    now: 0,
};

/// The demo's session files, as read at `now`.
fn sessions_at(now: i64) -> Vec<Session> {
    SESSIONS
        .iter()
        .map(|&(id, text)| Session {
            id: id.to_owned(),
            file: parse_session(text).expect("a demo session file reads"),
            mtime: now - 5_000,
        })
        .collect()
}

/// The demo, 12 s after its round, every session live.
fn demo(width: u16, height: u16) -> Model {
    let shared = parse_shared(RESULTS).expect("the demo's results.json reads");
    let now = shared.fetched_at + 12_000;
    let sessions = sessions_at(now);
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
    drawn(model, width, height).0
}

/// The screen's text, and where its clicks land.
fn drawn(model: &Model, width: u16, height: u16) -> (String, Hits) {
    drawn_at(model, width, height, 0)
}

/// The screen's text and its clicks, at the animation's `beat`.
fn drawn_at(model: &Model, width: u16, height: u16, beat: u64) -> (String, Hits) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    let mut hits = Hits::default();
    let ctx = Ctx { beat, ..CTX };
    terminal
        .draw(|frame| hits = render(frame, model, ctx))
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
    (out, hits)
}

/// The screen's cells, as drawn with `ctx`.
fn painted(model: &Model, width: u16, height: u16, ctx: Ctx) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal
        .draw(|frame| {
            render(frame, model, ctx);
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

/// The cell where `word` first shows on screen row `y`.
fn cell_of(text: &str, y: usize, word: &str) -> (u16, u16) {
    let line = text.lines().nth(y).expect("a row");
    let at = line
        .find(word)
        .unwrap_or_else(|| panic!("no {word:?} in {line:?}"));
    let x = u16::try_from(UnicodeWidthStr::width(&line[..at])).expect("a column");
    (x, u16::try_from(y).expect("a row"))
}

/// The first screen row that says `word`.
fn row_saying(text: &str, word: &str) -> usize {
    text.lines()
        .position(|l| l.contains(word))
        .unwrap_or_else(|| panic!("no {word:?} on\n{text}"))
}

/// A click where `word` shows on row `y`, as the loop gives it to the model.
fn click(model: Model, y: usize, word: &str) -> Model {
    let (text, hits) = drawn(&model, model.size().0, model.size().1);
    let (x, y) = cell_of(&text, y, word);
    let hit = hits
        .at(x, y)
        .unwrap_or_else(|| panic!("{word:?} takes no click"));
    feed(model, [Msg::Click(hit)])
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
        &["Keys", "next, previous list", "Actions go as"],
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
            Key::Char('G'), Key::Char('?'), Key::Char('m'), Key::Char('c'), Key::Char('f'),
            Key::Char('v'), Key::Char('n'), Key::Char('x'), Key::Esc, Key::Enter,
            Key::Char('1'), Key::Char('2'), Key::Char('3'), Key::Char('j'),
        ]), 0..8),
        is_read in any::<bool>(),
    ) {
        let mut model = feed(demo(width, height), keys.into_iter().map(Msg::Key));
        // An open PR, read or not yet.
        if is_read && let Some(url) = model.opened().map(|o| o.url().to_owned()) {
            model = feed(model, [read_of(&url)]);
        }
        let _ = screen(&model, width, height);
        // And an empty model, before anything was read.
        let _ = screen(&Model::new(width, height), width, height);
    }
}

#[test]
fn a_click_picks_a_row_then_a_hint_asks_and_an_answer_closes() {
    let model = demo(120, 30);
    let text = screen(&model, 120, 30);
    let model = click(model, row_saying(&text, "#409"), "#409");
    assert_eq!(model.selected_row().map(|r| r.pr.number), Some(409));
    // The hints' merge, on the bottom row.
    let model = click(model, 29, "merge");
    let (text, hits) = drawn(&model, 120, 30);
    assert_says(&text, &["y yes", "n no"]);
    // While it asks, only its answers take a click.
    let (x, y) = cell_of(&text, row_saying(&text, "#57"), "#57");
    assert_eq!(hits.at(x, y), None);
    let model = click(model, row_saying(&text, "n no"), "n no");
    assert!(model.acts().ask().is_none());
}

#[test]
fn a_click_picks_a_list_and_the_wheel_moves_where_it_is() {
    let model = demo(120, 30);
    let text = screen(&model, 120, 30);
    // The lists pane's, not the card's watchers.
    let model = click(model, row_saying(&text, "│ ◇ billing"), "billing");
    assert_eq!(model.list().label, "billing");
    assert_eq!(model.focus(), Focus::Lists);
    let (text, hits) = drawn(&model, 120, 30);
    let (x, y) = cell_of(&text, row_saying(&text, "LISTS"), "LISTS");
    assert_eq!(hits.pane_at(x, y), Some(Focus::Lists));
    let (x, y) = cell_of(&text, row_saying(&text, "CHECKS"), "CHECKS");
    assert_eq!(hits.pane_at(x, y), Some(Focus::Prs));
    // The strip's lists, narrower.
    let model = demo(84, 30);
    let model = click(model, 1, "billing");
    assert_eq!(model.list().label, "billing");
}

#[test]
fn a_moving_pr_spins_and_a_still_one_doesnt() {
    let model = demo(120, 30);
    assert!(is_animated(&model, 0));
    assert!(!is_animated(&Model::new(120, 30), 0));
    let first = drawn_at(&model, 120, 30, 0).0;
    let next = drawn_at(&model, 120, 30, 1).0;
    // #412's CI runs, #301 is loading: each its own frame (its number sets it apart).
    assert_says(&first, &["⠹ #412", "⠙ #301"]);
    assert_says(&next, &["⠸ #412", "⠹ #301"]);
    // #97 couldn't be read, #57 conflicts: nothing under way.
    assert_says(&next, &["◌ #97", "⚠ #57"]);
    // The selected PR's step turns too: #412's CI.
    let model = feed(model, to_row(7));
    assert_eq!(model.selected_row().map(|r| r.pr.number), Some(412));
    assert_says(&drawn_at(&model, 120, 30, 0).0, &["⠹ CI"]);
}

#[test]
fn a_pr_that_changes_glows_and_a_new_one_wipes_in() {
    const PR_57: &str = "https://github.com/acme/billing/pull/57";
    let model = demo(120, 30);
    let now = model.now();
    let mut merged = parse_shared(RESULTS).expect("the demo's results.json reads");
    if let Some(pr) = merged.prs.get_mut(PR_57).and_then(|e| e.pr.as_mut()) {
        pr.state = PrState::Merged;
    }
    // #409 leaves its one session's list, then comes back with #57 merged.
    let mut without = sessions_at(now);
    without[0].file.urls.retain(|u| !u.ends_with("/409"));
    let at = now + 1_000;
    let model = feed(
        model,
        [
            Msg::Tick(now),
            Msg::SessionsRead(without, now),
            Msg::SharedRead(Ok(merged), at),
            Msg::SessionsRead(sessions_at(at), at),
        ],
    );
    let ctx = |ms: i64| Ctx {
        now: at + ms,
        ..CTX
    };
    let text = drawn(&model, 120, 30).0;
    // Its row in the table (the selected one), not the card's title.
    let (x, y) = cell_of(&text, row_saying(&text, "✓ #57"), "#57");
    let bg = |ms: i64| painted(&model, 120, 30, ctx(ms))[(x, y)].bg;
    // Lit in its new pill's colour, easing back to the plain row.
    assert_ne!(bg(0), bg(LIT_MS));
    assert_ne!(bg(0), bg(LIT_MS / 2));
    assert_eq!(bg(LIT_MS), SEL);
    // #409 wipes in from the left: its title isn't there yet, then is.
    let y = row_saying(&text, "Release 4.12");
    let line = |ms: i64| {
        let buffer = painted(&model, 120, 30, ctx(ms));
        (0..120)
            .map(|x| {
                buffer[(x, u16::try_from(y).expect("a row"))]
                    .symbol()
                    .to_owned()
            })
            .collect::<String>()
    };
    assert!(!line(40).contains("Release 4.12"), "{}", line(40));
    assert!(line(40).contains('#'));
    assert!(line(450).contains("Release 4.12"));
}

#[test]
fn a_toast_fades_at_the_end_of_its_time() {
    let model = demo(120, 30);
    let now = model.now();
    let model = feed(
        model,
        [Msg::Opened(
            "https://github.com/acme/billing/pull/57".into(),
            Ok(()),
        )],
    );
    let until = model.acts().toast().expect("a toast").until;
    let text = drawn(&model, 120, 30).0;
    let (x, y) = cell_of(&text, row_saying(&text, "Opened #57"), "Opened");
    let fg = |at: i64| painted(&model, 120, 30, Ctx { now: at, ..CTX })[(x, y)].fg;
    assert_eq!(fg(now), fg(until - 600));
    assert_ne!(fg(until - 300), fg(now));
    // Its time up, the line says what it said before.
    let gone = painted(&model, 120, 30, Ctx { now: until, ..CTX });
    let line: String = (0..120).map(|x| gone[(x, y)].symbol()).collect();
    assert!(line.contains("Watching"), "{line}");
    // Drawn every beat from a tick before the fade on, and not before that or after.
    let still = feed(
        Model::new(120, 30),
        [Msg::Opened(
            "https://github.com/acme/billing/pull/57".into(),
            Ok(()),
        )],
    );
    let until = still.acts().toast().expect("a toast").until;
    let animated: Vec<bool> = [until - 2_000, until - 1_500, until - 1, until]
        .map(|at| is_animated(&still, at))
        .into();
    assert_eq!(animated, [false, true, true, false]);
}

#[test]
fn the_version_shows_and_so_does_a_newer_one() {
    let own = format!("pr-watch companion {}", env!("CARGO_PKG_VERSION"));
    let model = demo(120, 30);
    assert_says(&screen(&model, 120, 30), &[&own]);
    let newer = PollerSaid {
        latest: Some("99.0.0".into()),
        ..PollerSaid::default()
    };
    let model = feed(model, [Msg::Poller(newer)]);
    // Said on the status line while it says nothing else, after the toast.
    let now = model.now();
    let model = feed(model, [Msg::Tick(now), Msg::Tick(now + 7_000)]);
    assert_eq!(model.acts().toast(), None);
    let text = screen(&model, 120, 30);
    assert_says(
        &text,
        &[&own, "4 sessions live · 5 lists · ↑ prw 99.0.0 is out"],
    );
    assert_says(&screen(&model, 49, 30), &["↑ prw 99.0.0 is out"]);
    let help = feed(model, [Msg::Key(Key::Char('?'))]);
    let title = format!("Keys · prw {}", env!("CARGO_PKG_VERSION"));
    assert_says(&screen(&help, 120, 30), &[&title, "↑ prw 99.0.0 is out"]);
    assert_says(&screen(&help, 120, 14), &[&title]);
}

#[test]
fn a_click_anywhere_closes_the_help() {
    let model = feed(demo(84, 30), [Msg::Key(Key::Char('?'))]);
    let (_, hits) = drawn(&model, 84, 30);
    assert_eq!(hits.at(0, 0), Some(Hit::Key(Key::Char('?'))));
    assert!(!feed(model, [Msg::Click(Hit::Key(Key::Char('?')))]).is_help());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(60))]

    /// Every row a click lands on shows that row's PR, and every list its name.
    #[test]
    fn clicks_land_on_what_the_screen_shows(
        width in 40_u16..=200,
        height in 8_u16..=60,
        tabs in 0_usize..4,
    ) {
        let model = feed(demo(width, height), std::iter::repeat_n(Msg::Key(Key::Tab), tabs));
        let (text, hits) = drawn(&model, width, height);
        let lines: Vec<&str> = text.lines().collect();
        for y in 0..height {
            for x in 0..width {
                let line = lines[usize::from(y)];
                match hits.at(x, y) {
                    Some(Hit::Row(i)) => {
                        let number = format!("#{}", model.list().rows[i].pr.number);
                        prop_assert!(line.contains(&number), "{number} not on {line:?}");
                    }
                    Some(Hit::List(i)) => {
                        let label = &model.lists()[i].label;
                        let head: String = label.chars().take(2).collect();
                        prop_assert!(line.contains(&head), "{label} not on {line:?}");
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Keys that select the `n`th row of the list (0: the first).
fn to_row(n: usize) -> Vec<Msg> {
    std::iter::once(Key::Char('g'))
        .chain(std::iter::repeat_n(Key::Down, n))
        .map(Msg::Key)
        .collect()
}

#[test]
fn the_detail_offers_the_prs_actions() {
    // #88: CI failed.
    let text = screen(&feed(demo(120, 44), to_row(1)), 120, 44);
    assert_says(&text, &["ACTIONS", "[f] Fix", "[c] Comment", "[o] Open"]);
    // Where the checks need the rows, the actions shrink to keys on one line.
    let text = screen(&feed(demo(120, 30), to_row(1)), 120, 30);
    assert_says(&text, &["✗ build", "f fix", "c comment"]);
}

#[test]
fn the_bottom_line_asks_and_takes_the_comment() {
    // #409, ready to merge.
    let model = feed(
        demo(120, 30),
        to_row(5).into_iter().chain([Msg::Key(Key::Char('m'))]),
    );
    assert_says(&screen(&model, 120, 30), &["#409", "y yes", "n no"]);
    let typed = "looks good".chars().map(|c| Msg::Key(Key::Char(c)));
    let model = feed(
        demo(120, 30),
        to_row(1)
            .into_iter()
            .chain([Msg::Key(Key::Char('c'))])
            .chain(typed),
    );
    assert_says(
        &screen(&model, 120, 30),
        &["comment on #88 › looks good", "alt-⏎ new line"],
    );
}

#[test]
fn v_shows_the_prompt_last_copied() {
    let url = "https://github.com/acme/web/pull/88";
    let prompt = (1..=60)
        .map(|n| format!("line {n} of the prompt"))
        .collect::<Vec<_>>()
        .join("\n");
    let copied = feed(
        demo(84, 30),
        [
            Msg::Done(Done {
                url: url.to_owned(),
                number: 88,
                outcome: Outcome::Prompt {
                    kind: Kind::Fix,
                    text: prompt,
                    note: None,
                },
            }),
            Msg::Copied(Ok("pbcopy".to_owned())),
        ],
    );
    assert_says(
        &screen(&copied, 84, 30),
        &["Fix prompt for #88 copied (pbcopy)"],
    );
    let shown = feed(copied, [Msg::Key(Key::Char('v'))]);
    let text = screen(&shown, 84, 30);
    assert_says(
        &text,
        &["Fix prompt for #88", "line 1 of the prompt", "of 60"],
    );
    // The end, however far the scroll asks to go.
    let end = feed(shown, [Msg::Key(Key::Char('G')), Msg::Key(Key::Down)]);
    let text = screen(&end, 84, 30);
    assert_says(&text, &["line 60 of the prompt", "lines 33-60 of 60"]);
    let closed = feed(end, [Msg::Key(Key::Esc)]);
    assert!(!screen(&closed, 84, 30).contains("line 60 of the prompt"));
}

#[test]
fn with_two_logins_the_detail_says_which_and_a_switches() {
    let logins = prw_core::accounts::Logins {
        active: "hubot".to_owned(),
        others: vec!["octocat".to_owned()],
    };
    let said = Msg::Poller(PollerSaid {
        logins,
        ..PollerSaid::default()
    });
    let model = feed(demo(120, 44), std::iter::once(said).chain(to_row(1)));
    assert_says(
        &screen(&model, 120, 44),
        &["· as octocat", "[a] As octocat → switch to hubot"],
    );
    // One login: nothing said of it.
    assert!(!screen(&feed(demo(120, 44), to_row(1)), 120, 44).contains("· as octocat"));
}

/// The demo with its saved searches, as `--demo` shows them.
fn searched(width: u16, height: u16) -> Model {
    let searches = serde_json::from_str(SEARCHES).expect("the demo's searches.json reads");
    feed(
        demo(width, height),
        [Msg::Poller(PollerSaid {
            searches,
            ..PollerSaid::default()
        })],
    )
}

#[test]
fn saved_searches_are_lists_of_their_own() {
    let model = searched(120, 30);
    let text = screen(&model, 120, 30);
    // The pane is 22 wide: "Review requests" is cut.
    assert_says(
        &text,
        &["LISTS", "◎ Review reques…", "◎ Team web", "SESSIONS"],
    );
    // Above SESSIONS, with All.
    let at = |word: &str| text.find(word).expect(word);
    assert!(at("Team web") < at("SESSIONS"));
    // Tab to Team web: what it asks, and how many GitHub found in all.
    let web = feed(model, [Msg::Key(Key::Tab), Msg::Key(Key::Tab)]);
    assert_says(
        &screen(&web, 120, 30),
        &[
            "Team web  ·  is:open repo:acme/web · 4 of 112",
            "◆ Keep the cart",
            "#97",
        ],
    );
}

/// What the demo has on a PR for its full view (`demo/full/`), as a read would bring it.
fn read_of(url: &str) -> Msg {
    let pr = ref_from_url(url).expect("a PR URL");
    let reply = match pr.number {
        59 => include_str!("../../../demo/full/acme-billing-59.json"),
        88 => include_str!("../../../demo/full/acme-web-88.json"),
        _ => include_str!("../../../demo/full/acme-app-412.json"),
    };
    Msg::Read(ReadDone {
        url: url.to_owned(),
        got: Got::Pr(parse_full(reply, "", &pr)),
    })
}

/// The demo with row `n`'s PR open full size and read.
fn opened(width: u16, height: u16, n: usize) -> Model {
    let model = feed(demo(width, height), to_row(n));
    let model = feed(model, [Msg::Key(Key::Enter)]);
    let url = model.opened().expect("open").url().to_owned();
    feed(model, [read_of(&url)])
}

#[test]
fn enter_opens_the_pr_full_size_on_its_first_open_thread() {
    // #59: changes requested, two threads open and one resolved.
    let model = opened(120, 36, 2);
    let text = screen(&model, 120, 36);
    assert_says(
        &text,
        &[
            "‹ back",
            "acme/billing #59",
            "1 Conversation 5",
            "2 Checks 1",
            "3 Files 2",
            "@octocat opened this",
            "pdf/layout.ts:41",
            "This should end where the amount column ends",
            "2 comments · @octocat: Yes, only the glyphs used.",
            "@bob requested changes",
            "esc back",
        ],
    );
    // The selection starts on the first open thread, beside the PR's card.
    let at = row_saying(&text, "pdf/layout.ts:41");
    assert!(text.lines().nth(at).is_some_and(|l| l.starts_with('▌')));
    assert_says(&text, &["ACTIONS", "[f] Address"]);
    // Narrow: the tab alone, wrapped to it.
    let text = screen(&opened(49, 36, 2), 49, 36);
    assert_says(
        &text,
        &["‹ back", "pdf/layout.ts:41", "ends: 480, not 470."],
    );
    let text = screen(&feed(model, [Msg::Key(Key::Char('3'))]), 120, 36);
    assert_says(
        &text,
        &["M pdf/layout.ts", "● 2", "line 41 · @bob", "M pdf/fonts.ts"],
    );
}

#[test]
fn a_failed_pr_opens_on_its_checks_with_each_failed_jobs_log() {
    // #88: CI failed.
    let model = opened(120, 40, 1);
    assert_eq!(model.opened().map(Opened::tab), Some(Tab::Checks));
    let text = screen(&model, 120, 40);
    assert_says(&text, &["CI / build", "reading the log…", "✓ CI / types"]);
    let logs = Msg::Read(ReadDone {
        url: "https://github.com/acme/web/pull/88".into(),
        got: Got::Logs(vec![
            (9101, Ok(include_str!("../../../demo/full/9101.log").into())),
            (9102, Err("HTTP 404".into())),
        ]),
    });
    let text = screen(&feed(model, [logs]), 120, 40);
    assert_says(
        &text,
        &[
            "error TS2322",
            "Process completed with exit code 2.",
            "last 6 of 6 lines",
            "no log: HTTP 404",
        ],
    );
    assert!(!text.contains("##[error]"), "{text}");
}

#[test]
fn a_click_picks_a_tab_or_an_item_and_back_goes_back() {
    let model = opened(120, 36, 2);
    let model = click(model, 3, "2 Checks");
    assert_eq!(model.opened().map(Opened::tab), Some(Tab::Checks));
    let model = click(model, 3, "1 Conversation");
    let text = screen(&model, 120, 36);
    let model = click(
        model,
        row_saying(&text, "pdf/layout.ts:88"),
        "pdf/layout.ts:88",
    );
    let text = screen(&model, 120, 36);
    let at = row_saying(&text, "pdf/layout.ts:88");
    assert!(
        text.lines().nth(at).is_some_and(|l| l.starts_with('▌')),
        "{text}"
    );
    let model = click(model, 1, "‹ back");
    assert!(model.opened().is_none());
    // A click on the selected row opens it, as Enter does.
    let text = screen(&model, 120, 36);
    let model = click(model, row_saying(&text, "◇ Right-align"), "#59");
    assert_eq!(model.opened().map(Opened::number), Some(59));
}

#[test]
fn what_a_read_leaves_out_is_said_to_be_on_github() {
    let model = feed(demo(120, 36), to_row(2));
    let model = feed(model, [Msg::Key(Key::Enter)]);
    let url = model.opened().expect("open").url().to_owned();
    let mut reply: serde_json::Value =
        serde_json::from_str(include_str!("../../../demo/full/acme-billing-59.json"))
            .expect("json");
    let pr = &mut reply["data"]["p0"]["pullRequest"];
    pr["comments"]["totalCount"] = 31.into();
    pr["reviewThreads"]["totalCount"] = 104.into();
    pr["files"]["totalCount"] = 250.into();
    let reply = reply.to_string();
    let pr = ref_from_url(&url).expect("a PR URL");
    let model = feed(
        model,
        [Msg::Read(ReadDone {
            url,
            got: Got::Pr(parse_full(&reply, "", &pr)),
        })],
    );
    // The newest 50 comments and 100 threads came: the rest are older.
    let comments = 31
        - model
            .opened()
            .and_then(Opened::full)
            .map_or(0, |f| f.comments.len());
    let threads = 104
        - model
            .opened()
            .and_then(Opened::full)
            .map_or(0, |f| f.threads.len());
    let text = screen(&model, 120, 36);
    assert_says(
        &text,
        &[&format!(
            "… {comments} older comments, {threads} older threads on GitHub"
        )],
    );
    let model = feed(model, [Msg::Key(Key::Char('3'))]);
    let text = screen(&model, 120, 36);
    assert_says(&text, &["3 Files 250", "… 248 more files on GitHub"]);
}
