//! Properties of the TUI's model (the plan's Tests section): after any sequence of
//! messages, the selected list exists and the selection is a PR of it, or none exactly
//! when it is empty; a PR in Later is in no other list; and what the model shows is what its
//! inputs give.

use proptest::prelude::*;
use prw_core::act::{Done, Outcome};
use prw_core::full::Full;
use prw_core::gh::{GhPr, PrState};
use prw_core::merge::MergeRun;
use prw_core::opened::{Got, Opened, Read, ReadDone, Tab};
use prw_core::protocol::{FileKind, SessionFile, Writer};
use prw_core::status::{MergeMethod, PillKind};
use prw_core::ui::{
    Cmd, Key, ListId, Listed, Model, Msg, PollerSaid, ReadFailure, Session, lists_of, update,
};
use prw_core::watchlist::{Lists, Now};
use prw_core::web::{ListKind, REPO_HUES, state_of};
use prw_core::{Entry, Shared};

/// A handful of PRs, so reads overlap: the same URLs come and go.
const URLS: [&str; 8] = [
    "https://github.com/acme/app/pull/1",
    "https://github.com/acme/app/pull/2",
    "https://github.com/acme/app/pull/3",
    "https://github.com/acme/web/pull/4",
    "https://github.com/acme/web/pull/5",
    "https://github.com/acme/billing/pull/6",
    "https://github.com/acme/docs/pull/7",
    "not a url",
];

fn urls() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(prop::sample::select(&URLS[..]), 0..6)
        .prop_map(|urls| urls.into_iter().map(str::to_owned).collect())
}

fn entry() -> impl Strategy<Value = Entry> {
    let state = prop::sample::select(vec![PrState::Open, PrState::Merged, PrState::Closed]);
    (
        any::<bool>(),
        state,
        proptest::option::of("[a-z ]{0,12}"),
        any::<bool>(),
        "[A-Za-z ]{0,20}",
    )
        .prop_map(|(has_pr, state, error, is_unreachable, title)| Entry {
            at: 1,
            pr: has_pr.then(|| GhPr {
                number: 1,
                title,
                state,
                ..GhPr::default()
            }),
            error,
            failed_since: None,
            is_unreachable,
        })
}

fn shared() -> impl Strategy<Value = Shared> {
    (
        prop::collection::btree_map(prop::sample::select(&URLS[..]), entry(), 0..8),
        0_i64..200_000,
        0_i64..200_000,
    )
        .prop_map(|(prs, fetched_at, next_at)| Shared {
            fetched_at,
            next_at,
            prs: prs.into_iter().map(|(u, e)| (u.to_owned(), e)).collect(),
            ..Shared::default()
        })
}

fn session() -> impl Strategy<Value = Session> {
    let writer = proptest::option::of(prop::sample::select(vec![
        Writer::Mod,
        Writer::Prw,
        Writer::Other("x".into()),
    ]));
    (
        prop::sample::select(vec!["a", "b", "c", "prw-1", "prw-2"]),
        urls(),
        writer,
        proptest::option::of(prop::sample::select(vec!["app", "web", "App"])),
        proptest::option::of(prop::sample::select(vec!["claude", "codex"])),
        (
            0_i64..300_000,
            proptest::option::of(Just(FileKind::Searches)),
        ),
    )
        .prop_map(|(id, urls, writer, name, agent, (mtime, kind))| Session {
            id: id.to_owned(),
            file: SessionFile {
                urls,
                writer,
                agent: agent.map(str::to_owned),
                name: name.map(str::to_owned),
                cwd: None,
                kind,
            },
            mtime,
        })
}

fn key() -> impl Strategy<Value = Key> {
    prop_oneof![
        prop::sample::select(vec![
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Tab,
            Key::BackTab,
            Key::Home,
            Key::End,
            Key::Enter,
            Key::Esc,
            Key::CtrlC,
        ]),
        prop::sample::select(vec![
            'j', 'k', 'h', 'l', 'g', 'G', '?', 'q', 'x', '1', '2', '3', 'p', 'z'
        ])
        .prop_map(Key::Char),
    ]
}

/// Focus and Later as another `prw` may have written them: a PR in one of them at most.
fn lists() -> impl Strategy<Value = Lists> {
    let pill = prop::sample::select(vec![PillKind::Ready, PillKind::CiFailed, PillKind::Loading]);
    (
        urls(),
        prop::collection::vec((prop::sample::select(&URLS[..]), pill), 0..4),
    )
        .prop_map(|(focus, later)| {
            let mut lists = Lists::default();
            for url in focus {
                if !lists.is_focus(&url) {
                    lists.toggle_focus(&url);
                }
            }
            for (url, pill) in later {
                lists.put_aside(
                    url,
                    Now {
                        pill,
                        updated_at: None,
                    },
                    0,
                );
            }
            lists
        })
}

fn msg() -> impl Strategy<Value = Msg> {
    let at = || 0_i64..400_000;
    prop_oneof![
        1 => lists().prop_map(Msg::ListsRead),
        2 => (shared(), at()).prop_map(|(s, at)| Msg::SharedRead(Ok(s), at)),
        1 => at().prop_map(|at| Msg::SharedRead(Err(ReadFailure::Missing), at)),
        1 => at().prop_map(|at| Msg::SharedRead(Err(ReadFailure::Unreadable("torn".into())), at)),
        // Session ids are unique in a directory.
        2 => (prop::collection::vec(session(), 0..5), at()).prop_map(|(sessions, at)| {
            let mut seen = std::collections::HashSet::new();
            let sessions = sessions.into_iter().filter(|s| seen.insert(s.id.clone())).collect();
            Msg::SessionsRead(sessions, at)
        }),
        6 => key().prop_map(Msg::Key),
        // prw web's ★ and ⏾, which say what they want.
        2 => (prop::sample::select(&URLS[..]), any::<bool>(), any::<bool>()).prop_map(
            |(url, is_pin, is_on)| {
                let url = url.to_owned();
                if is_pin { Msg::Pin(url, is_on) } else { Msg::Aside(url, is_on) }
            }
        ),
        1 => (prop::sample::select(&URLS[..]), any::<bool>()).prop_map(|(url, is_ok)| {
            let got = if is_ok { Ok(Full::default()) } else { Err("HTTP 502".to_owned()) };
            Msg::Read(ReadDone { url: url.to_owned(), got: Got::Pr(got) })
        }),
        1 => (0_u16..=300, 0_u16..=80).prop_map(|(w, h)| Msg::Resize(w, h)),
        1 => at().prop_map(Msg::Tick),
        1 => (
            prop::sample::select(vec!["prw-1", "prw-9"]),
            proptest::option::of(at()),
            prop::collection::btree_map(prop::sample::select(vec!["Review", "Team", "prw-1"]), urls(), 0..3),
        ).prop_map(|(own, polled_at, searches)| {
            // Names are unique in config.toml; one may be a session's id.
            let searches = searches
                .into_iter()
                .map(|(name, urls)| Listed { name: name.into(), urls, ..Listed::default() })
                .collect();
            Msg::Poller(PollerSaid { own: own.into(), polled_at, searches, ..PollerSaid::default() })
        }),
    ]
}

fn check(model: &Model) -> Result<(), TestCaseError> {
    let lists = model.lists();
    prop_assert!(!lists.is_empty());
    prop_assert_eq!(&lists[0].id, &ListId::Focus);
    prop_assert_eq!(&lists[1].id, &ListId::Inbox);
    prop_assert_eq!(&lists[lists.len() - 1].id, &ListId::Later);
    // A PR put aside is in Later alone.
    let later = &lists[lists.len() - 1].rows;
    for row in later {
        let elsewhere = lists[..lists.len() - 1]
            .iter()
            .flat_map(|l| &l.rows)
            .any(|r| r.pr.url == row.pr.url);
        prop_assert!(!elsewhere, "{} in Later and elsewhere", row.pr.url);
    }
    prop_assert!(
        lists.iter().any(|l| l.id == *model.list_id()),
        "{:?}",
        model.list_id()
    );
    let ids: std::collections::HashSet<_> = lists.iter().map(|l| &l.id).collect();
    prop_assert_eq!(ids.len(), lists.len(), "list ids are unique");
    let rows = &model.list().rows;
    match model.selected() {
        None => prop_assert!(
            rows.is_empty(),
            "nothing selected in a list of {}",
            rows.len()
        ),
        Some(url) => prop_assert!(rows.iter().any(|r| r.pr.url == url), "{url} not listed"),
    }
    for list in lists {
        let urls: std::collections::HashSet<_> = list.rows.iter().map(|r| &r.pr.url).collect();
        prop_assert_eq!(urls.len(), list.rows.len(), "a PR once per list");
    }
    // An open PR's tabs each select one of their items.
    if let Some(opened) = model.opened() {
        for tab in Tab::ALL {
            let (at, len) = (opened.selected(tab), opened.len(tab));
            prop_assert!(
                at < len || (len == 0 && at == 0),
                "{:?}: {} of {}",
                tab,
                at,
                len
            );
        }
    }
    check_web(model)?;
    // What the model shows is what its inputs give now.
    prop_assert_eq!(
        lists,
        &lists_of(
            model.shared(),
            model.sessions(),
            model.now(),
            model.poller().map(|p| p.own.as_str()),
            model.poller().map_or(&[][..], |p| &p.searches),
            model.watchlist(),
        )[..]
    );
    Ok(())
}

/// What `prw web`'s page gets says what the model's lists do.
fn check_web(model: &Model) -> Result<(), TestCaseError> {
    let state = state_of(model, "1.0.0");
    prop_assert!(serde_json::to_string(&state).is_ok());
    prop_assert_eq!(state.lists.len(), model.lists().len());
    let ids: std::collections::HashSet<_> = state.lists.iter().map(|l| &l.id).collect();
    prop_assert_eq!(ids.len(), state.lists.len(), "page list ids are unique");
    for (list, view) in model.lists().iter().zip(&state.lists) {
        let urls: Vec<_> = list.rows.iter().map(|r| &r.pr.url).collect();
        let shown: Vec<_> = view.rows.iter().map(|r| &r.url).collect();
        prop_assert_eq!(urls, shown, "the page's rows are the model's, in order");
        for row in &view.rows {
            prop_assert_eq!(row.later.is_some(), view.kind == ListKind::Later);
            prop_assert_eq!(row.is_focus, model.watchlist().is_focus(&row.url));
            if view.kind == ListKind::Focus {
                prop_assert!(row.is_focus);
            }
            // Read or not, as the card has it.
            prop_assert_eq!(row.stations.is_none(), row.pill == PillKind::Loading);
            prop_assert!(!row.next.title.is_empty());
            prop_assert!(row.hue < REPO_HUES, "a colour the page has");
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn the_selection_stays_valid_after_any_messages(
        width in 0_u16..=300,
        height in 0_u16..=80,
        msgs in prop::collection::vec(msg(), 0..40),
    ) {
        let mut model = Model::new(width, height);
        check(&model)?;
        for msg in msgs {
            model = update(model, msg).0;
            check(&model)?;
        }
    }
}

const URL: &str = "https://github.com/acme/app/pull/1";

/// One open PR in `results.json`, read at 1000 ms, and no session: All lists it.
fn one_pr() -> Model {
    let shared = Shared {
        fetched_at: 1_000,
        next_at: 61_000,
        prs: [(
            URL.to_owned(),
            Entry {
                at: 1_000,
                pr: Some(GhPr {
                    number: 1,
                    title: "Retry uploads".into(),
                    state: PrState::Open,
                    ..GhPr::default()
                }),
                error: None,
                failed_since: None,
                is_unreachable: false,
            },
        )]
        .into(),
        ..Shared::default()
    };
    update(Model::new(120, 36), Msg::SharedRead(Ok(shared), 1_000)).0
}

fn read_pr() -> Read {
    Read::Pr {
        url: URL.into(),
        repo: "acme/app".into(),
        number: 1,
    }
}

fn got(full: Full) -> Msg {
    Msg::Read(ReadDone {
        url: URL.into(),
        got: Got::Pr(Ok(full)),
    })
}

#[test]
fn enter_opens_the_selected_pr_and_reads_it_and_esc_goes_back() {
    let (model, cmds) = update(one_pr(), Msg::Key(Key::Enter));
    assert_eq!(cmds, [Cmd::Read(read_pr())]);
    assert_eq!(model.opened().map(Opened::url), Some(URL));
    assert_eq!(model.opened().map(Opened::tab), Some(Tab::Conversation));
    // The tab keys, and the action keys act on the PR open.
    let (model, _) = update(model, Msg::Key(Key::Char('2')));
    assert_eq!(model.opened().map(Opened::tab), Some(Tab::Checks));
    let (model, cmds) = update(model, Msg::Key(Key::Char('o')));
    assert_eq!(cmds, [Cmd::Open(URL.into())]);
    let (model, cmds) = update(model, Msg::Key(Key::Esc));
    assert_eq!((model.opened(), cmds), (None, Vec::new()));
    assert_eq!(model.selected(), Some(URL));
    // Esc again quits, as before.
    assert_eq!(update(model, Msg::Key(Key::Esc)).1, [Cmd::Quit]);
}

#[test]
fn the_open_pr_is_read_again_every_thirty_seconds_and_after_a_merge() {
    let (model, _) = update(one_pr(), Msg::Key(Key::Enter));
    let (model, cmds) = update(model, got(Full::default()));
    assert_eq!(cmds, []);
    let (model, cmds) = update(model, Msg::Tick(30_999));
    assert_eq!(cmds, []);
    let (model, cmds) = update(model, Msg::Tick(31_000));
    assert_eq!(cmds, [Cmd::Read(read_pr())]);
    let (model, _) = update(model, got(Full::default()));
    let merged = Done {
        url: URL.into(),
        number: 1,
        outcome: Outcome::Merged(MergeRun::Merge(MergeMethod::Squash)),
    };
    let (_, cmds) = update(model, Msg::Done(merged));
    assert!(cmds.contains(&Cmd::Read(read_pr())), "{cmds:?}");
}

#[test]
fn a_read_for_a_pr_no_longer_open_changes_nothing() {
    let (model, _) = update(one_pr(), Msg::Key(Key::Enter));
    let (model, _) = update(model, Msg::Key(Key::Esc));
    let (model, cmds) = update(model, got(Full::default()));
    assert_eq!((model.opened(), cmds), (None, Vec::new()));
}
