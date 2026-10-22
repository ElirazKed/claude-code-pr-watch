//! A PR's review state as a card shows it: where each reviewer stands and the review
//! threads still open. A port of `hooks/review.ts` (`reviewsOf`, `nameOf`, `unresolved`,
//! `threadLine`, `toAddress`, `threadsBeside`, `verdictLine`).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::gh::{Actor, ActorKind, GhPr, ReviewState};

/// Where a reviewer stands: their approval or change request, a review asked of them, or a
/// comment-only review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Approved,
    Changes,
    Commented,
    Requested,
}

impl Verdict {
    /// The order reviewers are listed in (`RANK`): change requests first.
    const fn rank(self) -> u8 {
        match self {
            Self::Changes => 0,
            Self::Approved => 1,
            Self::Commented => 2,
            Self::Requested => 3,
        }
    }

    /// What a review's state says of its reviewer (`VERDICT`); `None` for the states that
    /// say nothing (dismissed, pending, and any GitHub adds).
    const fn of(state: &ReviewState) -> Option<Self> {
        match state {
            ReviewState::Approved => Some(Self::Approved),
            ReviewState::ChangesRequested => Some(Self::Changes),
            ReviewState::Commented => Some(Self::Commented),
            ReviewState::Dismissed | ReviewState::Pending | ReviewState::Other(_) => None,
        }
    }
}

/// One reviewer, once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reviewer {
    pub login: String,
    pub verdict: Verdict,
}

/// A PR's reviewers and open review threads, as GitHub said at the last check (`Reviews`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviews {
    pub reviewers: Vec<Reviewer>,
    /// Reviewers and requests beyond the ones asked for.
    pub more: usize,
    pub unresolved: usize,
    /// Unresolved threads on code that has changed since.
    pub outdated: usize,
    /// More threads than were asked for: `unresolved` counts the newest only.
    pub is_capped: bool,
}

/// A review app's login cut to its name (`copilot-pull-request-reviewer` → `copilot`), so
/// the line stays short; a person's login stays whole (`nameOf`).
#[must_use]
pub fn name_of(actor: Option<&Actor>) -> String {
    let login = actor
        .and_then(|a| a.login.as_deref().or(a.slug.as_deref()))
        .unwrap_or("");
    let is_bot = actor.is_some_and(|a| a.typename == Some(ActorKind::Bot));
    // `login.length`: UTF-16 code units, as JS counts.
    if is_bot && login.encode_utf16().count() > 12 {
        let head = login.split('-').next().unwrap_or("");
        return if head.is_empty() { login } else { head }.to_owned();
    }

    login.to_owned()
}

fn key_of(actor: Option<&Actor>) -> String {
    match actor {
        Some(a) if a.typename == Some(ActorKind::Team) => {
            format!("team:{}", a.slug.as_deref().unwrap_or(""))
        }
        _ => actor.and_then(|a| a.login.clone()).unwrap_or_default(),
    }
}

/// How many more there are than were shown: never below zero.
fn beyond(total: Option<usize>, shown: usize) -> usize {
    total.unwrap_or(shown).saturating_sub(shown)
}

/// Each reviewer once (`reviewsOf`). An approval or change request stands until its
/// reviewer changes it, as GitHub counts it for the decision; else a pending request; else
/// their last comment. The PR's author is no reviewer (their replies in a thread are reviews
/// too). `None` when the entry has no review facts: an older mod ran that round.
#[must_use]
pub fn reviews_of(pr: &GhPr) -> Option<Reviews> {
    let latest = pr.latest_reviews.as_ref()?;
    let threads = pr.review_threads.as_ref()?;
    let author = pr.author.as_ref().map(|a| a.login.as_str());
    let mut seen = HashSet::new();
    let mut reviewers: Vec<Reviewer> = Vec::new();
    let mut put = |actor: Option<&Actor>, verdict: Option<Verdict>| {
        let key = key_of(actor);
        let Some(verdict) = verdict else { return };
        if key.is_empty() || Some(key.as_str()) == author || seen.contains(&key) {
            return;
        }
        reviewers.push(Reviewer {
            login: name_of(actor),
            verdict,
        });
        seen.insert(key);
    };
    for review in pr.latest_opinionated_reviews.iter().flat_map(|c| &c.nodes) {
        if review.state != ReviewState::Commented {
            put(review.author.as_ref(), Verdict::of(&review.state));
        }
    }
    let requests = pr.review_requests.as_ref();
    for request in requests.iter().flat_map(|c| &c.nodes) {
        put(
            request.requested_reviewer.as_ref(),
            Some(Verdict::Requested),
        );
    }
    for review in &latest.nodes {
        put(review.author.as_ref(), Verdict::of(&review.state));
    }
    // Stable, as Array.prototype.sort is.
    reviewers.sort_by_key(|r| r.verdict.rank());
    let requested_more = requests.map_or(0, |c| beyond(c.total_count, c.nodes.len()));
    let open = threads.nodes.iter().filter(|t| !t.is_resolved);

    Some(Reviews {
        reviewers,
        more: beyond(latest.total_count, latest.nodes.len()).saturating_add(requested_more),
        unresolved: open.clone().count(),
        outdated: open.filter(|t| t.is_outdated).count(),
        is_capped: beyond(threads.total_count, threads.nodes.len()) > 0,
    })
}

/// "3", or "8+" when there are more threads than were asked for: a floor, not a count.
fn count(reviews: &Reviews) -> String {
    format!(
        "{}{}",
        reviews.unresolved,
        if reviews.is_capped { "+" } else { "" }
    )
}

fn plural(reviews: &Reviews, word: &str) -> String {
    let s = if reviews.unresolved == 1 && !reviews.is_capped {
        ""
    } else {
        "s"
    };
    format!("{} {word}{s}", count(reviews))
}

/// "3 unresolved threads", or nothing (`unresolved`).
#[must_use]
pub fn unresolved(reviews: &Reviews) -> String {
    if reviews.unresolved == 0 {
        String::new()
    } else {
        plural(reviews, "unresolved thread")
    }
}

/// "3 comments to address", for the headline (`toAddress`).
#[must_use]
pub fn to_address(reviews: &Reviews) -> String {
    format!("{} to address", plural(reviews, "comment"))
}

/// "3 unresolved threads (1 outdated)": outdated ones are on code that has changed since
/// (`threadLine`). Empty when none is open.
#[must_use]
pub fn thread_line(reviews: &Reviews) -> String {
    let text = unresolved(reviews);
    if text.is_empty() || reviews.outdated == 0 {
        text
    } else {
        format!("{text} ({} outdated)", reviews.outdated)
    }
}

/// What a card says of the threads under its headline (`threadsBeside`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadsBeside {
    /// The whole line ([`thread_line`]), where the headline doesn't count them.
    pub line: String,
    /// Where it does ("3 comments to address"): only how many are outdated, beside it.
    pub note: String,
}

/// The threads line under a headline, or only the outdated count where the headline counts
/// them already (`threadsBeside`).
#[must_use]
pub fn threads_beside(headline: &str, reviews: &Reviews) -> ThreadsBeside {
    let is_counted = reviews.unresolved > 0
        && (headline == to_address(reviews) || headline.contains(&unresolved(reviews)));
    if !is_counted {
        return ThreadsBeside {
            line: thread_line(reviews),
            note: String::new(),
        };
    }
    let note = if reviews.outdated > 0 {
        format!("{} outdated", reviews.outdated)
    } else {
        String::new()
    };

    ThreadsBeside {
        line: String::new(),
        note,
    }
}

impl Verdict {
    /// Its icon on a card (`ICON`).
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Approved => "✓",
            Self::Changes => "✗",
            Self::Commented => "💬",
            Self::Requested => "◷",
        }
    }

    /// What a card says it did (`SAYS`).
    #[must_use]
    pub const fn says(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Changes => "requested changes",
            Self::Commented => "commented",
            Self::Requested => "requested",
        }
    }
}

/// Terminal cells, as the mod counts them (`cells`): a character outside the basic plane
/// (an emoji, 💬) takes two, every other one.
#[must_use]
pub fn cells(text: &str) -> usize {
    text.chars()
        .map(|c| if u32::from(c) > 0xffff { 2 } else { 1 })
        .sum()
}

/// One piece of the reviewers line: its words, and the verdict that colours it (`None`: a
/// "+2 more", dim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub verdict: Option<Verdict>,
}

/// The ` · ` between pieces.
const SEP: usize = 3;

/// The reviewers line, to fit `columns` (`verdictLine`): "✓ alice approved · ✗ bob requested
/// changes" while it fits, else icons and names ("✓ alice · ✗ bob"), as many as fit beside a
/// "+2 more". Always at least one reviewer where there is one: the card cuts the rest.
#[must_use]
pub fn verdict_line(reviews: &Reviews, columns: usize) -> Vec<Piece> {
    let width = |pieces: &[Piece]| {
        pieces.iter().map(|p| cells(&p.text)).sum::<usize>() + SEP * pieces.len().saturating_sub(1)
    };
    let more = |n: usize| {
        (n > 0).then(|| Piece {
            text: format!("+{n} more"),
            verdict: None,
        })
    };
    let piece = |text: String, verdict: Verdict| Piece {
        text,
        verdict: Some(verdict),
    };
    let long: Vec<Piece> = reviews
        .reviewers
        .iter()
        .map(|r| {
            piece(
                format!("{} {} {}", r.verdict.icon(), r.login, r.verdict.says()),
                r.verdict,
            )
        })
        .chain(more(reviews.more))
        .collect();
    if width(&long) <= columns {
        return long;
    }
    let short: Vec<Piece> = reviews
        .reviewers
        .iter()
        .map(|r| piece(format!("{} {}", r.verdict.icon(), r.login), r.verdict))
        .collect();
    let with_more = |shown: usize| -> Vec<Piece> {
        short[..shown]
            .iter()
            .cloned()
            .chain(more(short.len() - shown + reviews.more))
            .collect()
    };
    let mut shown = short.len();
    while shown > 1 && width(&with_more(shown)) > columns {
        shown -= 1;
    }

    with_more(shown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gh::{Author, Connection, GhReview, ReviewRequest, ThreadState};

    fn actor(kind: ActorKind, login: &str) -> Actor {
        Actor {
            typename: Some(kind),
            login: Some(login.into()),
            slug: None,
        }
    }

    fn review(login: &str, state: ReviewState) -> GhReview {
        GhReview {
            author: Some(actor(ActorKind::User, login)),
            state,
            submitted_at: None,
        }
    }

    #[test]
    fn a_review_app_is_named_short_and_a_person_whole() {
        assert_eq!(
            name_of(Some(&actor(
                ActorKind::Bot,
                "copilot-pull-request-reviewer"
            ))),
            "copilot"
        );
        assert_eq!(
            name_of(Some(&actor(ActorKind::Bot, "-starts-with-a-dash"))),
            "-starts-with-a-dash"
        );
        assert_eq!(
            name_of(Some(&actor(ActorKind::User, "a-very-long-person-login"))),
            "a-very-long-person-login"
        );
        assert_eq!(name_of(None), "");
    }

    #[test]
    fn a_stance_outlasts_a_later_comment_and_the_author_is_no_reviewer() {
        let pr = GhPr {
            author: Some(Author {
                login: "octocat".into(),
            }),
            latest_reviews: Some(Connection {
                total_count: Some(5),
                nodes: vec![
                    review("alice", ReviewState::Commented),
                    review("octocat", ReviewState::Commented),
                    review("bob", ReviewState::Commented),
                ],
            }),
            latest_opinionated_reviews: Some(Connection {
                total_count: None,
                nodes: vec![review("alice", ReviewState::ChangesRequested)],
            }),
            review_requests: Some(Connection {
                total_count: Some(1),
                nodes: vec![ReviewRequest {
                    requested_reviewer: Some(Actor {
                        typename: Some(ActorKind::Team),
                        login: None,
                        slug: Some("core".into()),
                    }),
                }],
            }),
            review_threads: Some(Connection {
                total_count: Some(2),
                nodes: vec![
                    ThreadState {
                        is_resolved: false,
                        is_outdated: true,
                    },
                    ThreadState {
                        is_resolved: true,
                        is_outdated: true,
                    },
                ],
            }),
            ..GhPr::default()
        };
        let reviews = reviews_of(&pr).unwrap();
        let said: Vec<_> = reviews
            .reviewers
            .iter()
            .map(|r| (r.login.as_str(), r.verdict))
            .collect();
        assert_eq!(
            said,
            [
                ("alice", Verdict::Changes),
                ("bob", Verdict::Commented),
                ("core", Verdict::Requested)
            ]
        );
        assert_eq!(reviews.more, 2);
        assert_eq!(
            (reviews.unresolved, reviews.outdated, reviews.is_capped),
            (1, 1, false)
        );
        assert_eq!(unresolved(&reviews), "1 unresolved thread");
        assert_eq!(to_address(&reviews), "1 comment to address");
    }

    #[test]
    fn no_review_facts_is_none() {
        assert_eq!(reviews_of(&GhPr::default()), None);
    }

    fn reviewer(login: &str, verdict: Verdict) -> Reviewer {
        Reviewer {
            login: login.into(),
            verdict,
        }
    }

    fn joined(reviews: &Reviews, columns: usize) -> String {
        verdict_line(reviews, columns)
            .into_iter()
            .map(|p| p.text)
            .collect::<Vec<_>>()
            .join(" · ")
    }

    #[test]
    fn the_reviewers_line_fits_as_the_mods_does() {
        // review.test.ts: "too long for the card: names only, then as many as fit beside +N more".
        let reviews = Reviews {
            reviewers: vec![
                reviewer("bob", Verdict::Changes),
                reviewer("alice", Verdict::Approved),
                reviewer("nadav", Verdict::Commented),
                reviewer("dana", Verdict::Commented),
                reviewer("carol", Verdict::Requested),
                reviewer("erin", Verdict::Requested),
            ],
            ..Reviews::default()
        };
        assert_eq!(
            joined(&reviews, 46),
            "✗ bob · ✓ alice · 💬 nadav · 💬 dana · +2 more"
        );
        assert_eq!(joined(&reviews, 25), "✗ bob · ✓ alice · +4 more");
        assert_eq!(joined(&reviews, 5), "✗ bob · +5 more");
        assert_eq!(
            joined(&reviews, 200),
            "✗ bob requested changes · ✓ alice approved · 💬 nadav commented · 💬 dana commented · ◷ carol requested · ◷ erin requested"
        );
        let more = Reviews {
            reviewers: vec![reviewer("nadav", Verdict::Commented)],
            more: 12,
            ..Reviews::default()
        };
        assert_eq!(joined(&more, 100), "💬 nadav commented · +12 more");
        assert_eq!(joined(&Reviews::default(), 100), "");
    }

    #[test]
    fn threads_beside_a_headline_that_counts_them_say_only_the_outdated() {
        let reviews = Reviews {
            unresolved: 3,
            outdated: 1,
            ..Reviews::default()
        };
        assert_eq!(thread_line(&reviews), "3 unresolved threads (1 outdated)");
        let note = |note: &str| ThreadsBeside {
            line: String::new(),
            note: note.into(),
        };
        assert_eq!(
            threads_beside("3 comments to address", &reviews),
            note("1 outdated")
        );
        assert_eq!(
            threads_beside("Approved · 3 unresolved threads", &reviews),
            note("1 outdated")
        );
        assert_eq!(
            threads_beside("Changes requested", &reviews),
            ThreadsBeside {
                line: "3 unresolved threads (1 outdated)".into(),
                note: String::new()
            }
        );
        let capped = Reviews {
            unresolved: 1,
            is_capped: true,
            ..Reviews::default()
        };
        assert_eq!(thread_line(&capped), "1+ unresolved threads");
        assert_eq!(thread_line(&Reviews::default()), "");
    }
}
