//! Your own lists: Focus, the PRs you picked to watch now (`p`), and Later, the ones put
//! aside (`z`) until something changes on them. Kept in `lists.json` (the binary's
//! `$XDG_STATE_HOME/prw/lists.json`), never in the cache: they are yours, not something
//! fetched. A PR in either is announced to the poll (`kind: "lists"`), so it stays fetched
//! with no session watching it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::batch::ref_from_url;
use crate::status::PillKind;

/// Where a PR put aside came from, and so where it goes back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Place {
    Focus,
    /// Wherever else it was listed: the Inbox, My PRs, a saved search.
    #[default]
    Inbox,
}

/// A PR in Later, and how it was when put there: it comes back once that changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Later {
    pub url: String,
    /// When it was put aside (ms).
    #[serde(default)]
    pub at: i64,
    #[serde(default)]
    pub from: Place,
    /// Its pill then: one this version doesn't know (a newer `prw` wrote it) reads as not
    /// read yet, so the entry is kept and its next read fills it in.
    #[serde(deserialize_with = "pill_or_loading")]
    pub pill: PillKind,
    /// When GitHub said anything last changed on it then (ms), where it said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

fn pill_or_loading<'de, D: serde::Deserializer<'de>>(de: D) -> Result<PillKind, D::Error> {
    let raw = Value::deserialize(de)?;
    Ok(serde_json::from_value(raw).unwrap_or(PillKind::Loading))
}

/// `lists.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Lists {
    /// In the order they were pinned.
    pub focus: Vec<String>,
    /// In the order they were put aside.
    pub later: Vec<Later>,
}

/// How a PR is now, as Later compares it: its pill, and when it last changed (ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    pub pill: PillKind,
    pub updated_at: Option<i64>,
}

/// A pill that says nothing about the PR itself: not read yet, or GitHub still working out
/// whether it merges. A change to or from one wakes nothing.
const fn is_passing(pill: PillKind) -> bool {
    matches!(pill, PillKind::Loading | PillKind::Checking)
}

/// `lists.json`'s text, leniently: what doesn't read is left out (a URL that isn't a PR's, a
/// second copy of one, an entry torn), and a file that doesn't parse at all is no lists.
#[must_use]
pub fn parse_lists(text: &str) -> Lists {
    let Ok(Value::Object(raw)) = serde_json::from_str::<Value>(text) else {
        return Lists::default();
    };
    let is_pr = |url: &str| ref_from_url(url).is_some();
    let mut lists = Lists::default();
    for url in raw
        .get("focus")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if is_pr(url) && !lists.is_focus(url) {
            lists.focus.push(url.to_owned());
        }
    }
    for entry in raw
        .get("later")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Ok(later) = serde_json::from_value::<Later>(entry.clone()) else {
            continue;
        };
        // One place for a PR: Later wins over Focus, as `z` takes it out of Focus.
        if is_pr(&later.url) && !lists.is_later(&later.url) {
            lists.focus.retain(|u| *u != later.url);
            lists.later.push(later);
        }
    }
    lists
}

impl Lists {
    #[must_use]
    pub fn is_focus(&self, url: &str) -> bool {
        self.focus.iter().any(|u| u == url)
    }

    #[must_use]
    pub fn is_later(&self, url: &str) -> bool {
        self.later.iter().any(|l| l.url == url)
    }

    /// Whether neither list has anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.focus.is_empty() && self.later.is_empty()
    }

    /// Every PR in either, for the poll: Focus's, then Later's.
    #[must_use]
    pub fn urls(&self) -> Vec<String> {
        self.focus
            .iter()
            .cloned()
            .chain(self.later.iter().map(|l| l.url.clone()))
            .collect()
    }

    /// Pins a PR to Focus (out of Later, where it was), or unpins it. Whether it is in Focus
    /// now.
    pub fn toggle_focus(&mut self, url: &str) -> bool {
        if self.is_focus(url) {
            self.focus.retain(|u| u != url);
            return false;
        }
        self.later.retain(|l| l.url != url);
        self.focus.push(url.to_owned());
        true
    }

    /// Puts a PR aside, as it is `now`: out of Focus, where it was (and back there when it
    /// comes back).
    pub fn put_aside(&mut self, url: &str, now: Now, at: i64) {
        let from = if self.is_focus(url) {
            Place::Focus
        } else {
            Place::Inbox
        };
        self.focus.retain(|u| u != url);
        self.later.retain(|l| l.url != url);
        self.later.push(Later {
            url: url.to_owned(),
            at,
            from,
            pill: now.pill,
            updated_at: now.updated_at,
        });
    }

    /// Takes a PR out of Later, back to Focus where it came from there, or where `to_focus`
    /// (nothing else would list it). Whether it was in Later.
    pub fn bring_back(&mut self, url: &str, to_focus: bool) -> bool {
        let Some(i) = self.later.iter().position(|l| l.url == url) else {
            return false;
        };
        let later = self.later.remove(i);
        if (later.from == Place::Focus || to_focus) && !self.is_focus(url) {
            self.focus.push(later.url);
        }
        true
    }

    /// What wasn't known of a PR when it was put aside, as it reads now: its pill, where it
    /// wasn't read yet (or GitHub was working out whether it merges), and when it last
    /// changed, where GitHub hadn't said. From then on a change wakes it. Whether any was
    /// filled in.
    pub fn fill_in(&mut self, now_of: impl Fn(&str) -> Option<Now>) -> bool {
        let mut is_filled = false;
        for later in &mut self.later {
            let Some(now) = now_of(&later.url) else {
                continue;
            };
            if is_passing(later.pill) && !is_passing(now.pill) {
                later.pill = now.pill;
                is_filled = true;
            }
            if later.updated_at.is_none() && now.updated_at.is_some() {
                later.updated_at = now.updated_at;
                is_filled = true;
            }
        }
        is_filled
    }

    /// The PRs in Later that changed since they were put aside, as `now_of` says each is now:
    /// a new pill (CI done, a review, a conflict, merged), or GitHub saying something changed
    /// later than it did (a push, a comment). One not read now, or read only as loading or
    /// still being worked out, waits.
    pub fn changed(&self, now_of: impl Fn(&str) -> Option<Now>) -> Vec<&str> {
        self.later
            .iter()
            .filter(|later| {
                let Some(now) = now_of(&later.url) else {
                    return false;
                };
                if is_passing(now.pill) {
                    return false;
                }
                let is_new_pill = now.pill != later.pill && !is_passing(later.pill);
                let is_newer = match (later.updated_at, now.updated_at) {
                    (Some(was), Some(is)) => is > was,
                    _ => false,
                };
                is_new_pill || is_newer
            })
            .map(|later| later.url.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "https://github.com/acme/app/pull/1";
    const B: &str = "https://github.com/acme/app/pull/2";

    fn now(pill: PillKind, updated_at: Option<i64>) -> Now {
        Now { pill, updated_at }
    }

    #[test]
    fn a_file_reads_leniently_and_a_pr_is_in_one_list_once() {
        let text = format!(
            r#"{{"focus": ["{A}", "{A}", "not a pr", 3, "{B}"],
                "later": [{{"url": "{B}", "pill": "ciFailed", "updatedAt": 5}},
                          {{"url": "{B}", "pill": "ready"}}, {{"url": "{A}"}}, "torn"]}}"#
        );
        let lists = parse_lists(&text);
        assert_eq!(lists.focus, [A]);
        assert_eq!(lists.later.len(), 1);
        assert_eq!(lists.later[0].pill, PillKind::CiFailed);
        assert_eq!(lists.later[0].from, Place::Inbox);
        assert_eq!(lists.later[0].updated_at, Some(5));
        assert_eq!(parse_lists("{torn"), Lists::default());
        // A pill a newer prw wrote keeps the entry, as not read yet.
        let newer = parse_lists(&format!(
            r#"{{"later": [{{"url": "{A}", "pill": "someday"}}]}}"#
        ));
        assert_eq!(newer.later[0].pill, PillKind::Loading);
        assert_eq!(parse_lists("[]"), Lists::default());
    }

    #[test]
    fn what_is_written_reads_back() {
        let mut lists = Lists::default();
        lists.toggle_focus(A);
        lists.put_aside(B, now(PillKind::InReview, Some(9)), 100);
        let text = serde_json::to_string(&lists).unwrap();
        assert_eq!(parse_lists(&text), lists);
        assert_eq!(lists.urls(), [A, B]);
    }

    #[test]
    fn p_pins_and_unpins_and_takes_a_pr_out_of_later() {
        let mut lists = Lists::default();
        lists.put_aside(A, now(PillKind::Ready, None), 0);
        assert!(lists.toggle_focus(A));
        assert!(lists.is_focus(A) && !lists.is_later(A));
        assert!(!lists.toggle_focus(A));
        assert!(lists.is_empty());
    }

    #[test]
    fn a_pr_put_aside_from_focus_goes_back_there() {
        let mut lists = Lists::default();
        lists.toggle_focus(A);
        lists.put_aside(A, now(PillKind::Ready, None), 0);
        lists.put_aside(B, now(PillKind::Ready, None), 0);
        assert_eq!(lists.focus, [""; 0]);
        assert!(lists.bring_back(A, false));
        assert!(lists.bring_back(B, false));
        assert_eq!(lists.focus, [A]);
        assert!(!lists.bring_back(B, false));
        // One nothing else would list goes to Focus.
        lists.put_aside(B, now(PillKind::Ready, None), 0);
        lists.bring_back(B, true);
        assert_eq!(lists.focus, [A, B]);
    }

    #[test]
    fn a_new_pill_or_a_later_change_wakes_a_pr_and_nothing_else_does() {
        let mut lists = Lists::default();
        lists.put_aside(A, now(PillKind::CiRunning, Some(10)), 0);
        let changed = |pill, updated_at| lists.changed(|_| Some(now(pill, updated_at))).len();
        assert_eq!(changed(PillKind::CiRunning, Some(10)), 0);
        assert_eq!(changed(PillKind::CiRunning, None), 0);
        assert_eq!(changed(PillKind::CiFailed, Some(10)), 1);
        assert_eq!(changed(PillKind::CiRunning, Some(11)), 1);
        // Being worked out says nothing, nor does a PR not read.
        assert_eq!(changed(PillKind::Checking, Some(11)), 0);
        assert_eq!(changed(PillKind::Loading, Some(11)), 0);
        assert_eq!(lists.changed(|_| None), [""; 0]);
    }

    #[test]
    fn a_pr_put_aside_before_it_was_read_takes_its_first_read_as_how_it_was() {
        let mut lists = Lists::default();
        lists.put_aside(A, now(PillKind::Loading, None), 0);
        let read = |_: &str| Some(now(PillKind::Ready, Some(1)));
        assert_eq!(lists.changed(read), [""; 0]);
        assert!(lists.fill_in(read));
        assert_eq!(lists.later[0].pill, PillKind::Ready);
        assert_eq!(lists.later[0].updated_at, Some(1));
        assert!(!lists.fill_in(read));
        assert_eq!(lists.changed(read), [""; 0]);
        assert_eq!(
            lists.changed(|_| Some(now(PillKind::CiRunning, Some(1)))),
            [A]
        );
        // Still loading: nothing to fill in.
        lists.put_aside(B, now(PillKind::Loading, None), 0);
        assert!(!lists.fill_in(|_| Some(now(PillKind::Loading, None))));
        // Put aside before GitHub said when it changed: the first time it says is how it was.
        let mut lists = Lists::default();
        lists.put_aside(A, now(PillKind::Ready, None), 0);
        let said = |_: &str| Some(now(PillKind::Ready, Some(7)));
        assert_eq!(lists.changed(said), [""; 0]);
        assert!(lists.fill_in(said));
        assert_eq!(lists.later[0].updated_at, Some(7));
        assert_eq!(lists.changed(|_| Some(now(PillKind::Ready, Some(8)))), [A]);
    }
}
