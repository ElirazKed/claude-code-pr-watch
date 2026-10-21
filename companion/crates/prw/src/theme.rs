//! The colours: the mockup's palette (one committed dark look), by what they mean.
//!
//! Plain 24-bit RGB. A terminal without true colour approximates them; nothing on screen
//! depends on colour alone (pills carry a glyph and words, the selection a `▌`).

use prw_core::status::{PillKind, Tone};
use ratatui::style::{Color, Modifier, Style};

pub const INK: Color = Color::Rgb(0x10, 0x12, 0x1B);
pub const PANEL: Color = Color::Rgb(0x15, 0x17, 0x22);
pub const LINE: Color = Color::Rgb(0x26, 0x2A, 0x3E);
pub const SEL: Color = Color::Rgb(0x1E, 0x21, 0x32);
pub const FG: Color = Color::Rgb(0xEC, 0xEA, 0xF4);
pub const BODY: Color = Color::Rgb(0xC9, 0xC7, 0xDA);
pub const DIM: Color = Color::Rgb(0x7E, 0x7C, 0x96);
pub const ACCENT: Color = Color::Rgb(0xCB, 0xA6, 0xF7);
pub const RED: Color = Color::Rgb(0xF3, 0x8B, 0xA8);
pub const GREEN: Color = Color::Rgb(0xA6, 0xE3, 0xA1);
pub const YELLOW: Color = Color::Rgb(0xF9, 0xE2, 0xAF);
pub const BLUE: Color = Color::Rgb(0x89, 0xB4, 0xFA);
pub const PEACH: Color = Color::Rgb(0xFA, 0xB3, 0x87);
pub const TEAL: Color = Color::Rgb(0x94, 0xE2, 0xD5);

/// Text in a colour.
#[must_use]
pub fn fg(color: Color) -> Style {
    Style::new().fg(color)
}

/// Bold text in a colour.
#[must_use]
pub fn bold(color: Color) -> Style {
    fg(color).add_modifier(Modifier::BOLD)
}

/// A section label (`CHECKS`, `REVIEW`): dim.
#[must_use]
pub fn label() -> Style {
    fg(DIM)
}

/// A pill's colour, as the mockup colours its states: what blocks red or peach, what waits
/// yellow or blue, what is done green or dim. Finer than [`Tone`] (conflicts and CI failing
/// are both `error`), so the table's marks tell them apart.
#[must_use]
pub const fn pill_color(kind: PillKind) -> Color {
    match kind {
        PillKind::CiFailed | PillKind::CiFailing => RED,
        PillKind::Conflicts => PEACH,
        PillKind::Changes | PillKind::Comments => ACCENT,
        PillKind::CiRunning | PillKind::Behind | PillKind::Blocked => YELLOW,
        PillKind::InReview => BLUE,
        PillKind::Ready => GREEN,
        PillKind::AutoMerging => TEAL,
        PillKind::Checking
        | PillKind::Draft
        | PillKind::Loading
        | PillKind::Merged
        | PillKind::Closed => DIM,
    }
}

/// A tone's colour, for a headline or a verdict.
#[must_use]
pub const fn tone_color(tone: Tone) -> Color {
    match tone {
        Tone::Success => GREEN,
        Tone::Error => RED,
        Tone::Warning => YELLOW,
        Tone::Merged => ACCENT,
        Tone::Subtle => DIM,
        Tone::Suggestion => BLUE,
    }
}

/// An agent's glyph and colour: Claude ◆ peach, Codex ◇ teal, any other ◇ blue.
#[must_use]
pub fn agent_look(agent: Option<&str>) -> (&'static str, Color) {
    match agent {
        Some("claude") => ("◆", PEACH),
        Some("codex") => ("◇", TEAL),
        _ => ("◇", BLUE),
    }
}
