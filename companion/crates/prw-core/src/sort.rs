//! The order a list's rows are shown in: most urgent first (as the mod orders them), or by
//! repo, by when each PR was opened, or by when it last changed; `s` steps through them and
//! `S` turns the one shown around.

use std::cmp::Reverse;

use crate::repos::name_of;
use crate::ui::Row;

/// What the rows go by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortKey {
    /// Most urgent first (`order`), as the mod's band and pane list them.
    #[default]
    Urgency,
    /// Repo names A to Z (namesakes together, by owner), each repo's PRs most urgent first.
    Repo,
    /// Oldest first.
    Opened,
    /// The longest quiet first.
    Updated,
}

/// A key and which way round it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Sort {
    pub key: SortKey,
    /// The other way round: least urgent, Z to A, newest, or the latest activity first.
    pub is_flipped: bool,
}

impl Sort {
    /// The next key, the right way round (`s`); after `Updated`, urgency again.
    #[must_use]
    pub const fn next(self) -> Self {
        let key = match self.key {
            SortKey::Urgency => SortKey::Repo,
            SortKey::Repo => SortKey::Opened,
            SortKey::Opened => SortKey::Updated,
            SortKey::Updated => SortKey::Urgency,
        };
        Self {
            key,
            is_flipped: false,
        }
    }

    /// The same key the other way round (`S`).
    #[must_use]
    pub const fn flipped(self) -> Self {
        Self {
            key: self.key,
            is_flipped: !self.is_flipped,
        }
    }

    /// How the table's title says it; `None` for the usual order, which goes unsaid.
    #[must_use]
    pub const fn words(self) -> Option<&'static str> {
        Some(match (self.key, self.is_flipped) {
            (SortKey::Urgency, false) => return None,
            (SortKey::Urgency, true) => "least urgent first",
            (SortKey::Repo, false) => "by repo",
            (SortKey::Repo, true) => "by repo, Z to A",
            (SortKey::Opened, false) => "oldest first",
            (SortKey::Opened, true) => "newest first",
            (SortKey::Updated, false) => "quietest first",
            (SortKey::Updated, true) => "latest activity first",
        })
    }
}

/// `rows` (most urgent first, as `lists_of` gives them) in `sort`'s order. A stable sort, so
/// rows that tie (one repo's PRs, or the same time) stay most urgent first; a row with no
/// time yet goes last whichever way round.
#[must_use]
pub fn sorted(mut rows: Vec<Row>, sort: Sort) -> Vec<Row> {
    let flip = |order: std::cmp::Ordering| {
        if sort.is_flipped {
            order.reverse()
        } else {
            order
        }
    };
    match sort.key {
        SortKey::Urgency if sort.is_flipped => rows.reverse(),
        SortKey::Urgency => {}
        // Each key worked out once, not on every comparison; both sorts are stable.
        SortKey::Repo if sort.is_flipped => rows.sort_by_cached_key(|r| Reverse(repo_key(r))),
        SortKey::Repo => rows.sort_by_cached_key(repo_key),
        SortKey::Opened => rows.sort_by(|a, b| by_time(a.opened_at, b.opened_at, flip)),
        SortKey::Updated => rows.sort_by(|a, b| by_time(a.updated_at, b.updated_at, flip)),
    }
    rows
}

/// What the repo order goes by: the repo's name, then its owner (ignoring case), so a repo
/// shown with its owner (it has a namesake) sorts beside its namesakes, not under the owner.
fn repo_key(row: &Row) -> (String, String) {
    let repo = row.repo.to_lowercase();
    (name_of(&repo).to_owned(), repo)
}

/// Earlier first (or, flipped, later first), and no time after any time either way.
fn by_time(
    a: Option<i64>,
    b: Option<i64>,
    flip: impl Fn(std::cmp::Ordering) -> std::cmp::Ordering,
) -> std::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => flip(a.cmp(&b)),
        (a, b) => a.is_none().cmp(&b.is_none()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::Tracked;
    use proptest::prelude::*;

    fn row(i: u64, repo: &str, opened: Option<i64>, updated: Option<i64>) -> Row {
        let url = format!("https://github.com/acme/{repo}/pull/{i}");
        Row {
            pr: Tracked::placeholder(url, i),
            repo: format!("acme/{repo}"),
            repo_name: repo.to_owned(),
            agent: None,
            watchers: Vec::new(),
            checked_at: None,
            opened_at: opened,
            updated_at: updated,
        }
    }

    fn numbers(rows: &[Row]) -> Vec<u64> {
        rows.iter().map(|r| r.pr.number).collect()
    }

    #[test]
    fn s_steps_through_the_keys_the_right_way_round_and_s_back_to_urgency() {
        let flipped = Sort::default().next().flipped();
        assert_eq!(flipped.words(), Some("by repo, Z to A"));
        let keys: Vec<SortKey> = std::iter::successors(Some(flipped), |s| Some(s.next()))
            .skip(1)
            .take(4)
            .map(|s| s.key)
            .collect();
        assert_eq!(
            keys,
            [
                SortKey::Opened,
                SortKey::Updated,
                SortKey::Urgency,
                SortKey::Repo
            ]
        );
        assert_eq!(Sort::default().words(), None);
    }

    #[test]
    fn by_repo_keeps_each_repos_prs_most_urgent_first() {
        let rows = vec![
            row(1, "web", None, None),
            row(2, "Api", None, None),
            row(3, "web", None, None),
            row(4, "api", None, None),
        ];
        let by_repo = Sort::default().next();
        assert_eq!(numbers(&sorted(rows.clone(), by_repo)), [2, 4, 1, 3]);
        assert_eq!(numbers(&sorted(rows, by_repo.flipped())), [1, 3, 2, 4]);
    }

    #[test]
    fn namesakes_sort_beside_each_other_by_their_name_not_their_owner() {
        let mut rows = vec![
            row(1, "web", None, None),
            row(2, "api", None, None),
            row(3, "web", None, None),
        ];
        // `zed/web` is shown with its owner (a namesake), and still sorts under `web`.
        rows[2].repo = "zed/web".to_owned();
        rows[2].repo_name = "zed/web".to_owned();
        rows[0].repo_name = "acme/web".to_owned();
        let by_repo = Sort::default().next();
        assert_eq!(numbers(&sorted(rows, by_repo)), [2, 1, 3]);
    }

    #[test]
    fn a_pr_with_no_time_yet_goes_last_either_way_round() {
        let rows = vec![
            row(1, "web", None, None),
            row(2, "web", Some(20), Some(5)),
            row(3, "web", Some(10), Some(9)),
        ];
        let oldest = Sort {
            key: SortKey::Opened,
            is_flipped: false,
        };
        assert_eq!(numbers(&sorted(rows.clone(), oldest)), [3, 2, 1]);
        assert_eq!(numbers(&sorted(rows.clone(), oldest.flipped())), [2, 3, 1]);
        let quiet = oldest.next();
        assert_eq!(numbers(&sorted(rows, quiet)), [2, 3, 1]);
    }

    fn sorts() -> impl Strategy<Value = Sort> {
        let key = prop_oneof![
            Just(SortKey::Urgency),
            Just(SortKey::Repo),
            Just(SortKey::Opened),
            Just(SortKey::Updated),
        ];
        (key, any::<bool>()).prop_map(|(key, is_flipped)| Sort { key, is_flipped })
    }

    fn rows() -> impl Strategy<Value = Vec<Row>> {
        let one = (
            prop::sample::select(vec!["api", "web", "Web", "docs"]),
            prop::option::of(0_i64..5),
            prop::option::of(0_i64..5),
        );
        prop::collection::vec(one, 0..12).prop_map(|rows| {
            rows.into_iter()
                .zip(1..)
                .map(|((repo, opened, updated), i)| row(i, repo, opened, updated))
                .collect()
        })
    }

    proptest! {
        #[test]
        fn sorting_only_moves_rows(rows in rows(), sort in sorts()) {
            let mut was = numbers(&rows);
            let mut are = numbers(&sorted(rows, sort));
            was.sort_unstable();
            are.sort_unstable();
            prop_assert_eq!(was, are);
        }

        #[test]
        fn rows_that_tie_stay_most_urgent_first(rows in rows(), sort in sorts()) {
            prop_assume!(sort.key != SortKey::Urgency);
            let key = |r: &Row| match sort.key {
                SortKey::Repo => Some(format!("{:?}", repo_key(r))),
                SortKey::Opened => Some(format!("{:?}", r.opened_at)),
                _ => Some(format!("{:?}", r.updated_at)),
            };
            let are = sorted(rows, sort);
            for (i, a) in are.iter().enumerate() {
                for b in &are[i + 1..] {
                    if key(a) == key(b) {
                        // Numbers were given in urgency order.
                        prop_assert!(a.pr.number < b.pr.number);
                    }
                }
            }
        }

        #[test]
        fn flipped_twice_is_the_same_order(rows in rows(), sort in sorts()) {
            prop_assert_eq!(
                numbers(&sorted(rows.clone(), sort.flipped().flipped())),
                numbers(&sorted(rows, sort))
            );
        }
    }
}
