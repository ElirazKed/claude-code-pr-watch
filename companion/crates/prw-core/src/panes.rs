//! Where each pane goes, for a layout and a terminal size: the rows and columns the view
//! draws into. Pure arithmetic, so it is held to a property: the panes tile the screen, every
//! cell in exactly one pane, at every size.
//!
//! The widths follow the mockup (`pr-watch-companion.html`): at 120 columns the lists take
//! 24 and the selected PR 38; at 84 the PR takes 36 beside the table; at 49 the card sits
//! under the table, 11 rows high. Between and beyond those, each width scales and clamps.
//!
//! ```text
//! Three (>= 100)            Strip (60..=99)          Side (40..=59)
//! ┌ header ───────────────┐ ┌ header ────────────┐  ┌ header ──────┐
//! │lists│ table  │ detail │ │ strip              │  │ strip        │
//! │     │        │        │ │ table     │ detail │  │ table        │
//! │     │        │        │ │           │        │  │ detail (card)│
//! │ status ───────────────│ │ status ────────────│  │ status ──────│
//! └ hints ────────────────┘ └ hints ─────────────┘  └ hints ───────┘
//! ```
//!
//! The full view ([`full_panes`]) puts one PR in the body: its title and tabs over the tab's
//! body, and at full width its card beside them.
//!
//! ```text
//! Three (>= 100)            Strip and side
//! ┌ header ───────────────┐ ┌ header ────────────┐
//! │ title          │      │ │ title              │
//! │ tabs           │detail│ │ tabs               │
//! │ body           │      │ │ body               │
//! │ status ───────────────│ │ status ────────────│
//! └ hints ────────────────┘ └ hints ─────────────┘
//! ```

use crate::layout::Layout;

/// A rectangle of cells: `x`, `y` its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Area {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Area {
    /// A rectangle.
    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// How many cells it covers.
    #[must_use]
    pub const fn cells(self) -> u32 {
        self.width as u32 * self.height as u32
    }
}

/// What a pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    /// The top row: the name, the tallies, who polled and when.
    Header,
    /// The lists as a pane of their own (three panes only).
    Lists,
    /// The lists as one row (strip and side).
    Strip,
    /// The selected list's PRs.
    Table,
    /// The selected PR: a full card (three panes, strip) or a short one (side).
    Detail,
    /// A line of what's watched, above the hints (where there is room).
    Status,
    /// The key hints, the bottom row.
    Hints,
    /// The whole screen, saying it is too narrow.
    Narrow,
    /// The full view's PR: back, its title, its pill and branch (two rows).
    Title,
    /// The full view's tabs, over a line marking the one showing (two rows).
    Tabs,
    /// What the full view's tab shows.
    Body,
}

/// The widest the lists pane gets.
pub const LISTS_WIDTH: u16 = 24;
/// The status row shows from this height up.
pub const STATUS_MIN_HEIGHT: u16 = 10;
/// The side layout's card, where there is room for it.
pub const CARD_HEIGHT: u16 = 11;

/// The panes of `layout` on a `width` × `height` screen, in drawing order. Together they tile
/// the screen: every cell is in exactly one pane, none reaches outside it. A pane may be
/// empty (no rows) on a very short screen. `TooNarrow` is one pane, the whole screen.
#[must_use]
pub fn panes(layout: Layout, width: u16, height: u16) -> Vec<(Pane, Area)> {
    let full = Area::new(0, 0, width, height);
    if layout == Layout::TooNarrow {
        return vec![(Pane::Narrow, full)];
    }
    // Rows, top-down, each taking what it needs of what is left: the header first, then
    // the hints (the bottom row), the status, the strip; the body takes the rest.
    let mut left = height;
    let mut take = |want: u16| {
        let got = want.min(left);
        left -= got;
        got
    };
    let header = take(1);
    let hints = take(1);
    let status = take(u16::from(height >= STATUS_MIN_HEIGHT));
    let strip = take(u16::from(layout != Layout::Three));
    let body = left;
    let body_y = header + strip;
    let row = |y: u16, h: u16| Area::new(0, y, width, h);

    let mut out = vec![(Pane::Header, row(0, header))];
    if layout != Layout::Three {
        out.push((Pane::Strip, row(header, strip)));
    }
    match layout {
        Layout::Three => {
            let lists = LISTS_WIDTH.min(width);
            let detail = detail_width(width).min(width - lists);
            let table = width - lists - detail;
            out.push((Pane::Lists, Area::new(0, body_y, lists, body)));
            out.push((Pane::Table, Area::new(lists, body_y, table, body)));
            out.push((Pane::Detail, Area::new(lists + table, body_y, detail, body)));
        }
        Layout::Strip => {
            let detail = (width / 7 * 3 + width % 7 * 3 / 7).max(28).min(width);
            let table = width - detail;
            out.push((Pane::Table, Area::new(0, body_y, table, body)));
            out.push((Pane::Detail, Area::new(table, body_y, detail, body)));
        }
        Layout::Side | Layout::TooNarrow => {
            // The card keeps its 11 rows while the table keeps as many; below that they halve.
            let card = if body >= 2 * CARD_HEIGHT {
                CARD_HEIGHT
            } else {
                body.div_ceil(2)
            };
            let table = body - card;
            out.push((Pane::Table, row(body_y, table)));
            out.push((Pane::Detail, row(body_y + table, card)));
        }
    }
    out.push((Pane::Status, row(body_y + body, status)));
    out.push((Pane::Hints, row(body_y + body + status, hints)));

    out
}

/// The selected PR's card at full width: 38 of 120 columns, scaled, within 34 to 56.
const fn detail_width(width: u16) -> u16 {
    let scaled = width / 120 * 38 + width % 120 * 38 / 120;
    if scaled < 34 {
        34
    } else if scaled > 56 {
        56
    } else {
        scaled
    }
}

/// The full view's panes of `layout` on a `width` × `height` screen, in drawing order: tiling
/// the screen as [`panes`] do, the header, status and hints rows where they are there.
#[must_use]
pub fn full_panes(layout: Layout, width: u16, height: u16) -> Vec<(Pane, Area)> {
    if layout == Layout::TooNarrow {
        return vec![(Pane::Narrow, Area::new(0, 0, width, height))];
    }
    let mut left = height;
    let mut take = |want: u16| {
        let got = want.min(left);
        left -= got;
        got
    };
    let header = take(1);
    let hints = take(1);
    let status = take(u16::from(height >= STATUS_MIN_HEIGHT));
    let title = take(2);
    let tabs = take(2);
    let body = left;
    let side = if layout == Layout::Three {
        detail_width(width).min(width)
    } else {
        0
    };
    let main = width - side;
    let col = |y: u16, h: u16| Area::new(0, y, main, h);
    let mut out = vec![
        (Pane::Header, Area::new(0, 0, width, header)),
        (Pane::Title, col(header, title)),
        (Pane::Tabs, col(header + title, tabs)),
        (Pane::Body, col(header + title + tabs, body)),
    ];
    if side > 0 {
        out.push((
            Pane::Detail,
            Area::new(main, header, side, title + tabs + body),
        ));
    }
    let below = header + title + tabs + body;
    out.push((Pane::Status, Area::new(0, below, width, status)));
    out.push((Pane::Hints, Area::new(0, below + status, width, hints)));
    out
}

/// The area of `pane` in `panes`, if it has one.
#[must_use]
pub fn area_of(panes: &[(Pane, Area)], pane: Pane) -> Option<Area> {
    panes
        .iter()
        .find(|(p, _)| *p == pane)
        .map(|&(_, area)| area)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::layout;
    use proptest::prelude::*;

    fn widths(width: u16, height: u16) -> Vec<(Pane, u16)> {
        panes(layout(width), width, height)
            .into_iter()
            .filter(|(p, _)| matches!(p, Pane::Lists | Pane::Table | Pane::Detail))
            .map(|(p, a)| (p, a.width))
            .collect()
    }

    #[test]
    fn the_mockup_sizes_get_the_mockups_panes() {
        assert_eq!(
            widths(120, 36),
            [(Pane::Lists, 24), (Pane::Table, 58), (Pane::Detail, 38)]
        );
        assert_eq!(widths(84, 36), [(Pane::Table, 48), (Pane::Detail, 36)]);
        assert_eq!(widths(49, 36), [(Pane::Table, 49), (Pane::Detail, 49)]);
        let side = panes(Layout::Side, 49, 36);
        assert_eq!(area_of(&side, Pane::Detail).map(|a| a.height), Some(11));
        assert_eq!(area_of(&side, Pane::Table).map(|a| a.height), Some(21));
        assert_eq!(
            panes(Layout::TooNarrow, 30, 10),
            [(Pane::Narrow, Area::new(0, 0, 30, 10))]
        );
    }

    #[test]
    fn the_full_view_keeps_the_card_beside_it_only_at_full_width() {
        let full = full_panes(Layout::Three, 120, 36);
        assert_eq!(area_of(&full, Pane::Detail), Some(Area::new(82, 1, 38, 33)));
        assert_eq!(area_of(&full, Pane::Body), Some(Area::new(0, 5, 82, 29)));
        let half = full_panes(Layout::Strip, 84, 36);
        assert_eq!(area_of(&half, Pane::Detail), None);
        assert_eq!(area_of(&half, Pane::Body), Some(Area::new(0, 5, 84, 29)));
    }

    /// How many panes of `of` cover each cell.
    fn coverage(of: fn(Layout, u16, u16) -> Vec<(Pane, Area)>, width: u16, height: u16) -> Vec<u8> {
        let mut seen = vec![0_u8; usize::from(width) * usize::from(height)];
        for (_, a) in of(layout(width), width, height) {
            assert!(
                u32::from(a.x) + u32::from(a.width) <= u32::from(width),
                "{a:?}"
            );
            assert!(
                u32::from(a.y) + u32::from(a.height) <= u32::from(height),
                "{a:?}"
            );
            for y in a.y..a.y + a.height {
                for x in a.x..a.x + a.width {
                    seen[usize::from(y) * usize::from(width) + usize::from(x)] += 1;
                }
            }
        }
        seen
    }

    proptest! {
        #[test]
        fn panes_tile_the_screen_exactly(width in 40_u16..=300, height in 0_u16..=80) {
            let seen = coverage(panes, width, height);
            prop_assert!(seen.iter().all(|&n| n == 1), "{width}x{height}");
        }

        #[test]
        fn the_full_views_panes_tile_the_screen_exactly(width in 0_u16..=300, height in 0_u16..=80) {
            let seen = coverage(full_panes, width, height);
            prop_assert!(seen.iter().all(|&n| n == 1), "{width}x{height}");
        }

        #[test]
        fn the_side_by_side_panes_tile_the_width(width in 40_u16..=u16::MAX, height in 3_u16..=200) {
            let sum: u32 = widths(width, height)
                .iter()
                .filter(|(p, _)| layout(width) != Layout::Side || *p == Pane::Table)
                .map(|&(_, w)| u32::from(w))
                .sum();
            prop_assert_eq!(sum, u32::from(width));
        }

        #[test]
        fn a_wider_screen_never_gets_a_smaller_layout_or_a_narrower_card(a in 40_u16..=400, b in 40_u16..=400) {
            let (narrow, wide) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(layout(narrow) <= layout(wide));
            if layout(narrow) == layout(wide) {
                let detail = |w| area_of(&panes(layout(w), w, 30), Pane::Detail).map(|a| a.width);
                prop_assert!(detail(narrow) <= detail(wide));
            }
        }
    }
}
