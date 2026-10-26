//! The action keys, through the model as the binary drives it: what each key asks, what it
//! hands the worker, and what the screen says once the worker answers.

use proptest::prelude::*;
use prw_core::act::{Ask, Done, Job, Look, OnThread, Outcome, Tone};
use prw_core::address::{GhThread, Login};
use prw_core::full::Full;
use prw_core::handoff::Kind;
use prw_core::merge::{MergeError, MergeRun};
use prw_core::opened::{Got, Read, ReadDone};
use prw_core::protocol::{SessionFile, Writer};
use prw_core::status::MergeMethod::{Rebase, Squash};
use prw_core::ui::{Cmd, Key, Model, Msg, Session, update};
use prw_core::{Entry, Shared};
use serde_json::{Value, json};

const URL: &str = "https://github.com/acme/web/pull/88";
const NOW: i64 = 1_000_000;

fn pr(fields: Value) -> Entry {
    let mut pr = json!({
        "number": 88, "title": "Keep the cart across a refresh", "url": URL, "state": "OPEN",
        "headRefName": "fix/cart", "baseRefName": "main", "author": {"login": "ada"},
        "mergeStateStatus": "CLEAN",
        "repository": {"squashMergeAllowed": true, "rebaseMergeAllowed": true, "mergeCommitAllowed": false, "viewerPermission": "WRITE"},
    });
    if let (Some(base), Value::Object(more)) = (pr.as_object_mut(), fields) {
        base.extend(more);
    }
    Entry {
        at: NOW,
        pr: Some(serde_json::from_value(pr).expect("a PR")),
        ..Entry::default()
    }
}

/// The model with one PR, watched by a live Claude session in `web`.
fn model_of(entry: Entry) -> Model {
    let shared = Shared {
        fetched_at: NOW,
        next_at: NOW + 60_000,
        prs: [(URL.to_owned(), entry)].into_iter().collect(),
        accounts: [("acme".to_owned(), "ada".to_owned())]
            .into_iter()
            .collect(),
        ..Shared::default()
    };
    let session = Session {
        id: "s1".to_owned(),
        file: SessionFile {
            urls: vec![URL.to_owned()],
            writer: Some(Writer::Mod),
            agent: Some("claude".to_owned()),
            name: Some("web".to_owned()),
            ..SessionFile::default()
        },
        mtime: NOW,
    };
    let model = Model::new(120, 36);
    let (model, _) = update(model, Msg::SharedRead(Ok(shared), NOW));
    let (model, _) = update(model, Msg::SessionsRead(vec![session], NOW));
    model
}

/// Each message in turn; the commands they gave.
fn feed(model: Model, msgs: impl IntoIterator<Item = Msg>) -> (Model, Vec<Cmd>) {
    msgs.into_iter()
        .fold((model, Vec::new()), |(model, mut all), msg| {
            let (model, cmds) = update(model, msg);
            all.extend(cmds);
            (model, all)
        })
}

fn keys(text: &str) -> Vec<Msg> {
    text.chars().map(|c| Msg::Key(Key::Char(c))).collect()
}

fn toast(model: &Model) -> (&str, Tone) {
    let toast = model.acts().toast().expect("a toast");
    (toast.text.as_str(), toast.tone)
}

#[test]
fn m_asks_first_tries_the_next_method_and_y_merges() {
    let (model, cmds) = feed(model_of(pr(json!({}))), keys("m"));
    assert_eq!(cmds, []);
    assert!(matches!(
        model.acts().ask(),
        Some(Ask::Merge {
            run: MergeRun::Merge(Squash),
            ..
        })
    ));
    let (model, _) = feed(model, keys("m"));
    assert!(matches!(
        model.acts().ask(),
        Some(Ask::Merge {
            run: MergeRun::Merge(Rebase),
            ..
        })
    ));
    let (model, cmds) = feed(model, keys("y"));
    assert_eq!(
        cmds,
        [Cmd::Run(Job::Merge {
            url: URL.to_owned(),
            number: 88,
            run: MergeRun::Merge(Rebase)
        })]
    );
    assert_eq!(model.acts().ask(), None);
    assert_eq!(model.acts().busy().collect::<Vec<_>>(), ["Merging #88"]);
    // A second press while it runs starts nothing.
    let (model, cmds) = feed(model, keys("my"));
    assert_eq!(cmds, []);
    assert_eq!(toast(&model), ("Still merging #88…", Tone::Info));
    let done = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::Merged(MergeRun::Merge(Rebase)),
    };
    let (model, _) = feed(model, [Msg::Done(done)]);
    assert_eq!(model.acts().busy().count(), 0);
    assert_eq!(toast(&model), ("✓ Rebase-merged #88", Tone::Good));
}

#[test]
fn n_or_esc_cancels_and_nothing_runs() {
    for no in [Key::Char('n'), Key::Esc] {
        let (model, cmds) = feed(
            model_of(pr(json!({}))),
            [Msg::Key(Key::Char('m')), Msg::Key(no)],
        );
        assert_eq!(cmds, []);
        assert_eq!(model.acts().ask(), None);
    }
    // Esc answered the question: it didn't quit.
    let (_, cmds) = feed(
        model_of(pr(json!({}))),
        [Msg::Key(Key::Char('m')), Msg::Key(Key::Esc)],
    );
    assert!(!cmds.contains(&Cmd::Quit));
}

#[test]
fn a_pr_that_cant_merge_says_why_and_asks_nothing() {
    let (model, cmds) = feed(
        model_of(pr(json!({"mergeStateStatus": "DIRTY"}))),
        keys("m"),
    );
    assert_eq!(cmds, []);
    assert_eq!(model.acts().ask(), None);
    assert_eq!(
        toast(&model),
        ("#88 can't be merged now: Conflicts with main", Tone::Info)
    );
    let (model, _) = feed(model_of(pr(json!({"state": "MERGED"}))), keys("m"));
    assert_eq!(toast(&model).0, "#88 is merged");
}

#[test]
fn waiting_on_checks_m_turns_auto_merge_on() {
    let waiting = pr(json!({"mergeStateStatus": "BLOCKED", "viewerCanEnableAutoMerge": true}));
    let (model, cmds) = feed(model_of(waiting), keys("my"));
    assert_eq!(
        cmds,
        [Cmd::Run(Job::Merge {
            url: URL.to_owned(),
            number: 88,
            run: MergeRun::Auto(Squash)
        })]
    );
    assert_eq!(
        model.acts().busy().collect::<Vec<_>>(),
        ["Turning on auto-merge for #88"]
    );
}

#[test]
fn a_method_github_refused_is_not_offered_again() {
    let (model, _) = feed(model_of(pr(json!({}))), keys("my"));
    let refused = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::MergeFailed {
            run: MergeRun::Merge(Squash),
            error: MergeError {
                message: "Squash merges are not allowed on this repository.".to_owned(),
                is_method_refused: true,
            },
        },
    };
    let (model, _) = feed(model, [Msg::Done(refused)]);
    assert_eq!(
        toast(&model),
        (
            "✗ #88: Squash merges are not allowed on this repository.",
            Tone::Bad
        )
    );
    let (model, _) = feed(model, keys("m"));
    assert!(matches!(
        model.acts().ask(),
        Some(Ask::Merge { run: MergeRun::Merge(Rebase), methods, .. }) if *methods == [Rebase]
    ));
}

#[test]
fn c_writes_a_comment_and_enter_posts_it_whole() {
    let (model, _) = feed(model_of(pr(json!({}))), keys("c"));
    assert!(matches!(model.acts().ask(), Some(Ask::Comment { login, .. }) if login == "ada"));
    // Every key types, q and m included; Alt-Enter is a new line, Backspace takes one back.
    let mut msgs = keys("LGTM, qmx");
    msgs.extend([
        Msg::Key(Key::Backspace),
        Msg::Key(Key::Newline),
        Msg::Paste("@ops -f\r\nthanks".to_owned()),
    ]);
    let (model, cmds) = feed(model, msgs);
    assert_eq!(cmds, []);
    let (model, cmds) = feed(model, [Msg::Key(Key::Enter)]);
    assert_eq!(
        cmds,
        [Cmd::Run(Job::Comment {
            url: URL.to_owned(),
            number: 88,
            text: "LGTM, qm\n@ops -f\nthanks".to_owned()
        })]
    );
    assert_eq!(model.acts().ask(), None);
    let posted = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::Commented {
            login: "ada".to_owned(),
        },
    };
    let (model, _) = feed(model, [Msg::Done(posted)]);
    assert_eq!(toast(&model), ("✓ Commented on #88 as ada", Tone::Good));
    // Posted: the next comment starts empty.
    let (model, _) = feed(model, keys("c"));
    assert!(matches!(model.acts().ask(), Some(Ask::Comment { text, .. }) if text.is_empty()));
}

#[test]
fn a_blank_comment_posts_nothing_and_a_refused_one_comes_back() {
    let (model, cmds) = feed(
        model_of(pr(json!({}))),
        [
            Msg::Key(Key::Char('c')),
            Msg::Key(Key::Char(' ')),
            Msg::Key(Key::Enter),
        ],
    );
    assert_eq!(cmds, []);
    assert!(model.acts().ask().is_some());
    let (model, _) = feed(model, keys("hi"));
    let (model, _) = feed(model, [Msg::Key(Key::Enter)]);
    let refused = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::CommentFailed(
            "Couldn't comment: Unable to create comment because issue is locked.".to_owned(),
        ),
    };
    let (model, _) = feed(model, [Msg::Done(refused)]);
    assert_eq!(toast(&model).1, Tone::Bad);
    let (model, _) = feed(model, keys("c"));
    assert!(matches!(model.acts().ask(), Some(Ask::Comment { text, .. }) if text == " hi"));
    // Esc drops it.
    let (model, _) = feed(model, [Msg::Key(Key::Esc), Msg::Key(Key::Char('c'))]);
    assert!(matches!(model.acts().ask(), Some(Ask::Comment { text, .. }) if text.is_empty()));
}

#[test]
fn f_on_a_conflict_copies_the_resolve_prompt_at_once() {
    let (model, cmds) = feed(
        model_of(pr(json!({"mergeStateStatus": "DIRTY"}))),
        keys("f"),
    );
    let [Cmd::Copy(text)] = cmds.as_slice() else {
        panic!("one copy: {cmds:?}");
    };
    assert!(text.starts_with("Merge conflicts on acme/web#88: Keep the cart across a refresh\n"));
    let (model, _) = feed(model, [Msg::Copied(Ok("pbcopy".to_owned()))]);
    assert_eq!(
        toast(&model),
        (
            "✓ Resolve prompt for #88 copied (pbcopy) · paste it into Claude (web) · v shows it",
            Tone::Good
        )
    );
    // v shows it; j scrolls, Esc closes, and no key reaches the lists meanwhile.
    let (model, cmds) = feed(model, keys("vjq"));
    assert_eq!(cmds, []);
    assert_eq!(model.acts().view(), None);
    // A prompt that fits the screen doesn't scroll, nor does G take it past its end.
    let (model, _) = feed(model, keys("vjG"));
    assert_eq!(model.acts().view(), Some(0));
    assert_eq!(
        model.acts().copied().map(|c| c.text.as_str()),
        Some(text.as_str())
    );
}

#[test]
fn a_copy_that_failed_says_so_and_points_at_v() {
    let (model, _) = feed(
        model_of(pr(json!({"mergeStateStatus": "DIRTY"}))),
        keys("f"),
    );
    let (model, _) = feed(model, [Msg::Copied(Err("no clipboard tool".to_owned()))]);
    assert_eq!(
        toast(&model),
        (
            "✗ Couldn't copy the Resolve prompt for #88: no clipboard tool · v shows it to copy by hand",
            Tone::Bad
        )
    );
}

#[test]
fn f_with_more_than_one_hand_off_asks_which() {
    let failing = json!([{"__typename": "CheckRun", "name": "unit", "status": "COMPLETED", "conclusion": "FAILURE", "workflow": "CI"}]);
    let threads = json!({"totalCount": 1, "nodes": [{"isResolved": false, "isOutdated": false}]});
    let entry = pr(
        json!({"statusCheckRollup": failing, "latestReviews": {"nodes": []}, "reviewThreads": threads}),
    );
    let (model, cmds) = feed(model_of(entry), keys("f"));
    assert_eq!(cmds, []);
    assert!(
        matches!(model.acts().ask(), Some(Ask::Pick { kinds, .. }) if *kinds == [Kind::Fix, Kind::Address])
    );
    let (model, cmds) = feed(model, keys("f"));
    let [
        Cmd::Run(Job::Collect {
            kind: Kind::Fix,
            facts,
            failing,
        }),
    ] = cmds.as_slice()
    else {
        panic!("a collect: {cmds:?}");
    };
    assert_eq!((facts.number, facts.branch.as_str()), (88, "fix/cart"));
    assert_eq!(failing, &["unit"]);
    assert_eq!(
        model.acts().busy().collect::<Vec<_>>(),
        ["Reading #88's failing checks and their logs"]
    );
    // The prompt comes back: copied, and kept for v.
    let done = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::Prompt {
            kind: Kind::Fix,
            text: "CI failed on acme/web#88: …".to_owned(),
            note: Some("No log for unit: GitHub no longer keeps this log".to_owned()),
        },
    };
    let (model, cmds) = feed(model, [Msg::Done(done)]);
    assert_eq!(cmds, [Cmd::Copy("CI failed on acme/web#88: …".to_owned())]);
    let (model, _) = feed(model, [Msg::Copied(Ok("OSC 52".to_owned()))]);
    let (text, tone) = toast(&model);
    assert!(
        text.contains("copied (OSC 52) · No log for unit: GitHub no longer keeps this log · paste")
    );
    assert_eq!(tone, Tone::Info);
}

#[test]
fn nothing_to_hand_off_says_so() {
    let (model, cmds) = feed(model_of(pr(json!({}))), keys("f"));
    assert_eq!(cmds, []);
    assert_eq!(
        toast(&model).0,
        "Nothing to hand off on #88: no conflict, failing check or open thread"
    );
}

#[test]
fn o_opens_the_pr_and_the_answer_is_a_toast() {
    let (model, cmds) = feed(model_of(pr(json!({}))), keys("o"));
    assert_eq!(cmds, [Cmd::Open(URL.to_owned())]);
    let (model, _) = feed(model, [Msg::Opened(URL.to_owned(), Ok(()))]);
    assert_eq!(toast(&model), ("↗ Opened #88 in the browser", Tone::Good));
}

#[test]
fn a_toast_goes_once_its_time_is_up() {
    let (model, _) = feed(model_of(pr(json!({}))), keys("v"));
    assert_eq!(
        toast(&model).0,
        "Nothing copied yet: f copies a PR's hand-off"
    );
    let (model, _) = feed(model, [Msg::Tick(NOW + 5_000)]);
    assert!(model.acts().toast().is_some());
    let (model, _) = feed(model, [Msg::Tick(NOW + 7_000)]);
    assert!(model.acts().toast().is_none());
}

#[test]
fn the_actions_list_says_what_the_pr_offers() {
    let entry = pr(json!({"mergeStateStatus": "DIRTY"}));
    let offers = model_of(entry).offers();
    let said: Vec<(&str, &str, Look)> = offers
        .iter()
        .map(|o| (o.key, o.words.as_str(), o.look))
        .collect();
    assert_eq!(
        said,
        [
            ("f", "Resolve → copy for Claude (web)", Look::Handoff),
            ("c", "Comment as ada", Look::Plain),
            ("X", "Close the PR", Look::Plain),
            ("o", "Open on GitHub ↗", Look::Plain),
        ]
    );
    let (model, _) = feed(model_of(pr(json!({}))), keys("my"));
    assert_eq!(model.offers()[0].words, "Merging #88…");
    assert_eq!(model.offers()[0].look, Look::Busy);
}

#[test]
fn q_still_quits_and_ctrl_c_quits_even_mid_comment() {
    let (_, cmds) = feed(model_of(pr(json!({}))), keys("q"));
    assert_eq!(cmds, [Cmd::Quit]);
    let (_, cmds) = feed(
        model_of(pr(json!({}))),
        [Msg::Key(Key::Char('c')), Msg::Key(Key::CtrlC)],
    );
    assert_eq!(cmds, [Cmd::Quit]);
}

fn key() -> impl Strategy<Value = Key> {
    prop_oneof![
        prop::sample::select(vec![
            'm', 'c', 'f', 'o', 'v', 'y', 'n', 'a', 'r', 'j', 'k', 'x'
        ])
        .prop_map(Key::Char),
        prop::sample::select(vec![
            Key::Enter,
            Key::Esc,
            Key::Tab,
            Key::Backspace,
            Key::Newline,
            Key::Down
        ]),
    ]
}

proptest! {
    /// Whatever is pressed, a job only ever acts on the PR on screen, a question is about a
    /// listed PR, and no slot runs twice at once.
    #[test]
    fn every_job_is_for_the_pr_and_no_slot_runs_twice(presses in prop::collection::vec(key(), 0..40), status in prop::sample::select(vec!["CLEAN", "DIRTY", "BLOCKED"]), is_full in any::<bool>()) {
        let mut model = model_of(pr(json!({"mergeStateStatus": status, "viewerCanEnableAutoMerge": true})));
        if is_full {
            // Open full size, on its thread: r, x and a act there.
            model = opened_from(model, full("kai", false));
        }
        let mut running = 0;
        for key in presses {
            let (next, cmds) = update(model, Msg::Key(key));
            model = next;
            for cmd in &cmds {
                match cmd {
                    Cmd::Run(job) => {
                        prop_assert_eq!(job.url(), URL);
                        running += 1;
                    }
                    Cmd::Open(url) => prop_assert_eq!(url, URL),
                    Cmd::Read(read) => prop_assert_eq!(read.url(), URL),
                    Cmd::Copy(_) | Cmd::Quit => {}
                }
            }
            if let Some(ask) = model.acts().ask() {
                prop_assert_eq!(ask.url(), URL);
            }
            prop_assert_eq!(model.acts().busy().count(), running);
        }
    }
}

/// The model, told by its poller that gh has these logins.
fn with_logins(model: Model, active: &str, others: &[&str]) -> Model {
    let said = prw_core::ui::PollerSaid {
        logins: prw_core::accounts::Logins {
            active: active.to_owned(),
            others: others.iter().map(|&o| o.to_owned()).collect(),
        },
        ..prw_core::ui::PollerSaid::default()
    };
    update(model, Msg::Poller(said)).0
}

#[test]
fn a_moves_the_owner_to_the_next_login_and_says_so() {
    let model = with_logins(model_of(pr(json!({}))), "bob", &["ada"]);
    let row = model.selected_row().expect("a row").clone();
    assert_eq!(model.login_of(&row).as_deref(), Some("ada"));
    assert!(
        model
            .offers()
            .iter()
            .any(|o| o.key == "a" && o.words == "As ada → switch to bob")
    );
    let (model, cmds) = feed(model, keys("a"));
    assert_eq!(
        cmds,
        [Cmd::Run(Job::Switch {
            url: URL.to_owned(),
            number: 88,
            owner: "acme".to_owned(),
            login: "bob".to_owned(),
        })]
    );
    assert_eq!(
        model.offers().iter().find(|o| o.key == "a").map(|o| o.look),
        Some(Look::Busy)
    );
    let done = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::Switched {
            owner: "acme".to_owned(),
            login: "bob".to_owned(),
        },
    };
    let (model, _) = feed(model, [Msg::Done(done)]);
    assert_eq!(toast(&model), ("✓ acme's PRs go as bob", Tone::Good));
}

#[test]
fn with_one_login_there_is_nothing_to_switch_to_and_nothing_said_of_it() {
    let model = with_logins(model_of(pr(json!({}))), "ada", &[]);
    let row = model.selected_row().expect("a row").clone();
    assert_eq!(model.login_of(&row), None);
    assert!(!model.offers().iter().any(|o| o.key == "a"));
    let (model, cmds) = feed(model, keys("a"));
    assert_eq!(cmds, []);
    assert_eq!(
        toast(&model),
        ("One gh login: nothing to switch to", Tone::Info)
    );
}

#[test]
fn with_three_logins_a_login_put_back_is_passed_over() {
    let model = with_logins(model_of(pr(json!({}))), "ada", &["bob", "cy"]);
    let (model, cmds) = feed(model, keys("a"));
    let to = |cmds: &[Cmd]| match cmds {
        [Cmd::Run(Job::Switch { login, .. })] => login.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(to(&cmds), "bob");
    // bob couldn't see it: the fetch kept ada, and the next press tries cy.
    let done = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::PutBack {
            owner: "acme".to_owned(),
            login: "bob".to_owned(),
            kept: "ada".to_owned(),
        },
    };
    let (model, _) = feed(model, [Msg::Done(done)]);
    assert_eq!(
        toast(&model),
        ("✗ bob can't see it: acme's PRs stay as ada", Tone::Bad)
    );
    assert!(
        model
            .offers()
            .iter()
            .any(|o| o.words == "As ada → switch to cy")
    );
    let (model, cmds) = feed(model, keys("a"));
    assert_eq!(to(&cmds), "cy");
    // A switch that never got to the fetch (nothing written) isn't counted as tried.
    let failed = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::SwitchFailed("Couldn't write results.json".to_owned()),
    };
    let (model, _) = feed(model, [Msg::Done(failed)]);
    assert!(
        model
            .offers()
            .iter()
            .any(|o| o.words == "As ada → switch to cy")
    );
    // One that took forgets what was put back: on from the new login, in order.
    let took = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::Switched {
            owner: "acme".to_owned(),
            login: "cy".to_owned(),
        },
    };
    let (model, _) = feed(model, [Msg::Done(took)]);
    assert!(
        model
            .offers()
            .iter()
            .any(|o| o.words == "As ada → switch to bob")
    );
}

// ---- The full view: r, x and a ------------------------------------------------------

/// #88 as the full view reads it: kai's PR, with one review thread on `src/cart.ts:14`.
fn full(author: &str, is_resolved: bool) -> Full {
    Full {
        id: "PR_88".to_owned(),
        author: Some(Login {
            login: author.to_owned(),
        }),
        threads: vec![GhThread {
            id: "T1".to_owned(),
            is_resolved,
            path: "src/cart.ts".to_owned(),
            line: Some(14),
            ..GhThread::default()
        }],
        ..Full::default()
    }
}

/// #88 open full size and read, the selection on its thread (where a read puts it, or `j`
/// from the description where the thread is resolved).
fn opened(full: Full) -> Model {
    opened_from(model_of(pr(json!({}))), full)
}

fn opened_from(model: Model, full: Full) -> Model {
    let (model, _) = feed(model, [Msg::Key(Key::Enter)]);
    let got = Msg::Read(ReadDone {
        url: URL.to_owned(),
        got: Got::Pr(Ok(full)),
    });
    let (model, _) = feed(model, [got]);
    let on_thread = model.opened().and_then(|o| o.thread()).is_some();
    if on_thread {
        model
    } else {
        feed(model, keys("j")).0
    }
}

fn on_thread() -> OnThread {
    OnThread {
        id: "T1".to_owned(),
        spot: "src/cart.ts:14".to_owned(),
    }
}

fn read_again() -> Cmd {
    Cmd::Read(Read::Pr {
        url: URL.to_owned(),
        repo: "acme/web".to_owned(),
        number: 88,
    })
}

fn done(outcome: Outcome) -> Msg {
    Msg::Done(Done {
        url: URL.to_owned(),
        number: 88,
        outcome,
    })
}

#[test]
fn r_replies_on_the_thread_and_a_refused_reply_comes_back_and_reads_the_pr_again() {
    let (model, _) = feed(opened(full("kai", false)), keys("r"));
    assert_eq!(
        model.acts().ask(),
        Some(&Ask::Comment {
            url: URL.to_owned(),
            number: 88,
            text: String::new(),
            login: "ada".to_owned(),
            thread: Some(on_thread()),
        })
    );
    let (model, cmds) = feed(
        model,
        keys("on it").into_iter().chain([Msg::Key(Key::Enter)]),
    );
    let reply = Job::Reply {
        url: URL.to_owned(),
        number: 88,
        thread: on_thread(),
        text: "on it".to_owned(),
    };
    assert_eq!(cmds, [Cmd::Run(reply)]);
    assert_eq!(
        model.acts().busy().collect::<Vec<_>>(),
        ["Posting the reply on src/cart.ts:14"]
    );
    let (model, cmds) = feed(
        model,
        [done(Outcome::ReplyFailed {
            thread: on_thread(),
            error: "Couldn't reply: locked".into(),
        })],
    );
    assert_eq!(cmds, [read_again()]);
    assert_eq!(
        toast(&model),
        (
            "✗ Couldn't reply: locked · r brings your text back",
            Tone::Bad
        )
    );
    // The text comes back on its thread, not in the PR's comment field.
    let (model, _) = feed(model, keys("r"));
    assert!(matches!(model.acts().ask(), Some(Ask::Comment { text, .. }) if text == "on it"));
    let (model, _) = feed(model, [Msg::Key(Key::Esc), Msg::Key(Key::Char('c'))]);
    assert!(
        matches!(model.acts().ask(), Some(Ask::Comment { text, thread: None, .. }) if text.is_empty())
    );
    let (model, _) = feed(model, [Msg::Key(Key::Esc)]);
    let replied = Outcome::Replied {
        thread: on_thread(),
        login: "ada".to_owned(),
    };
    let (model, _) = feed(model, [done(replied)]);
    assert_eq!(
        toast(&model),
        ("✓ Replied on src/cart.ts:14 as ada", Tone::Good)
    );
}

#[test]
fn r_off_a_thread_comments_on_the_pr_and_in_the_lists_is_no_key() {
    let (model, _) = feed(opened(full("kai", false)), keys("kr"));
    assert!(matches!(
        model.acts().ask(),
        Some(Ask::Comment { thread: None, .. })
    ));
    let (model, cmds) = feed(model_of(pr(json!({}))), keys("r"));
    assert_eq!((model.acts().ask(), cmds), (None, Vec::new()));
}

#[test]
fn x_resolves_the_thread_or_reopens_a_resolved_one() {
    let resolve = |is_resolved| {
        let (_, cmds) = feed(opened(full("kai", is_resolved)), keys("x"));
        cmds
    };
    let job = |resolve| {
        Cmd::Run(Job::Resolve {
            url: URL.to_owned(),
            number: 88,
            thread: on_thread(),
            resolve,
        })
    };
    assert_eq!(resolve(false), [job(true)]);
    assert_eq!(resolve(true), [job(false)]);
    let (model, cmds) = feed(opened(full("kai", false)), keys("kx"));
    assert_eq!(cmds, []);
    assert_eq!(
        toast(&model),
        (
            "Pick a review thread with j/k first: x resolves it",
            Tone::Info
        )
    );
    let reopened = Outcome::Resolved {
        thread: on_thread(),
        resolve: false,
    };
    let (model, cmds) = feed(opened(full("kai", true)), [done(reopened)]);
    assert_eq!(cmds, [read_again()]);
    assert_eq!(toast(&model), ("● Reopened src/cart.ts:14", Tone::Good));
}

#[test]
fn a_in_the_full_view_approves_after_a_y_and_in_the_lists_still_switches() {
    let (model, cmds) = feed(opened(full("kai", false)), keys("a"));
    assert_eq!(cmds, []);
    assert_eq!(
        model.acts().ask(),
        Some(&Ask::Approve {
            url: URL.to_owned(),
            number: 88,
            pr: "PR_88".to_owned(),
            login: "ada".to_owned(),
        })
    );
    let (model, cmds) = feed(model, keys("y"));
    let approve = Job::Approve {
        url: URL.to_owned(),
        number: 88,
        pr: "PR_88".to_owned(),
    };
    assert_eq!(cmds, [Cmd::Run(approve)]);
    let approved = Outcome::Approved {
        login: "ada".to_owned(),
    };
    let (model, cmds) = feed(model, [done(approved)]);
    assert_eq!(cmds, [read_again()]);
    assert_eq!(toast(&model), ("✓ Approved #88 as ada", Tone::Good));
    // n answers no.
    let (model, cmds) = feed(opened(full("kai", false)), keys("an"));
    assert_eq!((model.acts().ask(), cmds), (None, Vec::new()));
    // In the lists, a is the switch, as before.
    let (model, _) = feed(model_of(pr(json!({}))), keys("a"));
    assert_eq!(model.acts().ask(), None);
    assert_eq!(toast(&model).0, "One gh login: nothing to switch to");
}

#[test]
fn a_on_your_own_pr_or_one_not_read_yet_says_why_and_asks_nothing() {
    let (model, _) = feed(opened(full("ada", false)), keys("a"));
    assert_eq!(model.acts().ask(), None);
    assert_eq!(
        toast(&model),
        (
            "#88 is ada's own: GitHub doesn't let its author approve it",
            Tone::Info
        )
    );
    let (model, _) = feed(
        model_of(pr(json!({}))),
        [Msg::Key(Key::Enter), Msg::Key(Key::Char('a'))],
    );
    assert_eq!(toast(&model).0, "#88 hasn't been read yet");
}

#[test]
fn the_full_views_actions_list_offers_the_thread_and_approval_not_the_switch() {
    let model = with_logins(opened(full("kai", false)), "ada", &["hubot"]);
    let said: Vec<(&str, String)> = model
        .offers()
        .into_iter()
        .map(|o| (o.key, o.words))
        .collect();
    let keys: Vec<&str> = said.iter().map(|(k, _)| *k).collect();
    assert_eq!(keys, ["m", "r", "x", "a", "c", "X", "o"]);
    assert_eq!(said[1].1, "Reply on src/cart.ts:14");
    assert_eq!(said[2].1, "Resolve src/cart.ts:14");
    assert_eq!(said[3].1, "Approve as ada");
    // Back in the lists the switch is offered again.
    let (model, _) = feed(model, [Msg::Key(Key::Esc)]);
    assert!(
        model
            .offers()
            .iter()
            .any(|o| o.key == "a" && o.words.contains("switch"))
    );
}

#[test]
fn a_resolve_on_one_thread_holds_up_none_on_another_and_x_again_undoes_it() {
    let mut two = full("kai", false);
    let mut second = two.threads[0].clone();
    second.id = "T2".to_owned();
    second.line = Some(30);
    two.threads.push(second);
    // On T1, x; then on T2, x: both run.
    let (model, cmds) = feed(opened(two), keys("xjx"));
    let threads: Vec<String> = cmds
        .iter()
        .filter_map(|c| match c {
            Cmd::Run(Job::Resolve { thread, .. }) => Some(thread.id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(threads, ["T1", "T2"]);
    assert_eq!(model.acts().busy().count(), 2);
    // T1 resolved: before the read comes, x on it again reopens it.
    let resolved = Outcome::Resolved {
        thread: on_thread(),
        resolve: true,
    };
    let (model, _) = feed(model, [done(resolved)].into_iter().chain(keys("k")));
    assert!(
        model
            .offers()
            .iter()
            .any(|o| o.words == "Reopen src/cart.ts:14")
    );
    let (_, cmds) = feed(model, keys("x"));
    assert!(cmds.contains(&Cmd::Run(Job::Resolve {
        url: URL.to_owned(),
        number: 88,
        thread: on_thread(),
        resolve: false,
    })));
}

#[test]
fn a_read_asked_before_a_resolve_does_not_undo_it_on_screen() {
    let read = |is_resolved| {
        Msg::Read(ReadDone {
            url: URL.to_owned(),
            got: Got::Pr(Ok(full("kai", is_resolved))),
        })
    };
    let commented = Outcome::Commented {
        login: "ada".to_owned(),
    };
    let resolved = Outcome::Resolved {
        thread: on_thread(),
        resolve: true,
    };
    // A comment asks a read; the resolve comes while it is out.
    let (model, cmds) = feed(
        opened(full("kai", false)),
        [done(commented), done(resolved)],
    );
    assert_eq!(cmds, [read_again()]);
    // That read, asked before the resolve, still says open: the thread stays resolved, and
    // the read after it is asked.
    let (model, cmds) = feed(model, [read(false)]);
    assert_eq!(cmds, [read_again()]);
    assert!(
        model
            .offers()
            .iter()
            .any(|o| o.words == "Reopen src/cart.ts:14")
    );
    // The read asked after it says what GitHub has.
    let (model, _) = feed(model, [read(false)]);
    assert!(
        model
            .offers()
            .iter()
            .any(|o| o.words == "Resolve src/cart.ts:14")
    );
}

#[test]
fn a_reply_sent_while_one_on_its_thread_posts_stays_in_the_field() {
    let (model, _) = feed(
        opened(full("kai", false)),
        keys("r")
            .into_iter()
            .chain(keys("one"))
            .chain([Msg::Key(Key::Enter)]),
    );
    let (model, cmds) = feed(
        model,
        keys("r")
            .into_iter()
            .chain(keys("two"))
            .chain([Msg::Key(Key::Enter)]),
    );
    assert_eq!(cmds, []);
    // The field opened empty, not with the reply on its way, so nothing goes twice.
    assert!(matches!(model.acts().ask(), Some(Ask::Comment { text, .. }) if text == "two"));
    assert_eq!(
        toast(&model).0,
        "Still posting the reply on src/cart.ts:14…"
    );
}

#[test]
fn x_asks_first_then_closes_and_a_closed_pr_reopens() {
    let (model, cmds) = feed(model_of(pr(json!({}))), keys("X"));
    assert_eq!(cmds, []);
    assert!(matches!(
        model.acts().ask(),
        Some(Ask::Close { close: true, .. })
    ));
    let (model, cmds) = feed(model, keys("y"));
    assert_eq!(
        cmds,
        [Cmd::Run(Job::Close {
            url: URL.to_owned(),
            number: 88,
            close: true
        })]
    );
    assert_eq!(model.acts().busy().collect::<Vec<_>>(), ["Closing #88"]);
    let done = Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::Closed {
            close: true,
            login: "ada".to_owned(),
        },
    };
    let (model, _) = feed(model, [Msg::Done(done)]);
    assert_eq!(toast(&model), ("✓ Closed #88 as ada", Tone::Good));

    let closed = model_of(pr(json!({"state": "CLOSED"})));
    assert!(
        closed
            .offers()
            .iter()
            .any(|o| o.key == "X" && o.words == "Reopen the PR")
    );
    let (model, _) = feed(closed, keys("Xn"));
    assert_eq!(model.acts().ask(), None, "n asks nothing more");
    let (model, cmds) = feed(model, keys("Xy"));
    assert!(matches!(
        cmds.as_slice(),
        [Cmd::Run(Job::Close { close: false, .. })]
    ));
    assert_eq!(model.acts().busy().collect::<Vec<_>>(), ["Reopening #88"]);

    let (model, cmds) = feed(model_of(pr(json!({"state": "MERGED"}))), keys("X"));
    assert_eq!(cmds, []);
    assert_eq!(toast(&model).0, "#88 is merged");
    assert!(!model.offers().iter().any(|o| o.key == "X"));
}

#[test]
fn r_offers_actions_checks_that_failed_or_were_skipped_and_starts_them_behind_a_question() {
    let check = |name: &str, conclusion: &str, workflow: Option<&str>| json!({"__typename": "CheckRun", "name": name, "status": "COMPLETED", "conclusion": conclusion, "workflow": workflow});
    let passing = model_of(pr(
        json!({"statusCheckRollup": [check("build", "SUCCESS", Some("CI"))]}),
    ));
    assert!(!passing.offers().iter().any(|o| o.key == "R"));
    let (model, cmds) = feed(passing, keys("R"));
    assert_eq!(cmds, []);
    assert_eq!(
        toast(&model).0,
        "#88 has no failed or skipped Actions check"
    );

    let entry = pr(json!({"statusCheckRollup": [
        check("build", "SUCCESS", Some("CI")),
        check("e2e", "SKIPPED", Some("CI")),
        check("deploy", "FAILURE", Some("Deploy")),
        check("ext", "FAILURE", None),
    ]}));
    let model = model_of(entry);
    let offer = model
        .offers()
        .into_iter()
        .find(|o| o.key == "R")
        .expect("R");
    assert_eq!(offer.words, "Run 2 checks again");
    let (model, _) = feed(model, keys("R"));
    assert!(matches!(
        model.acts().ask(),
        Some(Ask::Rerun { names, .. }) if names == &["e2e".to_owned(), "deploy".to_owned()]
    ));
    let (model, cmds) = feed(model, keys("y"));
    assert_eq!(
        cmds,
        [Cmd::Run(Job::Rerun {
            url: URL.to_owned(),
            number: 88
        })]
    );
    let reran = |ran: &[&str], refused: &[&str]| Done {
        url: URL.to_owned(),
        number: 88,
        outcome: Outcome::Reran {
            ran: ran.iter().map(|&r| r.to_owned()).collect(),
            refused: refused.iter().map(|&r| r.to_owned()).collect(),
        },
    };
    let (model, _) = feed(
        model,
        [Msg::Done(reran(
            &["CI (whole run)", "Deploy (failed jobs)"],
            &[],
        ))],
    );
    assert_eq!(
        toast(&model),
        (
            "✓ Started again on #88: CI (whole run), Deploy (failed jobs)",
            Tone::Good
        )
    );
    let (model, _) = feed(model, [Msg::Done(reran(&[], &[]))]);
    assert_eq!(toast(&model).1, Tone::Info);
    let (model, _) = feed(
        model,
        [Msg::Done(reran(
            &["CI (whole run)"],
            &["Deploy: Couldn't run it again: HTTP 403"],
        ))],
    );
    assert_eq!(toast(&model).1, Tone::Bad);
}
