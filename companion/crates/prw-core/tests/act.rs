//! The action keys, through the model as the binary drives it: what each key asks, what it
//! hands the worker, and what the screen says once the worker answers.

use proptest::prelude::*;
use prw_core::act::{Ask, Done, Job, Look, Outcome, Tone};
use prw_core::handoff::Kind;
use prw_core::merge::{MergeError, MergeRun};
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
    let (model, _) = feed(model, keys("vj"));
    assert_eq!(model.acts().view(), Some(1));
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
    fn every_job_is_for_the_pr_and_no_slot_runs_twice(presses in prop::collection::vec(key(), 0..40), status in prop::sample::select(vec!["CLEAN", "DIRTY", "BLOCKED"])) {
        let mut model = model_of(pr(json!({"mergeStateStatus": status, "viewerCanEnableAutoMerge": true})));
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
