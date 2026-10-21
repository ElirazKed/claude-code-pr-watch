//! The responsive layout: which arrangement of panes fits a terminal width.
//!
//! | width (cols) | layout      | what it shows                                  |
//! |--------------|-------------|------------------------------------------------|
//! | `>= 100`     | `Three`     | lists, PR cards and the selected PR, side by side |
//! | `60..=99`    | `Strip`     | the lists as a strip on top, cards below       |
//! | `40..=59`    | `Side`      | one list and the selected PR's card (beside an agent) |
//! | `< 40`       | `TooNarrow` | a "too narrow" screen                          |
//!
//! The thresholds sit between the mockup's three widths (120, 84 and 49
//! columns), so each mockup lands well inside its own band rather than on an
//! edge: 120 is 20 above `THREE_MIN`, 84 is 24 above `STRIP_MIN`, 49 is 9 above
//! `SIDE_MIN`. 40 is the plan's floor for anything useful.

/// The narrowest width that gets three panes.
pub const THREE_MIN: u16 = 100;
/// The narrowest width that gets the lists as a strip.
pub const STRIP_MIN: u16 = 60;
/// The narrowest width that gets one list and a card; anything below is too narrow.
pub const SIDE_MIN: u16 = 40;

/// How the screen is arranged. Ordered from least to most room: a wider
/// terminal never gets a smaller variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layout {
    /// Under 40 columns: nothing fits, say so.
    TooNarrow,
    /// 40 to 59 columns: one list and the selected PR's card.
    Side,
    /// 60 to 99 columns: the lists as a strip, the cards below.
    Strip,
    /// 100 columns and up: three panes side by side.
    Three,
}

impl Layout {
    /// A short lowercase name, for headers and logs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TooNarrow => "too narrow",
            Self::Side => "side",
            Self::Strip => "strip",
            Self::Three => "three panes",
        }
    }
}

/// The layout for a terminal `width` columns wide. Total: every `u16` maps to a layout.
#[must_use]
pub const fn layout(width: u16) -> Layout {
    if width >= THREE_MIN {
        Layout::Three
    } else if width >= STRIP_MIN {
        Layout::Strip
    } else if width >= SIDE_MIN {
        Layout::Side
    } else {
        Layout::TooNarrow
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn the_mockup_widths_get_their_layouts() {
        assert_eq!(layout(120), Layout::Three);
        assert_eq!(layout(84), Layout::Strip);
        assert_eq!(layout(49), Layout::Side);
        assert_eq!(layout(30), Layout::TooNarrow);
    }

    #[test]
    fn each_threshold_is_the_first_width_of_its_band() {
        assert_eq!(layout(0), Layout::TooNarrow);
        assert_eq!(layout(SIDE_MIN - 1), Layout::TooNarrow);
        assert_eq!(layout(SIDE_MIN), Layout::Side);
        assert_eq!(layout(STRIP_MIN - 1), Layout::Side);
        assert_eq!(layout(STRIP_MIN), Layout::Strip);
        assert_eq!(layout(THREE_MIN - 1), Layout::Strip);
        assert_eq!(layout(THREE_MIN), Layout::Three);
        assert_eq!(layout(u16::MAX), Layout::Three);
    }

    #[test]
    fn wider_never_means_smaller_for_every_width() {
        // u16 is small enough to check exhaustively, so the property holds for
        // every width, not just the ones proptest happens to draw.
        for w in 0..u16::MAX {
            assert!(layout(w) <= layout(w + 1), "{w} -> {}", w + 1);
        }
    }

    proptest! {
        #[test]
        fn wider_never_means_a_smaller_layout(a: u16, b: u16) {
            let (narrow, wide) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(layout(narrow) <= layout(wide));
        }

        #[test]
        fn too_narrow_exactly_below_the_floor(w: u16) {
            prop_assert_eq!(layout(w) == Layout::TooNarrow, w < SIDE_MIN);
        }
    }
}
