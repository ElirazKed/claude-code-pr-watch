//! Which watched PR comes first: a port of `hooks/band.ts`'s `byUrgency`.

use crate::gh::PrState;
use crate::status::{Derived, PillKind};

/// Most urgent first (`URGENCY`): what blocks the merge (conflicts, then CI, then review),
/// what the person can act on now, then what waits on others, then what is done.
pub const URGENCY: [PillKind; 16] = [
    PillKind::Conflicts,
    PillKind::CiFailed,
    PillKind::CiFailing,
    PillKind::Changes,
    PillKind::Comments,
    PillKind::Ready,
    PillKind::Behind,
    PillKind::Blocked,
    PillKind::CiRunning,
    PillKind::InReview,
    PillKind::Checking,
    PillKind::AutoMerging,
    PillKind::Draft,
    PillKind::Loading,
    PillKind::Merged,
    PillKind::Closed,
];

/// What ordering needs of a watched PR: the part of the mod's `TrackedPr` a list sorts by,
/// with what names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracked {
    pub url: String,
    pub number: u64,
    pub title: String,
    /// `None` until the PR is first read (the mod's `'UNKNOWN'`).
    pub state: Option<PrState>,
    pub derived: Derived,
    /// What the card says went wrong, if anything.
    pub error: Option<String>,
}

impl Tracked {
    /// A PR watched but not read yet (`placeholder`).
    #[must_use]
    pub fn placeholder(url: String, number: u64) -> Self {
        Self {
            url,
            number,
            title: String::new(),
            state: None,
            derived: Derived::loading(),
            error: None,
        }
    }
}

/// Where a PR ranks, lower first (band.ts's `rank`, doubled so its half steps are whole).
/// A PR GitHub won't show (never read, with an error) is worth a look before the ones that
/// only wait: just before `ready`.
///
/// The TypeScript also ranks a pill it doesn't know (a row an older mod kept) just before
/// `merged`; here a pill is always a [`PillKind`], so that case can't arise.
#[must_use]
pub fn rank(pr: &Tracked) -> usize {
    let index = |kind: PillKind| {
        URGENCY
            .iter()
            .position(|&k| k == kind)
            .unwrap_or(URGENCY.len())
    };
    if pr.error.is_some() && pr.state.is_none() {
        return 2 * index(PillKind::Ready) - 1;
    }

    2 * index(pr.derived.pill)
}

/// The watched PRs, most urgent first; a tie keeps the order they were watched in
/// (`byUrgency`). A stable sort, as `Array.prototype.sort` is.
#[must_use]
pub fn order(mut list: Vec<Tracked>) -> Vec<Tracked> {
    list.sort_by_key(rank);
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(number: u64, pill: PillKind) -> Tracked {
        let mut pr =
            Tracked::placeholder(format!("https://github.com/acme/app/pull/{number}"), number);
        pr.state = Some(PrState::Open);
        pr.derived.pill = pill;
        pr
    }

    #[test]
    fn urgency_names_every_pill_once() {
        for kind in PillKind::ALL {
            assert_eq!(
                URGENCY.iter().filter(|&&k| k == kind).count(),
                1,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn most_urgent_first_and_ties_in_watch_order() {
        let mut unread = Tracked::placeholder("https://github.com/acme/app/pull/9".into(), 9);
        unread.error = Some("Not found".into());
        let list = vec![
            with(1, PillKind::Merged),
            with(2, PillKind::Ready),
            unread,
            with(4, PillKind::Conflicts),
            with(5, PillKind::Ready),
            with(6, PillKind::Comments),
        ];
        let numbers: Vec<u64> = order(list).iter().map(|pr| pr.number).collect();
        assert_eq!(numbers, [4, 6, 9, 2, 5, 1]);
    }
}
