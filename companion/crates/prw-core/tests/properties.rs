//! Properties of the kernel (the plan's Tests section): the parse is total and forgiving,
//! `order` is a stable idempotent permutation, and the check counts add up.

use std::fmt::Debug;

use proptest::prelude::*;
use proptest::sample::Index;
use prw_core::gh::{
    Actor, ActorKind, Author, AutoMergeRequest, CheckItem, CheckKind, CheckStatus, Conclusion,
    Connection, GhMergeMethod, GhPr, GhReview, MergeStateStatus, MergeableState, Permission,
    PrState, RepoMerge, ReviewDecision, ReviewRequest, ReviewState, StatusState, ThreadState,
};
use prw_core::order::{Tracked, order, rank};
use prw_core::protocol::{Bucket, Latest, LoginRates, RateLimit};
use prw_core::review::reviews_of;
use prw_core::status::{
    Mergeable, PillKind, Stage, activity, derive, is_status_context, latest_runs, mergeable_of,
    step_index, summarize_checks,
};
use prw_core::{Entry, Shared, parse_lease, parse_session, parse_shared, urls_in};
use serde_json::{Map, Value};

// ---- Strategies -----------------------------------------------------------------------

/// A GitHub enum: mostly its known values, sometimes one it doesn't know (never spelled like
/// a known one, so a round trip gives the same variant back), sometimes none at all.
fn gh_enum<T: From<String> + Clone + Debug>(
    known: &'static [&'static str],
) -> impl Strategy<Value = T> {
    prop_oneof![
        4 => proptest::sample::select(known).prop_map(|text| T::from(text.to_owned())),
        1 => "x-[a-z]{0,6}".prop_map(T::from),
        1 => Just(T::from(String::new())),
    ]
}

fn text() -> impl Strategy<Value = String> {
    any::<String>()
}

fn opt<S: Strategy>(s: S) -> impl Strategy<Value = Option<S::Value>> {
    proptest::option::of(s)
}

/// A time as JS writes one: a whole number of ms, inside 2^53.
fn ms() -> impl Strategy<Value = i64> {
    -(1_i64 << 53)..(1_i64 << 53)
}

fn stamp() -> impl Strategy<Value = Option<String>> {
    opt(prop_oneof![
        Just("2026-10-07T11:00:00Z".to_owned()),
        Just("2026-10-07T11:10:00Z".to_owned()),
        Just("2026-10-07T11:10:00.500Z".to_owned()),
        Just("not a date".to_owned()),
        text(),
    ])
}

/// JSON with no floats (which needn't read back to the same bits).
fn json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        ".{0,6}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(".{0,4}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

/// Any JSON at all, floats included.
fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<f64>().prop_map(Value::from),
        ".{0,6}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            prop::collection::btree_map(
                prop_oneof![Just("urls".to_owned()), ".{0,4}".prop_map(String::from)],
                inner,
                0..4
            )
            .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

/// Check items whose names, workflows and stamps collide often, so re-runs fold.
fn check_item() -> impl Strategy<Value = CheckItem> {
    let names =
        || opt(proptest::sample::select(&["build", "e2e", "lint"][..]).prop_map(str::to_owned));
    let core = (
        opt(gh_enum::<CheckKind>(&["CheckRun", "StatusContext"])),
        names(),
        names(),
        opt(gh_enum::<CheckStatus>(&[
            "REQUESTED",
            "QUEUED",
            "IN_PROGRESS",
            "COMPLETED",
            "WAITING",
            "PENDING",
        ])),
        opt(gh_enum::<Conclusion>(&[
            "ACTION_REQUIRED",
            "CANCELLED",
            "FAILURE",
            "NEUTRAL",
            "SKIPPED",
            "STALE",
            "STARTUP_FAILURE",
            "SUCCESS",
            "TIMED_OUT",
        ])),
        opt(opt(gh_enum::<StatusState>(&[
            "ERROR", "EXPECTED", "FAILURE", "PENDING", "SUCCESS",
        ]))),
        opt(proptest::sample::select(&["CI", "E2E", "a/b"][..]).prop_map(str::to_owned)),
        stamp(),
        stamp(),
    );
    let more = (
        stamp(),
        opt(any::<i64>()),
        opt(text()),
        opt(text()),
        opt(text()),
        opt(text()),
        opt(text()),
    );
    (core, more).prop_map(
        |(
            (typename, name, context, status, conclusion, state, workflow, started_at, created_at),
            (completed_at, database_id, details_url, title, summary, target_url, description),
        )| CheckItem {
            typename,
            name,
            context,
            status,
            conclusion,
            state,
            workflow,
            started_at,
            created_at,
            completed_at,
            database_id,
            details_url,
            title,
            summary,
            target_url,
            description,
        },
    )
}

fn actor() -> impl Strategy<Value = Actor> {
    (
        opt(gh_enum::<ActorKind>(&[
            "User",
            "Bot",
            "Team",
            "Mannequin",
            "Organization",
        ])),
        opt(prop_oneof![
            proptest::sample::select(
                &[
                    "alice",
                    "bob",
                    "octocat",
                    "copilot-pull-request-reviewer",
                    ""
                ][..]
            )
            .prop_map(str::to_owned),
            text(),
        ]),
        opt(proptest::sample::select(&["core", "docs"][..]).prop_map(str::to_owned)),
    )
        .prop_map(|(typename, login, slug)| Actor {
            typename,
            login,
            slug,
        })
}

fn review() -> impl Strategy<Value = GhReview> {
    (
        opt(actor()),
        gh_enum::<ReviewState>(&[
            "APPROVED",
            "CHANGES_REQUESTED",
            "COMMENTED",
            "DISMISSED",
            "PENDING",
        ]),
        opt(text()),
    )
        .prop_map(|(author, state, submitted_at)| GhReview {
            author,
            state,
            submitted_at,
        })
}

fn connection<S: Strategy>(node: S) -> impl Strategy<Value = Connection<S::Value>> {
    (opt(0_usize..80), prop::collection::vec(node, 0..4))
        .prop_map(|(total_count, nodes)| Connection { total_count, nodes })
}

fn repo() -> impl Strategy<Value = RepoMerge> {
    (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        opt(gh_enum::<GhMergeMethod>(&["MERGE", "SQUASH", "REBASE"])),
        opt(gh_enum::<Permission>(&[
            "READ", "TRIAGE", "WRITE", "MAINTAIN", "ADMIN",
        ])),
    )
        .prop_map(
            |(
                squash_merge_allowed,
                rebase_merge_allowed,
                merge_commit_allowed,
                viewer_default_merge_method,
                viewer_permission,
            )| RepoMerge {
                squash_merge_allowed,
                rebase_merge_allowed,
                merge_commit_allowed,
                viewer_default_merge_method,
                viewer_permission,
            },
        )
}

#[expect(
    clippy::too_many_lines,
    reason = "a strategy for each of GhPr's fields, one per line"
)]
fn gh_pr() -> impl Strategy<Value = GhPr> {
    let head = (
        any::<u64>(),
        text(),
        text(),
        gh_enum::<PrState>(&["OPEN", "MERGED", "CLOSED"]),
        any::<bool>(),
        opt(gh_enum::<ReviewDecision>(&[
            "APPROVED",
            "CHANGES_REQUESTED",
            "REVIEW_REQUIRED",
        ])),
        opt(gh_enum::<MergeStateStatus>(&[
            "BEHIND",
            "BLOCKED",
            "CLEAN",
            "DIRTY",
            "DRAFT",
            "HAS_HOOKS",
            "UNKNOWN",
            "UNSTABLE",
        ])),
        opt(gh_enum::<MergeableState>(&[
            "MERGEABLE",
            "CONFLICTING",
            "UNKNOWN",
        ])),
        opt((
            opt(text()),
            opt(gh_enum::<GhMergeMethod>(&["MERGE", "SQUASH", "REBASE"])),
        )
            .prop_map(|(enabled_at, merge_method)| AutoMergeRequest {
                enabled_at,
                merge_method,
            })),
        opt(any::<bool>()),
        opt(any::<bool>()),
        opt(repo()),
    );
    let body = (
        opt(prop::collection::vec(check_item(), 0..6)),
        opt(text()),
        opt(text()),
        any::<u64>(),
        any::<u64>(),
        text(),
        text(),
        opt(text().prop_map(|login| Author { login })),
    );
    let reviews = (
        opt(connection(review())),
        opt(connection(review())),
        opt(connection(opt(actor()).prop_map(|requested_reviewer| {
            ReviewRequest { requested_reviewer }
        }))),
        opt(connection((any::<bool>(), any::<bool>()).prop_map(
            |(is_resolved, is_outdated)| ThreadState {
                is_resolved,
                is_outdated,
            },
        ))),
    );
    (head, body, reviews).prop_map(
        |(
            (
                number,
                title,
                url,
                state,
                is_draft,
                review_decision,
                merge_state_status,
                mergeable,
                auto_merge_request,
                viewer_can_enable_auto_merge,
                viewer_can_disable_auto_merge,
                repository,
            ),
            (
                status_check_rollup,
                merged_at,
                closed_at,
                additions,
                deletions,
                head_ref_name,
                base_ref_name,
                author,
            ),
            (latest_reviews, latest_opinionated_reviews, review_requests, review_threads),
        )| GhPr {
            number,
            title,
            url,
            state,
            is_draft,
            review_decision,
            merge_state_status,
            mergeable,
            auto_merge_request,
            viewer_can_enable_auto_merge,
            viewer_can_disable_auto_merge,
            repository,
            status_check_rollup,
            merged_at,
            closed_at,
            additions,
            deletions,
            head_ref_name,
            base_ref_name,
            author,
            latest_reviews,
            latest_opinionated_reviews,
            review_requests,
            review_threads,
        },
    )
}

fn entry() -> impl Strategy<Value = Entry> {
    (ms(), opt(gh_pr()), opt(text()), opt(ms()), any::<bool>()).prop_map(
        |(at, pr, error, failed_since, is_unreachable)| Entry {
            at,
            pr,
            error,
            failed_since,
            is_unreachable,
        },
    )
}

fn bucket() -> impl Strategy<Value = Bucket> {
    (ms(), ms(), ms(), ms()).prop_map(|(limit, remaining, reset_at, seen_at)| Bucket {
        limit,
        remaining,
        reset_at,
        seen_at,
    })
}

fn shared() -> impl Strategy<Value = Shared> {
    (
        ms(),
        ms(),
        prop::collection::btree_map(text(), entry(), 0..3),
        prop::collection::btree_map(text(), text(), 0..3),
        opt(
            (ms(), text(), opt(ms())).prop_map(|(remaining, reset_at, limit)| RateLimit {
                remaining,
                reset_at,
                limit,
            }),
        ),
        prop::collection::btree_map(
            text(),
            (opt(bucket()), opt(bucket()), opt(ms())).prop_map(|(graphql, core, asked_at)| {
                LoginRates {
                    graphql,
                    core,
                    asked_at,
                }
            }),
            0..3,
        ),
        (opt(text()), ms()).prop_map(|(version, checked_at)| Latest {
            version,
            checked_at,
        }),
        opt(ms()),
        prop::collection::btree_map("x-[a-z]{1,5}", json(), 0..3),
    )
        .prop_map(
            |(fetched_at, next_at, prs, accounts, rate, rates, latest, held_until, extra)| Shared {
                fetched_at,
                next_at,
                prs,
                accounts,
                rate,
                rates,
                latest,
                held_until,
                extra: extra.into_iter().collect(),
            },
        )
}

// ---- Unknown fields -------------------------------------------------------------------

const SHARED_FIELDS: [&str; 8] = [
    "fetchedAt",
    "nextAt",
    "prs",
    "accounts",
    "rate",
    "rates",
    "latest",
    "heldUntil",
];
/// `results.json`'s maps: their keys are data (URLs, owners, logins), not field names.
const SHARED_MAPS: [&str; 3] = ["prs", "accounts", "rates"];

/// The paths to every object in a serialized [`Shared`] that is a struct, not a map, below
/// the top level (which keeps unknown fields: tested apart).
fn struct_paths(shared: &Value) -> Vec<Vec<String>> {
    fn walk(value: &Value, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        match value {
            Value::Object(fields) => {
                out.push(path.clone());
                for (key, value) in fields {
                    path.push(key.clone());
                    walk(value, path, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    path.push(i.to_string());
                    walk(item, path, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    let Value::Object(top) = shared else {
        return out;
    };
    for (key, value) in top
        .iter()
        .filter(|(key, _)| SHARED_FIELDS.contains(&key.as_str()))
    {
        let mut path = vec![key.clone()];
        if SHARED_MAPS.contains(&key.as_str()) {
            let Value::Object(map) = value else { continue };
            for (k, v) in map {
                path.push(k.clone());
                walk(v, &mut path, &mut out);
                path.pop();
            }
        } else {
            walk(value, &mut path, &mut out);
        }
    }
    out
}

fn object_at<'a>(value: &'a mut Value, path: &[String]) -> &'a mut Map<String, Value> {
    let mut at = value;
    for key in path {
        at = match at {
            Value::Object(fields) => fields.get_mut(key).unwrap(),
            Value::Array(items) => &mut items[key.parse::<usize>().unwrap()],
            _ => unreachable!("a path to an object"),
        };
    }
    at.as_object_mut().unwrap()
}

// ---- Properties -----------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn parse_shared_never_panics_on_any_text(text in any::<String>()) {
        let _ = parse_shared(&text);
        let _ = parse_session(&text);
        let _ = parse_lease(&text);
        let _ = urls_in(&text);
    }

    #[test]
    fn parse_shared_never_panics_on_json_like_text(text in r#"[\[\]{}":,0-9a-zA-Z .eE+-]{0,80}"#) {
        let _ = parse_shared(&text);
        let _ = parse_session(&text);
        let _ = parse_lease(&text);
    }

    #[test]
    fn any_json_reads_or_errs_without_panicking(value in any_json()) {
        let text = value.to_string();
        prop_assert_eq!(parse_shared(&text).is_ok(), value.is_object());
        prop_assert_eq!(parse_session(&text).is_some(), value.is_object());
        prop_assert_eq!(parse_lease(&text).is_some(), value.is_object());
    }

    #[test]
    fn shared_round_trips(shared in shared()) {
        let text = serde_json::to_string(&shared).unwrap();
        prop_assert_eq!(parse_shared(&text).unwrap(), shared);
    }

    #[test]
    fn a_truncated_results_json_is_an_error(shared in shared(), cut in any::<Index>()) {
        let text = serde_json::to_string(&shared).unwrap();
        let boundaries: Vec<usize> = (0..text.len()).filter(|&i| text.is_char_boundary(i)).collect();
        let at = boundaries[cut.index(boundaries.len())];
        prop_assert!(parse_shared(&text[..at]).is_err(), "{:?}", &text[..at]);
    }

    #[test]
    fn an_unknown_field_inside_changes_nothing(shared in shared(), place in any::<Index>(), value in json()) {
        let mut tree = serde_json::to_value(&shared).unwrap();
        let paths = struct_paths(&tree);
        prop_assume!(!paths.is_empty());
        let path = &paths[place.index(paths.len())];
        object_at(&mut tree, path).insert("x-added".into(), value);
        prop_assert_eq!(parse_shared(&tree.to_string()).unwrap(), shared, "at {:?}", path);
    }

    #[test]
    fn an_unknown_top_level_field_is_kept_and_changes_nothing_else(shared in shared(), value in json()) {
        let mut tree = serde_json::to_value(&shared).unwrap();
        tree.as_object_mut().unwrap().insert("x-added".into(), value.clone());
        let mut read = parse_shared(&tree.to_string()).unwrap();
        prop_assert_eq!(read.extra.remove("x-added"), Some(value));
        prop_assert_eq!(read, shared);
    }

    #[test]
    fn urls_in_takes_exactly_the_strings_of_urls(value in any_json(), urls in prop::collection::vec(any_json(), 0..6)) {
        for file in [value, serde_json::json!({ "urls": urls })] {
            let expected: Vec<String> = file
                .get("urls")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(|u| u.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            prop_assert_eq!(urls_in(&file.to_string()), expected);
        }
    }

    #[test]
    fn a_torn_session_file_lists_no_urls(urls in prop::collection::vec(text(), 1..4), cut in any::<Index>()) {
        let text = serde_json::json!({ "urls": urls, "writer": "mod" }).to_string();
        let boundaries: Vec<usize> = (0..text.len()).filter(|&i| text.is_char_boundary(i)).collect();
        prop_assert!(urls_in(&text[..boundaries[cut.index(boundaries.len())]]).is_empty());
    }

    #[test]
    fn the_counts_add_up(items in prop::collection::vec(check_item(), 0..10)) {
        let checks = summarize_checks(&items);
        let latest = latest_runs(&items);
        prop_assert_eq!(checks.total, latest.len());
        prop_assert!(checks.total <= items.len());
        prop_assert_eq!(checks.passed + checks.failed + checks.pending, checks.total);
        prop_assert_eq!(checks.failing.len(), checks.failed);
        prop_assert_eq!(checks.running.len() + checks.queued, checks.pending);
        prop_assert_eq!(activity(&checks).is_empty(), checks.pending == 0);
        prop_assert_eq!(items.is_empty(), checks.total == 0);
    }

    #[test]
    fn latest_runs_keeps_one_of_each_check_in_rollup_order(items in prop::collection::vec(check_item(), 0..10)) {
        let key = |item: &CheckItem| {
            if is_status_context(item) {
                format!("status:{}", item.context.as_deref().unwrap_or(""))
            } else {
                format!("run:{}/{}", item.workflow.as_deref().unwrap_or(""), item.name.as_deref().unwrap_or(""))
            }
        };
        let latest = latest_runs(&items);
        let at: Vec<usize> = latest.iter().map(|&kept| items.iter().position(|item| std::ptr::eq(item, kept)).unwrap()).collect();
        prop_assert!(at.windows(2).all(|w| w[0] < w[1]), "in rollup order: {:?}", at);
        let mut kept: Vec<String> = latest.iter().map(|item| key(item)).collect();
        let mut all: Vec<String> = items.iter().map(key).collect();
        kept.sort();
        all.sort();
        all.dedup();
        prop_assert_eq!(kept, all);
    }

    #[test]
    fn derive_is_total_and_consistent(pr in gh_pr()) {
        let derived = derive(&pr, None);
        let merge = mergeable_of(&pr);
        let _ = reviews_of(&pr);
        let is_done = matches!(pr.state, PrState::Merged | PrState::Closed);
        prop_assert_eq!(derived.stage == Stage::Merged, pr.state == PrState::Merged);
        if derived.checks.pending > 0 && !is_done {
            prop_assert!(derived.is_moving, "pending checks keep a card live");
        }
        prop_assert!(step_index(derived.stage) <= 4);
        prop_assert!(!(merge.can_merge && merge.can_auto));
        prop_assert!(!merge.can_merge || !merge.methods.is_empty());
        if pr.state != PrState::Open {
            prop_assert_eq!(merge, Mergeable::default());
        }
    }

    #[test]
    fn order_is_a_stable_idempotent_permutation(list in prop::collection::vec(tracked(), 0..12)) {
        let list: Vec<Tracked> = list.into_iter().enumerate().map(|(i, mut pr)| { pr.number = i as u64; pr }).collect();
        let once = order(list.clone());
        let mut numbers: Vec<u64> = once.iter().map(|pr| pr.number).collect();
        numbers.sort_unstable();
        prop_assert_eq!(numbers, (0..list.len() as u64).collect::<Vec<_>>());
        prop_assert_eq!(order(once.clone()), once.clone());
        for pair in once.windows(2) {
            let (a, b) = (rank(&pair[0]), rank(&pair[1]));
            prop_assert!(a < b || (a == b && pair[0].number < pair[1].number), "#{} then #{}", pair[0].number, pair[1].number);
        }
    }
}

fn tracked() -> impl Strategy<Value = Tracked> {
    (
        proptest::sample::select(&PillKind::ALL[..]),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(pill, is_read, has_error)| {
            let mut pr = Tracked::placeholder("https://github.com/acme/app/pull/1".into(), 0);
            pr.derived.pill = pill;
            pr.state = is_read.then_some(PrState::Open);
            pr.error = has_error.then(|| "Not found".to_owned());
            pr
        })
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(parse_shared(&deep).is_err());
    let deep_inside = format!(
        r#"{{"prs": {}{}}}"#,
        "[".repeat(100_000),
        "]".repeat(100_000)
    );
    assert!(parse_shared(&deep_inside).is_err());
    assert_eq!(urls_in(&deep_inside), Vec::<String>::new());
    assert_eq!(parse_lease(&deep_inside), None);
}

#[test]
fn a_known_field_never_lands_in_extra() {
    let shared =
        parse_shared(r#"{"prs": 5, "rate": "x", "latest": [], "heldUntil": "soon", "nope": 1}"#)
            .unwrap();
    assert_eq!(shared.extra.keys().collect::<Vec<_>>(), ["nope"]);
}
