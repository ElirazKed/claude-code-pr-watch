//! Handing a PR to an agent: which hand-offs a PR has now (Fix, Resolve, Address), the facts
//! every prompt names, and the words shared by the prompts' ports ([`crate::fix`],
//! [`crate::resolve`], [`crate::address`]). The mod drafts a prompt into Claude's input; `prw`
//! copies it for the person to paste into whichever agent runs the PR.

use crate::gh::GhPr;
use crate::review::reviews_of;
use crate::status::{is_conflicted, summarize_checks};
use crate::ui::repo_of;

/// A hand-off, in the order a card offers them: what blocks the merge first (a conflict,
/// then CI), then review (`URGENCY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// The PR conflicts with its base: merge the base in and resolve (`resolve.ts`).
    Resolve,
    /// CI failed: the failing checks and their logs' tails (`fix.ts`).
    Fix,
    /// Review threads are open: what each says (`address.ts`).
    Address,
}

impl Kind {
    /// What the toast and the actions call it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Resolve => "Resolve",
            Self::Fix => "Fix",
            Self::Address => "Address",
        }
    }

    /// The key that picks it where a PR has more than one.
    #[must_use]
    pub const fn key(self) -> char {
        match self {
            Self::Resolve => 'r',
            Self::Fix => 'f',
            Self::Address => 'a',
        }
    }

    /// What it asks for, in a few words ("fix the failing checks").
    #[must_use]
    pub const fn about(self) -> &'static str {
        match self {
            Self::Resolve => "resolve the conflicts",
            Self::Fix => "fix the failing checks",
            Self::Address => "address the review threads",
        }
    }
}

/// The hand-offs a PR has now, most urgent first: Resolve while it conflicts, Fix while a
/// check fails, Address while a review thread is open. None once it is merged or closed.
#[must_use]
pub fn kinds_of(pr: &GhPr) -> Vec<Kind> {
    if pr.state != crate::gh::PrState::Open {
        return Vec::new();
    }
    let unresolved = reviews_of(pr).map_or(0, |r| r.unresolved);
    [
        (Kind::Resolve, is_conflicted(pr)),
        (Kind::Fix, summarize_checks(pr.checks()).failed > 0),
        (Kind::Address, unresolved > 0),
    ]
    .into_iter()
    .filter_map(|(kind, is_on)| is_on.then_some(kind))
    .collect()
}

/// What a prompt says of its PR (the mod's `PrFacts`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrFacts {
    /// `owner/repo`.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    /// The head branch; "" where unknown.
    pub branch: String,
    /// The base branch; "" where unknown.
    pub base: String,
    /// The PR's author's login; "" where unknown.
    pub author: String,
}

impl PrFacts {
    /// The facts of the PR at `url`, as `results.json` has it.
    #[must_use]
    pub fn of(url: &str, pr: &GhPr) -> Self {
        Self {
            repo: repo_of(url),
            number: if pr.number > 0 {
                pr.number
            } else {
                crate::ui::number_of(url)
            },
            title: pr.title.clone(),
            url: url.to_owned(),
            branch: pr.head_ref_name.clone(),
            base: pr.base_ref_name.clone(),
            author: pr
                .author
                .as_ref()
                .map(|a| a.login.clone())
                .unwrap_or_default(),
        }
    }

    /// The head branch, or words for it.
    pub(crate) fn branch(&self) -> &str {
        or_words(&self.branch, "its head branch")
    }

    /// The base branch, or words for it.
    pub(crate) fn base(&self) -> &str {
        or_words(&self.base, "its base")
    }

    /// The title, or words for none.
    pub(crate) fn title(&self) -> &str {
        or_words(&self.title, "untitled")
    }
}

const fn or_words<'a>(text: &'a str, words: &'a str) -> &'a str {
    if text.is_empty() { words } else { text }
}

/// `text` cut to `max` characters, the last one an ellipsis where it was longer (`clip`).
#[must_use]
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Characters, as the caps count them.
pub(crate) fn chars(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use serde_json::json;

    fn pr(value: serde_json::Value) -> GhPr {
        serde_json::from_value(value).expect("a PR")
    }

    #[test]
    fn a_pr_offers_what_it_needs_most_urgent_first() {
        let threads = json!({"totalCount": 2, "nodes": [
            {"isResolved": false, "isOutdated": false}, {"isResolved": true, "isOutdated": false}
        ]});
        let failed = json!([{"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "FAILURE"}]);
        let everything = pr(json!({
            "state": "OPEN", "mergeStateStatus": "DIRTY", "statusCheckRollup": failed,
            "latestReviews": {"nodes": []}, "reviewThreads": threads,
        }));
        assert_eq!(
            kinds_of(&everything),
            vec![Kind::Resolve, Kind::Fix, Kind::Address]
        );
        let clean = pr(json!({"state": "OPEN", "mergeStateStatus": "CLEAN"}));
        assert_eq!(kinds_of(&clean), []);
        let merged = pr(json!({"state": "MERGED", "mergeStateStatus": "DIRTY"}));
        assert_eq!(kinds_of(&merged), []);
    }

    #[test]
    fn facts_come_from_the_pr_and_its_url() {
        let facts = PrFacts::of(
            "https://github.com/acme/app/pull/13",
            &pr(
                json!({"title": "Retry", "headRefName": "feat/retry", "baseRefName": "main", "author": {"login": "octocat"}}),
            ),
        );
        assert_eq!(facts.repo, "acme/app");
        assert_eq!(facts.number, 13);
        assert_eq!((facts.branch(), facts.base()), ("feat/retry", "main"));
        assert_eq!(facts.author, "octocat");
        let bare = PrFacts::default();
        assert_eq!(
            (bare.branch(), bare.base(), bare.title()),
            ("its head branch", "its base", "untitled")
        );
    }

    proptest! {
        #[test]
        fn clip_keeps_short_text_and_cuts_long_text_to_its_cap(text in ".{0,40}", max in 1usize..30) {
            let out = clip(&text, max);
            if chars(&text) <= max {
                prop_assert_eq!(out, text);
            } else {
                prop_assert_eq!(chars(&out), max);
                prop_assert!(out.ends_with('…'));
            }
        }
    }
}
