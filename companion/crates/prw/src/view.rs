//! Drawing the model: one function per pane, into the areas `prw_core::panes` gives. Nothing
//! here decides anything the model hasn't; it lays out words the core worked out, and notes
//! where it put what a click can land on ([`Hits`]).

use prw_core::act::{Acts, Ask, Copied, Look, Toast, Tone, prompt_body};
use prw_core::gh::{GhPr, PrState};
use prw_core::layout::{Layout, layout};
use prw_core::merge::{Standing, merge_question, next_method, standing};
use prw_core::panes::{Pane, panes};
use prw_core::review::{Verdict, reviews_of, threads_beside, verdict_line};
use prw_core::status::{
    CheckVerdict, Checks, Mergeable, PillKind, STEPS, Stage, activity, is_status_context,
    latest_runs, mergeable_of, meta_of, step_index, verdict_of,
};
use prw_core::time::parse_iso;
use prw_core::ui::{
    Focus, Hit, Key, LIT_MS, List, ListId, Lit, Model, Poll, Row, agents_live, ago,
};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::{
    ACCENT, BLUE, BODY, DIM, FG, GREEN, INK, LINE, PANEL, PEACH, RED, SEL, TEAL, YELLOW,
    agent_look, bold, fg, label, pill_color, tone_color,
};

/// What the view needs beyond the model: where things are, to name them.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    /// The cache directory, as the screen names it (`~` for the home directory).
    pub dir: &'a str,
    /// The home directory, which a session's `cwd` shows as `~`; "" for none.
    pub home: &'a str,
    /// The animation's frame, one every [`BEAT`]: what turns the spinners and runs the shine.
    pub beat: u64,
    /// The model's clock at this frame (ms): what fades the lit rows and the toast.
    pub now: i64,
}

/// How long each frame of the animation shows, as the mod's (`FRAME_MS`).
pub const BEAT: std::time::Duration = std::time::Duration::from_millis(90);

/// How long a toast takes to fade, at the end of its time (ms).
const FADE_MS: i64 = 600;

/// How often the loop looks at the files when nothing else wakes it (ms): how far ahead a fade
/// starts drawing, since that tick would otherwise be its next frame.
pub const TICK_MS: u16 = 1_000;

/// How far ahead a fade starts drawing (ms): one [`TICK_MS`].
const LEAD_MS: i64 = TICK_MS as i64;

/// How long a new row takes to wipe in, left to right (ms).
const WIPE_MS: usize = 450;

/// The spinner's frames, the mod's (`BRAILLE`).
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Whether a row's PR moves on screen: something under way, and not a read that failed.
fn is_spinning(row: &Row) -> bool {
    row.pr.derived.is_moving && !(row.pr.error.is_some() && row.pr.state.is_none())
}

/// Whether anything on screen moves at `now` (a PR the list shows or lights, a job's spinner
/// on the status line, a toast about to fade), so the loop draws a frame every [`BEAT`].
/// Nothing does under the prompt pane or on a screen too narrow to draw.
pub fn is_animated(model: &Model, now: i64) -> bool {
    let acts = model.acts();
    if acts.view().is_some() || model.layout() == Layout::TooNarrow {
        return false;
    }
    let toast = toast_at(acts, now);
    let is_busy_shown = acts.ask().is_none() && toast.is_none() && acts.busy().next().is_some();
    let is_fading =
        acts.ask().is_none() && toast.is_some_and(|t| t.until - now <= FADE_MS + LEAD_MS);
    let is_lit = |row: &Row| glow(model.lit(&row.pr.url), now) > 0.0;
    is_busy_shown
        || is_fading
        || model
            .list()
            .rows
            .iter()
            .any(|r| is_spinning(r) || is_lit(r))
}

/// The toast, while it still shows at `now`.
fn toast_at(acts: &Acts, now: i64) -> Option<&Toast> {
    acts.toast().filter(|t| t.until > now)
}

/// How much of a light is left at `now`: 1 as it lights, easing out to 0 at [`LIT_MS`].
fn glow(lit: Option<Lit>, now: i64) -> f64 {
    #[expect(clippy::cast_precision_loss, reason = "ms within LIT_MS")]
    // A light from after this frame (the clock stepped back) isn't drawn.
    let left = lit
        .filter(|l| l.at <= now)
        .map_or(0.0, |l| 1.0 - (now - l.at) as f64 / LIT_MS as f64);
    left.clamp(0.0, 1.0).powi(2)
}

/// The first `n` cells of `spans`.
fn first_cells(spans: Vec<Span<'static>>, n: usize) -> Vec<Span<'static>> {
    let mut left = n;
    spans
        .into_iter()
        .map(|s| {
            let kept: String = s
                .content
                .chars()
                .take_while(|ch| {
                    let w = ch.width().unwrap_or(0);
                    let fits = w <= left;
                    left = if fits { left - w } else { 0 };
                    fits
                })
                .collect();
            Span::styled(kept, s.style)
        })
        .collect()
}

/// `from` moved `share` (0..=1) of the way to `to`, where both are RGB.
fn mix(from: Color, to: Color, share: f64) -> Color {
    let (Color::Rgb(r, g, b), Color::Rgb(r2, g2, b2)) = (from, to) else {
        return from;
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "between two bytes, so a byte"
    )]
    let step = |one: u8, other: u8| {
        (f64::from(one) + (f64::from(other) - f64::from(one)) * share).round() as u8
    };
    Color::Rgb(step(r, r2), step(g, g2), step(b, b2))
}

/// The spinner at `beat`, its PR's `seed` (its number) setting it apart from the others' (the
/// mod's `spinnerCell`): the frames in turn, `colour` pulsing toward white.
fn spinner(beat: u64, seed: u64, colour: Color) -> Span<'static> {
    let at = beat.wrapping_add(seed);
    let glyph = SPIN[usize::try_from(at % 10).unwrap_or(0)];
    #[expect(
        clippy::cast_precision_loss,
        reason = "a phase: precision past 2^52 frames doesn't matter"
    )]
    let pulse = f64::midpoint((at as f64 / 3.0).sin(), 1.0);
    span(glyph, bold(mix(colour, FG, pulse * 0.6)))
}

/// Where the screen drew what a click can land on. A later zone lies over an earlier one (a
/// list over its pane, the help over everything). While a question is open, only its answers
/// take clicks.
#[derive(Debug, Default)]
pub struct Hits {
    zones: Vec<(Rect, Hit)>,
    is_asking: bool,
}

impl Hits {
    /// What a click on this cell lands on.
    pub fn at(&self, x: u16, y: u16) -> Option<Hit> {
        self.zones
            .iter()
            .rev()
            .find(|(r, _)| r.contains(Position::new(x, y)))
            .map(|&(_, hit)| hit)
    }

    /// The pane under this cell, for the wheel.
    pub fn pane_at(&self, x: u16, y: u16) -> Option<Focus> {
        self.zones.iter().rev().find_map(|&(r, hit)| match hit {
            Hit::Pane(pane) if r.contains(Position::new(x, y)) => Some(pane),
            _ => None,
        })
    }

    fn add(&mut self, at: Rect, hit: Hit) {
        if !self.is_asking && !at.is_empty() {
            self.zones.push((at, hit));
        }
    }

    /// The spots of a line drawn across `line` (one row): each at its cells, cut to the row.
    fn spots(&mut self, line: Rect, spots: Spots) {
        for (x, width, hit) in spots {
            let at = Rect::new(line.x.saturating_add(cut(x)), line.y, cut(width), 1);
            self.add(at.intersection(line), hit);
        }
    }

    /// The open question's answers, which alone take clicks while it is open.
    fn answers(&mut self, line: Rect, spots: Spots) {
        self.is_asking = false;
        self.spots(line, spots);
        self.is_asking = true;
    }
}

/// Where on a line its clickable spans lie: their first cell, how many, and what they hit.
type Spots = Vec<(usize, usize, Hit)>;

/// A line's spans, and the spots among them a click can land on.
#[derive(Default)]
struct Marked {
    spans: Vec<Span<'static>>,
    spots: Spots,
}

impl Marked {
    fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }

    fn push(&mut self, span: Span<'static>) {
        self.spans.push(span);
    }

    /// `spans`, a click on any of them landing on `hit`.
    fn hit(&mut self, spans: impl IntoIterator<Item = Span<'static>>, hit: Hit) {
        let x = self.width();
        self.spans.extend(spans);
        self.spots.push((x, self.width() - x, hit));
    }

    fn done(self) -> (Line<'static>, Spots) {
        (Line::from(self.spans), self.spots)
    }
}

/// A cell count as the screen takes it (no screen is 65536 cells wide).
fn cut(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// The row `i` of `r` (counted from its top), where `r` has it.
fn row_of(r: Rect, i: usize) -> Option<Rect> {
    (i < usize_of(r.height)).then(|| Rect::new(r.x, r.y + cut(i), r.width, 1))
}

/// An offer's key, as the model takes it.
fn offer_key(key: &str) -> Option<Hit> {
    key.chars().next().map(|c| Hit::Key(Key::Char(c)))
}

/// Draws the whole screen, and says where what a click can land on went.
pub fn render(frame: &mut Frame, model: &Model, ctx: Ctx) -> Hits {
    let mut hits = Hits {
        zones: Vec::new(),
        is_asking: model.acts().ask().is_some(),
    };
    let area = frame.area();
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(INK).fg(BODY));
    let chosen = layout(area.width);
    for (pane, at) in panes(chosen, area.width, area.height) {
        let r = Rect::new(area.x + at.x, area.y + at.y, at.width, at.height);
        if r.is_empty() {
            continue;
        }
        let h = &mut hits;
        match pane {
            Pane::Header => header(frame, r, model, chosen),
            Pane::Lists => lists_pane(frame, r, model, h),
            Pane::Strip => strip(frame, r, model, chosen, h),
            Pane::Table => table(frame, r, model, chosen, ctx, h),
            Pane::Detail if chosen == Layout::Side => card(frame, r, model, ctx, h),
            Pane::Detail => detail(frame, r, model, ctx, h),
            Pane::Status => status(frame, r, model, ctx, h),
            Pane::Hints => hints(frame, r, chosen, h),
            Pane::Narrow => narrow(frame, r),
        }
    }
    if chosen == Layout::TooNarrow {
        return hits;
    }
    if let (Some(top), Some(copied)) = (model.acts().view(), model.acts().copied()) {
        // The mouse is the terminal's while it shows, to select the prompt with.
        prompt_pane(frame, area, top, copied);
        return Hits::default();
    }
    if model.is_help() {
        help(frame, area, chosen);
        // A click anywhere closes it, and the wheel moves nothing under it.
        hits.zones.clear();
        hits.add(area, Hit::Key(Key::Char('?')));
    }
    hits
}

// ---- Text -------------------------------------------------------------------------------

fn cells(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// `text` cut to `n` cells, an ellipsis standing for what's cut.
fn clip(text: &str, n: usize) -> String {
    if cells(text) <= n {
        return text.to_owned();
    }
    if n == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w > n - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// `text` in exactly `n` cells: cut, or padded on the right.
fn fit(text: &str, n: usize) -> String {
    let mut out = clip(text, n);
    let pad = n.saturating_sub(cells(&out));
    out.extend(std::iter::repeat_n(' ', pad));
    out
}

/// `text` in exactly `n` cells, padded on the left.
fn right(text: &str, n: usize) -> String {
    let out = clip(text, n);
    format!("{}{out}", " ".repeat(n.saturating_sub(cells(&out))))
}

/// `text` in lines of at most `n` cells, at most `max` of them (the last cut).
fn wrap(text: &str, n: usize, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if line.is_empty() {
            word.clone_into(&mut line);
        } else if cells(&line) + 1 + cells(word) <= n {
            line.push(' ');
            line.push_str(word);
        } else if let Some(head) = line.strip_suffix(" ·") {
            // A " · " at the break starts the next line, rather than hang at the end.
            lines.push(head.to_owned());
            line = format!("· {word}");
        } else {
            lines.push(std::mem::take(&mut line));
            word.clone_into(&mut line);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > max {
        let rest = lines[max - 1..].join(" ");
        lines.truncate(max - 1);
        lines.push(rest);
    }
    lines.iter().map(|l| clip(l, n)).collect()
}

fn usize_of(n: u16) -> usize {
    usize::from(n)
}

fn span(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

/// A line of `spans` padded to `n` cells in `bg`, so a selection's colour runs to the edge.
fn filled(mut spans: Vec<Span<'static>>, n: usize, bg: Color) -> Line<'static> {
    let used: usize = spans.iter().map(Span::width).sum();
    spans.push(Span::raw(" ".repeat(n.saturating_sub(used))));
    Line::from(spans).style(Style::new().bg(bg))
}

fn boxed(title: &str, is_hot: bool) -> Block<'static> {
    let (edge, title_style) = if is_hot {
        (fg(ACCENT), bold(ACCENT))
    } else {
        (fg(LINE), bold(BODY))
    };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(edge)
        .title(Line::from(span(format!(" {title} "), title_style)))
}

/// The table's mark for a PR: its pill's glyph, one cell wide (💬 is two, so comments get ●).
fn mark(kind: PillKind) -> String {
    match kind {
        PillKind::Comments => "●".to_owned(),
        _ => kind.text().chars().next().unwrap_or('●').to_string(),
    }
}

/// A pill: its glyph and words on its colour.
fn pill(kind: PillKind) -> Span<'static> {
    span(format!(" {} ", kind.text()), bold(INK).bg(pill_color(kind)))
}

/// "4.6k", "812".
fn short_count(n: i64) -> String {
    if n >= 1000 {
        let tenths = n / 100;
        format!("{}.{}k", tenths / 10, tenths % 10)
    } else {
        n.to_string()
    }
}

const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];

/// A meter `n` cells wide, `frac` of it full (eighths of a cell).
fn meter(n: usize, frac: f64, on: Color) -> Vec<Span<'static>> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "a meter a few cells wide, its fraction clamped to 0..=1"
    )]
    let eighths = (frac.clamp(0.0, 1.0) * (n * 8) as f64).round() as usize;
    let full = eighths / 8;
    let part = PARTS[eighths % 8];
    let mut out = vec![span("█".repeat(full), fg(on))];
    if full < n {
        let (first, style) = if part.is_empty() {
            ("░", fg(LINE))
        } else {
            (part, fg(on))
        };
        out.push(span(first, style));
        out.push(span("░".repeat(n - full - 1), fg(LINE)));
    }
    out
}

/// The checks bar (look.ts's `allot` and `barCells`): passed green, failed red, pending
/// yellow, each a share of `width` (at least one cell where there is any). A shine runs along
/// the pending, a cell a beat.
fn bar(checks: &Checks, width: usize, beat: u64) -> Vec<Span<'static>> {
    let [passed, failed, pending] =
        allot(&[checks.passed, checks.failed, checks.pending], width)[..]
    else {
        return Vec::new();
    };
    let mut out: Vec<Span<'static>> = [(passed, GREEN), (failed, RED)]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, colour)| span("━".repeat(n), fg(colour)))
        .collect();
    out.extend(shine(pending, beat).map(|colour| span("━", fg(colour))));
    out
}

/// The pending run's `n` cells at `beat`: yellow, but for a glint passing through (the mod's).
fn shine(n: usize, beat: u64) -> impl Iterator<Item = Color> {
    let lap = n as u64 + 8;
    #[expect(clippy::cast_precision_loss, reason = "a bar a few dozen cells wide")]
    let at = (beat % lap) as f64 - 4.0;
    (0..n).map(move |i| {
        #[expect(clippy::cast_precision_loss, reason = "a cell of a bar")]
        let near = (1.0 - (i as f64 - at).abs() / 3.0).max(0.0);
        mix(YELLOW, FG, near * 0.85)
    })
}

/// `width` cells shared out by `counts` (look.ts's `allot`): each count that isn't 0 gets one
/// cell at least, the rest by the largest remainders.
fn allot(counts: &[usize], width: usize) -> Vec<usize> {
    let total: usize = counts.iter().sum();
    if total == 0 {
        return vec![0; counts.len()];
    }
    let mut out: Vec<usize> = counts
        .iter()
        .map(|&n| if n > 0 { (n * width / total).max(1) } else { 0 })
        .collect();
    let mut by_rest: Vec<usize> = (0..counts.len()).collect();
    by_rest.sort_by_key(|&i| std::cmp::Reverse(counts[i] * width % total));
    let mut used: usize = out.iter().sum();
    for &i in by_rest.iter().cycle().take(counts.len() * 4) {
        if used >= width {
            break;
        }
        if counts[i] > 0 {
            out[i] += 1;
            used += 1;
        }
    }
    while used > width {
        // The first of the largest, as `indexOf(Math.max(...))` finds it.
        let Some(i) = (0..out.len()).min_by_key(|&i| std::cmp::Reverse(out[i])) else {
            break;
        };
        out[i] -= 1;
        used -= 1;
    }
    out
}

// ---- Header, status, hints --------------------------------------------------------------

fn poll_color(poll: Poll) -> Color {
    match poll {
        Poll::Mod | Poll::Prw => GREEN,
        Poll::Nobody => YELLOW,
        Poll::Stale => PEACH,
        Poll::Waiting => DIM,
    }
}

fn header(frame: &mut Frame, r: Rect, model: &Model, chosen: Layout) {
    let said = model.header();
    let bg = |style: Style| style.bg(PANEL);
    let mut left = vec![span(" ◆ pr-watch", bg(bold(ACCENT)))];
    if chosen == Layout::Three {
        left.push(span(" companion", bg(fg(BODY))));
    }
    let mut words = vec![said.poll.words().to_owned()];
    if let Some(age) = said.age {
        words.push(ago(age));
    }
    words.push(format!("{} live", said.live));
    let mut right_side = vec![
        span("● ", bg(fg(poll_color(said.poll)))),
        span(words.join(" · "), bg(fg(BODY))),
    ];
    if said.is_read_failing {
        right_side.push(span(" · ⚠ read failed", bg(fg(RED))));
    }
    right_side.push(span(" ", bg(fg(BODY))));
    let width = usize_of(r.width);
    let right_cells: usize = right_side.iter().map(Span::width).sum();
    let mut used: usize = left.iter().map(Span::width).sum::<usize>() + 2;
    let tally = said.tally;
    let plural = |n: usize, one: &str, many: &str| if n == 1 { one } else { many }.to_owned();
    let counts = [
        (tally.failing, "✗", "failing".to_owned(), RED),
        (
            tally.conflicts,
            "⚠",
            plural(tally.conflicts, "conflict", "conflicts"),
            PEACH,
        ),
        (tally.changes, "↺", "changes".to_owned(), ACCENT),
        (tally.running, "●", "running".to_owned(), YELLOW),
        (tally.ready, "✓", "ready".to_owned(), GREEN),
        (tally.review, "◷", "in review".to_owned(), BLUE),
    ];
    for (n, glyph, word, colour) in counts {
        if n == 0 {
            continue;
        }
        let text = if chosen == Layout::Side {
            n.to_string()
        } else {
            format!("{n} {word}")
        };
        let need = 2 + cells(&text) + 2;
        if used + need + right_cells > width {
            break;
        }
        left.push(span(format!("  {glyph} "), bg(fg(colour))));
        left.push(span(text, bg(fg(FG))));
        used += need;
    }
    let left_cells: usize = left.iter().map(Span::width).sum();
    left.push(span(
        " ".repeat(width.saturating_sub(left_cells + right_cells)),
        bg(fg(BODY)),
    ));
    left.extend(right_side);
    frame.render_widget(Paragraph::new(Line::from(left)).style(bg(fg(BODY))), r);
}

fn open_count(model: &Model) -> usize {
    model.lists()[0]
        .rows
        .iter()
        .filter(|row| row.pr.state == Some(PrState::Open))
        .count()
}

fn status(frame: &mut Frame, r: Rect, model: &Model, ctx: Ctx, hits: &mut Hits) {
    let width = usize_of(r.width).saturating_sub(2);
    if let Some(ask) = model.acts().ask() {
        let (line, spots) = ask_line(ask, width);
        hits.answers(r, spots);
        frame.render_widget(Paragraph::new(line), r);
        return;
    }
    let said = model.header();
    let (text, colour) = if let Some(toast) = toast_at(model.acts(), ctx.now) {
        // Its last moments fade it into the ground.
        #[expect(clippy::cast_precision_loss, reason = "ms within FADE_MS")]
        let left = ((toast.until - ctx.now) as f64 / FADE_MS as f64).min(1.0);
        (toast.text.clone(), mix(INK, tone_of(toast.tone), left))
    } else if let Some(busy) = model.acts().busy().next() {
        // The spinner, then the words.
        let words = clip(&format!("{busy}…"), width.saturating_sub(2));
        let line = Line::from(vec![
            Span::raw(" "),
            spinner(ctx.beat, 0, YELLOW),
            span(format!(" {words}"), fg(YELLOW)),
        ]);
        frame.render_widget(Paragraph::new(line), r);
        return;
    } else if model.is_waiting() {
        (format!("Reading {}", ctx.dir), DIM)
    } else if said.is_read_failing {
        let when = model
            .reads()
            .1
            .map(|at| ago(model.now().saturating_sub(at)))
            .unwrap_or_default();
        (
            format!("⚠ results.json didn't read ({when}): showing the last good copy"),
            YELLOW,
        )
    } else if let Some(error) = model.poller().and_then(|p| p.error.as_deref()) {
        // prw's own poller can't find a login's PRs: after a cache that won't read, the
        // thing to fix.
        (format!("⚠ {error}"), YELLOW)
    } else if let Some(problem) = model.poller().and_then(|p| p.problems.first()) {
        (format!("⚠ {problem}"), YELLOW)
    } else if let Some(error) = model.list().search.as_ref().and_then(|s| s.error.as_ref()) {
        (format!("⚠ {}: {error}", model.list().label), YELLOW)
    } else if said.live == 0 {
        (
            "No live sessions: showing every PR in results.json".to_owned(),
            DIM,
        )
    } else {
        (
            format!(
                "Watching {} · {} live · {}",
                counted(open_count(model), "open PR"),
                counted(said.live, "session"),
                counted(model.lists().len(), "list")
            ),
            DIM,
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            span(clip(&text, width), fg(colour)),
        ])),
        r,
    );
}

/// `n` and the noun, plural but for one.
fn counted(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn hints(frame: &mut Frame, r: Rect, chosen: Layout, hits: &mut Hits) {
    // Most needed first: a narrow screen drops the last.
    let h_l = if chosen == Layout::Three {
        "pane"
    } else {
        "list"
    };
    let char = |c| Key::Char(c);
    let keys = [
        ("j/k", "move", char('j')),
        ("tab", "list", Key::Tab),
        ("m", "merge", char('m')),
        ("c", "comment", char('c')),
        ("f", "hand off", char('f')),
        ("o", "browser", char('o')),
        ("?", "help", char('?')),
        ("q", "quit", char('q')),
        ("a", "account", char('a')),
        ("h/l", h_l, char('l')),
        ("g/G", "top/end", char('g')),
    ];
    let width = usize_of(r.width);
    let mut line = Marked::default();
    line.push(span(" ", Style::new()));
    for (key, what, press) in keys {
        if line.width() + cells(key) + 1 + cells(what) + 2 > width {
            break;
        }
        line.hit(
            [span(key, fg(ACCENT)), span(format!(" {what}"), fg(DIM))],
            Hit::Key(press),
        );
        line.push(span("  ", fg(DIM)));
    }
    hits.spots(r, line.spots);
    frame.render_widget(
        Paragraph::new(filled(line.spans, width, PANEL)).style(Style::new().bg(PANEL)),
        r,
    );
}

fn narrow(frame: &mut Frame, r: Rect) {
    let width = usize_of(r.width);
    let lines = vec![
        Line::from(span(clip("prw needs 40 columns", width), bold(FG))).centered(),
        Line::from(span(
            clip(&format!("this one has {}", r.width), width),
            fg(DIM),
        ))
        .centered(),
    ];
    let top = r.height.saturating_sub(2) / 2;
    let at = Rect::new(r.x, r.y + top, r.width, r.height - top);
    frame.render_widget(Paragraph::new(lines), at);
}

fn help(frame: &mut Frame, area: Rect, chosen: Layout) {
    let h_l = if chosen == Layout::Three {
        "lists pane, PRs pane"
    } else {
        "previous, next list"
    };
    let keys = [
        ("j k  ↑ ↓", "move"),
        ("g G", "first, last"),
        ("tab ⇧tab", "next, previous list"),
        ("h l  ← →", h_l),
        ("m", "merge, after a y (m again: method)"),
        ("c", "comment: ⏎ posts, alt-⏎ a new line"),
        ("f", "fix, resolve or address: copy the prompt"),
        ("o", "open in the browser"),
        ("a", "switch the gh login its owner goes as"),
        ("v", "show the prompt last copied"),
        ("?", "this help"),
        ("q  esc", "quit"),
    ];
    let width = 56.min(area.width.saturating_sub(2));
    let height = 18.min(area.height.saturating_sub(2));
    let at = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let iw = usize_of(width).saturating_sub(4);
    let mut lines = vec![Line::raw("")];
    for (key, what) in keys {
        lines.push(Line::from(vec![
            span(fit(key, 10), fg(ACCENT)),
            span(clip(what, iw.saturating_sub(10)), fg(BODY)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(span(
        clip("Actions go as the gh login that sees the repo.", iw),
        fg(DIM),
    )));
    lines.push(Line::from(span(
        clip("A click picks a list or a PR, or presses its key.", iw),
        fg(DIM),
    )));
    frame.render_widget(Clear, at);
    frame.render_widget(
        Paragraph::new(lines)
            .block(boxed("Keys", true).padding(Padding::horizontal(1)))
            .style(Style::new().bg(PANEL)),
        at,
    );
}

// ---- Lists ------------------------------------------------------------------------------

fn list_glyph(list: &List) -> Option<(&'static str, Color)> {
    match list.id {
        ListId::All => None,
        ListId::Session(_) | ListId::Unnamed => Some(agent_look(list.agent.as_deref())),
        ListId::Search(_) => Some(("◎", ACCENT)),
    }
}

/// Whether a list goes under SESSIONS: an agent's session, not `prw`'s own lists (All, My
/// PRs, the saved searches).
fn is_sessions_list(model: &Model, list: &List) -> bool {
    let own = model.poller().map(|p| p.own.as_str());
    match &list.id {
        ListId::All | ListId::Search(_) => false,
        ListId::Session(id) => own != Some(id.as_str()),
        ListId::Unnamed => true,
    }
}

/// One list in the lists pane: ▌, the name, the mark between spaces, the count ("Claude
/// session" fits in 22).
fn list_line(list: &List, is_on: bool, w: usize) -> Line<'static> {
    let bg = if is_on { SEL } else { INK };
    let on = |style: Style| style.bg(bg);
    let mut spans = vec![span(if is_on { "▌" } else { " " }, on(fg(ACCENT)))];
    let count = list.rows.len().to_string();
    let count_w = cells(&count).max(2);
    let name_w = w.saturating_sub(1 + 3 + count_w);
    let name = match list_glyph(list) {
        Some((glyph, colour)) => {
            spans.push(span(format!("{glyph} "), on(fg(colour))));
            fit(&list.label, name_w.saturating_sub(2))
        }
        None => fit(&list.label, name_w),
    };
    spans.push(span(name, on(if is_on { bold(FG) } else { fg(BODY) })));
    let (glyph, colour) = list
        .worst()
        .map_or((" ".to_owned(), DIM), |k| (mark(k), pill_color(k)));
    spans.push(span(format!(" {glyph} "), on(fg(colour))));
    spans.push(span(right(&count, count_w), on(fg(DIM))));
    filled(spans, w, bg)
}

fn lists_pane(frame: &mut Frame, r: Rect, model: &Model, hits: &mut Hits) {
    let block = boxed("Lists", model.focus() == Focus::Lists);
    let inner = block.inner(r);
    hits.add(r, Hit::Pane(Focus::Lists));
    frame.render_widget(block, r);
    let w = usize_of(inner.width);
    let ix = 1;
    let pad = || Span::raw(" ".repeat(ix));
    let section = |name: &str| Line::from(vec![pad(), span(name.to_owned(), label())]);
    let mut lines = vec![Line::raw(""), section("LISTS")];
    let first_session = model
        .lists()
        .iter()
        .position(|l| is_sessions_list(model, l));
    for (i, list) in model.lists().iter().enumerate() {
        if Some(i) == first_session {
            lines.push(Line::raw(""));
            lines.push(section("SESSIONS"));
        }
        if let Some(at) = row_of(inner, lines.len()) {
            hits.add(at, Hit::List(i));
        }
        lines.push(list_line(list, i == model.list_index(), w));
    }
    let agents = agents_live(model);
    if !agents.is_empty() {
        lines.push(Line::raw(""));
        lines.push(section("AGENTS"));
        for (agent, n) in agents {
            let (glyph, colour) = agent_look(Some(&agent));
            lines.push(Line::from(vec![
                pad(),
                span(format!("{glyph} "), fg(colour)),
                span(fit(&agent, 9), fg(BODY)),
                span(format!("{n} live"), fg(DIM)),
            ]));
        }
    }
    if let Some(shared) = model.shared() {
        let rates: Vec<_> = shared
            .rates
            .iter()
            .filter_map(|(login, r)| r.graphql.map(|b| (login, b)))
            .collect();
        if !rates.is_empty() {
            lines.push(Line::raw(""));
            lines.push(section("GITHUB"));
            for (login, bucket) in rates {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a rate limit, far inside f64's exact range"
                )]
                let frac = if bucket.limit > 0 {
                    bucket.remaining as f64 / bucket.limit as f64
                } else {
                    0.0
                };
                let mut spans = vec![pad(), span(fit(login, 8), fg(BODY)), Span::raw(" ")];
                spans.extend(meter(6, frac, TEAL));
                spans.push(span(right(&short_count(bucket.remaining), 5), fg(DIM)));
                lines.push(Line::from(spans));
            }
        }
        lines.push(Line::raw(""));
        let next = shared.next_at.saturating_sub(model.now());
        let said = match model.header().poll {
            Poll::Mod | Poll::Prw if next > 0 => {
                format!("next round in {}s", next.saturating_add(999) / 1000)
            }
            Poll::Mod | Poll::Prw => "a round is due".to_owned(),
            poll => poll.words().to_owned(),
        };
        lines.push(Line::from(vec![pad(), span(clip(&said, w - ix), fg(DIM))]));
        match model.header().poll {
            Poll::Mod => lines.push(Line::from(vec![
                pad(),
                span("shared with the mod", fg(DIM)),
            ])),
            Poll::Prw => lines.push(Line::from(vec![pad(), span("polled by prw", fg(DIM))])),
            _ => {}
        }
        if let Some(error) = model.poller().and_then(|p| p.error.as_deref()) {
            lines.push(Line::from(vec![
                pad(),
                span(clip(&format!("⚠ {error}"), w - ix), fg(YELLOW)),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn strip(frame: &mut Frame, r: Rect, model: &Model, chosen: Layout, hits: &mut Hits) {
    let width = usize_of(r.width);
    let lists = model.lists();
    let at = model.list_index();
    if chosen == Layout::Side {
        let list = model.list();
        let mut spans = vec![span(" ◂ ", fg(ACCENT))];
        if let Some((glyph, colour)) = list_glyph(list) {
            spans.push(span(format!("{glyph} "), fg(colour)));
        }
        let count = format!(" · {}", list.rows.len());
        let of = format!("{}/{} lists ", at + 1, lists.len());
        let room = width.saturating_sub(3 + 2 + cells(&count) + 3 + cells(&of) + 1);
        spans.push(span(clip(&list.label, room), bold(FG)));
        spans.push(span(count, fg(DIM)));
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(span(" ▸", fg(ACCENT)));
        hits.spots(
            r,
            vec![
                (0, 3, Hit::Key(Key::BackTab)),
                (used, 2, Hit::Key(Key::Tab)),
            ],
        );
        let used = used + 2;
        spans.push(Span::raw(
            " ".repeat(width.saturating_sub(used + cells(&of))),
        ));
        spans.push(span(of, fg(DIM)));
        frame.render_widget(Paragraph::new(Line::from(spans)), r);
        return;
    }
    let label_of = |list: &List| {
        let glyph = list_glyph(list).map_or(String::new(), |(g, _)| format!("{g} "));
        format!(" {glyph}{} {} ", list.label, list.rows.len())
    };
    // Start where the selected list still shows.
    let mut first = 0;
    while first < at
        && lists[first..=at]
            .iter()
            .map(|l| cells(&label_of(l)) + 1)
            .sum::<usize>()
            + 4
            > width
    {
        first += 1;
    }
    let mut line = Marked::default();
    if first > 0 {
        line.hit([span("‹", fg(DIM))], Hit::Key(Key::BackTab));
    } else {
        line.push(span(" ", fg(DIM)));
    }
    let mut shown = first;
    for (i, list) in lists.iter().enumerate().skip(first) {
        let text = label_of(list);
        let used = line.width();
        if used + cells(&text) + 1 + 4 > width && i != at {
            break;
        }
        let style = if i == at { bold(FG).bg(SEL) } else { fg(BODY) };
        line.hit(
            [span(clip(&text, width.saturating_sub(used + 1)), style)],
            Hit::List(i),
        );
        line.push(Span::raw(" "));
        shown = i + 1;
    }
    if shown < lists.len() {
        line.hit(
            [span(format!("+{} ›", lists.len() - shown), fg(DIM))],
            Hit::Key(Key::Tab),
        );
    }
    let (line, spots) = line.done();
    hits.spots(r, spots);
    frame.render_widget(Paragraph::new(line), r);
}

// ---- The table --------------------------------------------------------------------------

/// What the table's status column says of a row, in a few words.
fn table_state(row: &Row) -> String {
    let d = &row.pr.derived;
    match d.pill {
        PillKind::CiFailed | PillKind::CiFailing => format!("{} failed", d.checks.failed),
        PillKind::Conflicts => "conflicts".to_owned(),
        PillKind::Changes => "changes".to_owned(),
        PillKind::Comments => "comments".to_owned(),
        PillKind::InReview => "in review".to_owned(),
        PillKind::Ready => "ready".to_owned(),
        PillKind::Behind => "behind".to_owned(),
        PillKind::Blocked => "blocked".to_owned(),
        PillKind::Checking => "checking".to_owned(),
        PillKind::AutoMerging => "auto-merge".to_owned(),
        PillKind::Draft => "draft".to_owned(),
        PillKind::Loading if row.pr.error.is_some() => "can't read".to_owned(),
        PillKind::Loading => "loading…".to_owned(),
        PillKind::Merged => "merged".to_owned(),
        PillKind::Closed => "closed".to_owned(),
        PillKind::CiRunning => format!("{}/{}", d.checks.passed, d.checks.total),
    }
}

const STATUS_W: usize = 11;

fn status_cell(row: &Row, bg: Color, beat: u64) -> Vec<Span<'static>> {
    let d = &row.pr.derived;
    let colour = if row.pr.error.is_some() && row.pr.state.is_none() {
        RED
    } else {
        pill_color(d.pill)
    };
    if d.pill == PillKind::CiRunning && d.checks.total > 0 {
        let count = format!(" {}/{}", d.checks.passed, d.checks.total);
        let mut spans: Vec<Span<'static>> = bar(
            &d.checks,
            STATUS_W.saturating_sub(cells(&count)).min(5),
            beat,
        )
        .into_iter()
        .map(|s| {
            let style = s.style.bg(bg);
            s.style(style)
        })
        .collect();
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(span(fit(&count, STATUS_W - used), fg(colour).bg(bg)));
        return spans;
    }
    vec![span(fit(&table_state(row), STATUS_W), fg(colour).bg(bg))]
}

fn about(model: &Model, list: &List, home_dir: &str) -> String {
    match list.id {
        ListId::All if model.header().live == 0 => "every PR in results.json".to_owned(),
        ListId::All => "every PR a live session watches".to_owned(),
        ListId::Session(_) | ListId::Unnamed => {
            let agent = list.agent.as_deref().unwrap_or("agent");
            match &list.cwd {
                Some(cwd) => format!("{agent} · {}", tilde(cwd, home_dir)),
                None => agent.to_owned(),
            }
        }
        ListId::Search(_) => list.search.as_ref().map_or_else(String::new, |search| {
            let shown = search.urls.len() as u64;
            if search.total > shown {
                format!("{} · {shown} of {}", search.query, search.total)
            } else {
                search.query.clone()
            }
        }),
    }
}

/// `path` with the home directory as `~`.
pub fn tilde(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if !home.is_empty() && (rest.is_empty() || rest.starts_with('/')) => {
            format!("~{rest}")
        }
        _ => path.to_owned(),
    }
}

/// What the table says where it has no rows: waiting for pr-watch, or nothing listed.
fn table_empty(model: &Model, list: &List, iw: usize, ctx: Ctx) -> Option<Vec<Line<'static>>> {
    if model.is_waiting() {
        let mut lines = vec![
            Line::raw(""),
            Line::from(span(clip(" Waiting for pr-watch", iw), bold(FG))),
            Line::raw(""),
        ];
        let say = "Start a Claude session with the pr-watch mod, or run prw on its own later.";
        for l in wrap(say, iw.saturating_sub(2), 4) {
            lines.push(Line::from(span(format!(" {l}"), fg(BODY))));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(span(
            clip(&format!(" Reading {}", ctx.dir), iw),
            fg(DIM),
        )));
        return Some(lines);
    }
    if let Some(search) = &list.search
        && let Some(error) = &search.error
    {
        // The finds before stay listed; the error goes on the status line then.
        if list.rows.is_empty() {
            let mut lines = vec![Line::raw("")];
            for l in wrap(&format!("⚠ {error}"), iw.saturating_sub(2), 4) {
                lines.push(Line::from(span(format!(" {l}"), fg(YELLOW))));
            }
            return Some(lines);
        }
    }
    if !list.rows.is_empty() {
        return None;
    }
    let say = match &list.search {
        Some(search) if !search.is_searched => " Searching GitHub…",
        Some(_) => " No open PR matches this search.",
        None if list.id == ListId::All && model.header().live == 0 => {
            " No PRs in results.json yet."
        }
        None => " Nothing here yet.",
    };
    Some(vec![
        Line::raw(""),
        Line::from(span(clip(say, iw), fg(DIM))),
    ])
}

/// The table's columns: the title takes what the others leave.
#[derive(Debug, Clone, Copy)]
struct Columns {
    /// The All list says whose each PR is (an agent glyph).
    agent: usize,
    title: usize,
    /// 0: no repo column.
    repo: usize,
    /// `#412 `: the longest number and a space, 6 at least.
    number: usize,
    width: usize,
}

impl Columns {
    /// `▌`, the mark, a space, the number, then the title, the repo and the status.
    fn new(iw: usize, list: &List, chosen: Layout) -> Self {
        // A session's PRs are all its agent's; All's and a search's are anyone's.
        let agent = if matches!(list.id, ListId::All | ListId::Search(_)) {
            2
        } else {
            0
        };
        let longest = list.rows.iter().map(|r| r.pr.number).max().unwrap_or(0);
        let number = (format!("#{longest}").len() + 1).max(6);
        let rest = iw.saturating_sub(3 + number + agent + STATUS_W + 1);
        let repo = if chosen == Layout::Three && rest >= 16 + 11 {
            11
        } else {
            0
        };
        Self {
            agent,
            title: rest - repo,
            repo,
            number,
            width: iw,
        }
    }

    fn heading(self) -> Line<'static> {
        Line::from(vec![
            span("   ", label()),
            span(fit("PR", self.number), label()),
            span(fit("TITLE", self.agent + self.title), label()),
            // The repo and status cells each start after a space, as the row's do.
            span(fit(" REPO", self.repo), label()),
            span(" STATUS", label()),
        ])
    }

    /// A PR's row at `beat`; lit, at `now` it glows (a new one in the accent, a changed one
    /// in its new pill's colour), and a new one wipes in.
    fn row(self, row: &Row, is_sel: bool, beat: u64, lit: Option<Lit>, now: i64) -> Line<'static> {
        let kind = row.pr.derived.pill;
        let tint = if lit.is_some_and(|l| l.is_new) {
            ACCENT
        } else {
            pill_color(kind)
        };
        let bg = mix(if is_sel { SEL } else { INK }, tint, 0.3 * glow(lit, now));
        let on = |style: Style| style.bg(bg);
        let glyph = if is_spinning(row) {
            let turning = spinner(beat, row.pr.number, pill_color(kind));
            turning.patch_style(on(Style::new()))
        } else {
            span(mark(kind), on(fg(pill_color(kind))))
        };
        let mut spans = vec![
            span(if is_sel { "▌" } else { " " }, on(fg(ACCENT))),
            glyph,
            span(" ", on(Style::new())),
            span(
                fit(&format!("#{}", row.pr.number), self.number),
                on(fg(DIM)),
            ),
        ];
        if self.agent > 0 {
            let (glyph, colour) = match &row.agent {
                Some(agent) => agent_look(Some(agent)),
                None => (" ", DIM),
            };
            spans.push(span(format!("{glyph} "), on(fg(colour))));
        }
        let is_done = matches!(row.pr.state, Some(PrState::Merged | PrState::Closed));
        let (title, style) = if row.pr.title.is_empty() {
            ("Loading…", fg(DIM))
        } else if is_done {
            (row.pr.title.as_str(), fg(DIM))
        } else {
            (row.pr.title.as_str(), fg(FG))
        };
        let style = if is_sel {
            style.add_modifier(Modifier::BOLD)
        } else {
            style
        };
        spans.push(span(fit(title, self.title), on(style)));
        if self.repo > 0 {
            spans.push(span(fit(&format!(" {}", row.repo), self.repo), on(fg(DIM))));
        }
        spans.push(span(" ", on(Style::new())));
        spans.extend(status_cell(row, bg, beat));
        let wiping = lit
            .filter(|l| l.is_new)
            .and_then(|l| usize::try_from(now - l.at).ok())
            .filter(|&ms| ms < WIPE_MS);
        let spans = match wiping {
            Some(ms) => first_cells(spans, self.width * ms / WIPE_MS),
            None => spans,
        };
        filled(spans, self.width, bg)
    }
}

fn table(frame: &mut Frame, r: Rect, model: &Model, chosen: Layout, ctx: Ctx, hits: &mut Hits) {
    hits.add(r, Hit::Pane(Focus::Prs));
    let list = model.list();
    let title = if r.width >= 56 {
        format!("{}  ·  {}", list.label, about(model, list, ctx.home))
    } else {
        list.label.clone()
    };
    let block = boxed(
        &clip(&title, usize_of(r.width).saturating_sub(6)),
        model.focus() == Focus::Prs,
    );
    let inner = block.inner(r);
    let iw = usize_of(inner.width);
    let ih = usize_of(inner.height);
    if let Some(lines) = table_empty(model, list, iw, ctx) {
        frame.render_widget(block, r);
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }
    let columns = Columns::new(iw, list, chosen);
    let mut lines = vec![columns.heading()];
    if ih > 8 {
        lines.push(Line::raw(""));
    }
    // Scrolled just enough to show the selected row.
    let visible = ih.saturating_sub(lines.len()).max(1);
    let sel = model.selected_index().unwrap_or(0);
    let offset = sel.saturating_sub(visible - 1);
    for (i, row) in list.rows.iter().enumerate().skip(offset).take(visible) {
        if let Some(at) = row_of(inner, lines.len()) {
            hits.add(at, Hit::Row(i));
        }
        let line = columns.row(row, i == sel, ctx.beat, model.lit(&row.pr.url), ctx.now);
        lines.push(line);
    }
    let total = list.rows.len();
    let block = if total > visible {
        let end = (offset + visible).min(total);
        let shown = format!(" {}–{end} of {total} ", offset + 1);
        block.title_bottom(Line::from(span(shown, fg(DIM))).right_aligned())
    } else {
        block
    };
    frame.render_widget(block, r);
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---- The selected PR --------------------------------------------------------------------

/// The four steps, the current one marked (the mod's stepper): done ✓, the current one its
/// pill's glyph (the spinner while moving), the rest ○.
fn stepper(row: &Row, width: usize, beat: u64) -> Line<'static> {
    let d = &row.pr.derived;
    let step = usize::from(step_index(d.stage));
    let done = if d.stage == Stage::Merged {
        ACCENT
    } else {
        GREEN
    };
    let colour = pill_color(d.pill);
    // Labels 17 cells, glyphs and their spaces 8, then " ━ " between each (31 + 3 × link).
    let link = if width >= 34 {
        ((width - 31) / 3).min(6)
    } else {
        0
    };
    let mut spans = Vec::new();
    for (i, name) in STEPS.iter().enumerate() {
        let (glyph, style) = match i.cmp(&step) {
            std::cmp::Ordering::Less => ("✓".to_owned(), fg(done)),
            std::cmp::Ordering::Equal if is_spinning(row) => {
                spans.push(spinner(beat, row.pr.number, colour));
                (String::new(), bold(colour))
            }
            std::cmp::Ordering::Equal => {
                let glyph = if d.is_moving {
                    "◌".to_owned()
                } else {
                    mark(d.pill)
                };
                (glyph, bold(colour))
            }
            std::cmp::Ordering::Greater => ("○".to_owned(), fg(DIM)),
        };
        let text = if glyph.is_empty() {
            format!(" {name}")
        } else {
            format!("{glyph} {name}")
        };
        spans.push(span(text, style));
        if i + 1 < STEPS.len() {
            let joint = if i < step { "━" } else { "┈" };
            let style = if i < step { fg(done) } else { fg(DIM) };
            // Too narrow for connectors: two spaces between the steps.
            let text = if link == 0 {
                "  ".to_owned()
            } else {
                format!(" {} ", joint.repeat(link))
            };
            spans.push(span(text, style));
        }
    }
    Line::from(spans)
}

/// One check as the card lists it: glyph, name, what it did.
fn check_lines(pr: &GhPr, width: usize) -> Vec<(u8, Line<'static>)> {
    let mut out: Vec<(u8, Line<'static>)> = latest_runs(pr.checks())
        .into_iter()
        .map(|item| {
            let name = item
                .name
                .clone()
                .or_else(|| item.context.clone())
                .unwrap_or_else(|| "check".to_owned());
            let is_started = if is_status_context(item) {
                matches!(item.state, Some(Some(prw_core::gh::StatusState::Pending)))
            } else {
                item.status == Some(prw_core::gh::CheckStatus::InProgress)
            };
            let (rank, glyph, colour, word) = match verdict_of(item) {
                CheckVerdict::Failed => (0, "✗", RED, "failed"),
                CheckVerdict::Pending if is_started => (1, "◐", YELLOW, "running"),
                CheckVerdict::Pending => (2, "·", DIM, "queued"),
                CheckVerdict::Passed => (3, "✓", GREEN, "passed"),
            };
            let name_style = if rank == 0 { fg(FG) } else { fg(BODY) };
            let line = Line::from(vec![
                span(format!("{glyph} "), fg(colour)),
                span(fit(&name, width.saturating_sub(2 + 8)), name_style),
                span(right(word, 8), fg(if rank == 3 { DIM } else { colour })),
            ]);
            (rank, line)
        })
        .collect();
    out.sort_by_key(|(rank, _)| *rank);
    out
}

/// What the card says of merging, where it is open, in the colour of where it stands.
fn merge_words(pr: &GhPr, merge: &Mergeable) -> Option<(String, Color)> {
    let (words, standing) = standing(pr, merge)?;
    let colour = match standing {
        Standing::Auto => TEAL,
        Standing::Can => GREEN,
        Standing::Conflicts => PEACH,
        Standing::Draft | Standing::ReadOnly => DIM,
        Standing::CanAuto => BLUE,
        Standing::Waiting => YELLOW,
    };
    Some((words, colour))
}

fn verdict_color(verdict: Option<Verdict>) -> Color {
    match verdict {
        Some(Verdict::Approved) => GREEN,
        Some(Verdict::Changes) => RED,
        Some(Verdict::Commented) => BLUE,
        Some(Verdict::Requested) | None => DIM,
    }
}

/// "checked 12s ago", where the PR has data.
fn checked(model: &Model, row: &Row) -> Option<String> {
    row.checked_at
        .map(|at| format!("checked {}", ago(model.now().saturating_sub(at))))
}

/// The watchers line: "by @octocat · as hubot  ◆ app-retry, web" (the login only where gh
/// has more than one).
fn by_line(row: &Row, author: &str, login: Option<&str>, width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    if !author.is_empty() {
        spans.push(span(format!("by @{author}"), fg(DIM)));
    }
    if let Some(login) = login.filter(|l| !l.is_empty()) {
        if !spans.is_empty() {
            spans.push(span(" · ", fg(DIM)));
        }
        spans.push(span(format!("as {login}"), fg(BODY)));
    }
    if !row.watchers.is_empty() {
        let (glyph, colour) = agent_look(row.agent.as_deref());
        if !spans.is_empty() {
            spans.push(Span::raw("  "));
        }
        spans.push(span(format!("{glyph} "), fg(colour)));
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(span(
            clip(&row.watchers.join(", "), width.saturating_sub(used)),
            fg(BODY),
        ));
    }
    Line::from(spans)
}

/// The selected PR's box, titled `owner/repo #12`, and the area inside it; a bare
/// "Details" box where nothing is selected.
fn selected_box<'m>(frame: &mut Frame, r: Rect, model: &'m Model) -> Option<(&'m Row, Rect)> {
    let Some(row) = model.selected_row() else {
        frame.render_widget(boxed("Details", false), r);
        return None;
    };
    let title = format!("{} #{}", row.repo, row.pr.number);
    let block = boxed(&clip(&title, usize_of(r.width).saturating_sub(6)), false)
        .padding(Padding::horizontal(1));
    let inner = block.inner(r);
    frame.render_widget(block, r);
    Some((row, inner))
}

fn title_lines(row: &Row, iw: usize) -> Vec<Line<'static>> {
    let title = if row.pr.title.is_empty() {
        "Loading…"
    } else {
        &row.pr.title
    };
    wrap(title, iw, 2)
        .into_iter()
        .map(|l| Line::from(span(l, bold(FG))))
        .collect()
}

/// The card's top: title, branch and diff, who, the pill, the headline, the steps.
fn detail_head(
    model: &Model,
    row: &Row,
    gh: Option<&GhPr>,
    iw: usize,
    beat: u64,
) -> Vec<Line<'static>> {
    let d = &row.pr.derived;
    let mut head = title_lines(row, iw);
    if let Some(pr) = gh {
        let meta = meta_of(pr);
        let diff = format!("+{} −{}", meta.additions, meta.deletions);
        let branch = clip(
            &format!("{} → {}", meta.branch, meta.base),
            iw.saturating_sub(cells(&diff) + 1),
        );
        let gap = iw.saturating_sub(cells(&branch) + cells(&diff));
        head.push(Line::from(vec![
            span(branch, fg(BLUE)),
            Span::raw(" ".repeat(gap)),
            span(format!("+{}", meta.additions), fg(GREEN)),
            span(format!(" −{}", meta.deletions), fg(RED)),
        ]));
        head.push(by_line(
            row,
            &meta.author,
            model.login_of(row).as_deref(),
            iw,
        ));
    } else {
        head.push(by_line(row, "", model.login_of(row).as_deref(), iw));
    }
    head.push(Line::raw(""));
    head.push(Line::from(pill(d.pill)));
    // Beside the headline, as the mod puts it: how long ago it merged, or the outdated threads.
    let note = match gh {
        Some(pr) if pr.state == PrState::Merged => pr
            .merged_at
            .as_deref()
            .and_then(parse_iso)
            .map(|at| ago(model.now().saturating_sub(at))),
        Some(pr) if pr.state == PrState::Open => reviews_of(pr)
            .map(|r| threads_beside(&d.headline, &r).note)
            .filter(|n| !n.is_empty()),
        _ => None,
    };
    let headline = match &note {
        Some(note) => format!("{} · {note}", d.headline),
        None => d.headline.clone(),
    };
    for l in wrap(&headline, iw, 2) {
        head.push(Line::from(span(l, fg(tone_color(d.tone)))));
    }
    head.push(stepper(row, iw, beat));
    head
}

/// The checks' heading, bar and activity; and one line per check.
fn detail_checks(
    row: &Row,
    gh: Option<&GhPr>,
    iw: usize,
    beat: u64,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let checks = &row.pr.derived.checks;
    let Some(pr) = gh.filter(|_| checks.total > 0) else {
        return (Vec::new(), Vec::new());
    };
    let count = format!("{}/{}", checks.passed, checks.total);
    let mut head = vec![
        Line::raw(""),
        Line::from(vec![
            span(fit("CHECKS", iw.saturating_sub(cells(&count))), label()),
            span(count, fg(DIM)),
        ]),
        Line::from(bar(checks, iw, beat)),
    ];
    // What CI is doing, unless the headline says it already (CI running, a draft).
    let doing = activity(checks);
    if !doing.is_empty() && !row.pr.derived.headline.contains(&doing) {
        head.push(Line::from(span(clip(&doing, iw), fg(YELLOW))));
    }
    let rows = check_lines(pr, iw).into_iter().map(|(_, l)| l).collect();
    (head, rows)
}

/// Reviews, merging, and the error, if any.
fn detail_tail(row: &Row, gh: Option<&GhPr>, iw: usize) -> Vec<Line<'static>> {
    let d = &row.pr.derived;
    let mut tail = Vec::new();
    if let Some(pr) = gh.filter(|pr| pr.state == PrState::Open) {
        tail.push(Line::raw(""));
        tail.push(Line::from(span("REVIEW", label())));
        let reviews = reviews_of(pr);
        let pieces = reviews
            .as_ref()
            .map(|r| verdict_line(r, iw))
            .unwrap_or_default();
        if pieces.is_empty() {
            tail.push(Line::from(span("No reviews yet", fg(DIM))));
        } else {
            let mut spans = Vec::new();
            for (i, piece) in pieces.into_iter().enumerate() {
                if i > 0 {
                    spans.push(span(" · ", fg(DIM)));
                }
                spans.push(span(piece.text, fg(verdict_color(piece.verdict))));
            }
            tail.push(Line::from(spans));
        }
        if let Some(reviews) = &reviews {
            let line = threads_beside(&d.headline, reviews).line;
            if !line.is_empty() {
                tail.push(Line::from(span(clip(&line, iw), fg(YELLOW))));
            }
        }
        if let Some((words, colour)) = merge_words(pr, &mergeable_of(pr)) {
            tail.push(Line::raw(""));
            tail.push(Line::from(span("MERGE", label())));
            tail.push(Line::from(span(clip(&words, iw), fg(colour))));
        }
    }
    if let Some(error) = &row.pr.error {
        tail.push(Line::raw(""));
        for l in wrap(&format!("⚠ {error}"), iw, 3) {
            tail.push(Line::from(span(l, fg(RED))));
        }
    }
    tail
}

/// The selected PR as a full card (three panes, strip): the URL at the bottom, the check
/// rows taking what the rest leaves ("+ N more" standing for the ones cut).
fn detail(frame: &mut Frame, r: Rect, model: &Model, ctx: Ctx, hits: &mut Hits) {
    hits.add(r, Hit::Pane(Focus::Prs));
    let Some((row, inner)) = selected_box(frame, r, model) else {
        return;
    };
    let iw = usize_of(inner.width);
    let ih = usize_of(inner.height);
    let gh = model.entry(&row.pr.url).and_then(|e| e.pr.as_ref());
    let head = detail_head(model, row, gh, iw, ctx.beat);
    let (checks_head, check_rows) = detail_checks(row, gh, iw, ctx.beat);
    let mut tail = detail_tail(row, gh, iw);
    let mut foot = Vec::new();
    if let Some(when) = checked(model, row) {
        foot.push(Line::from(span(clip(&when, iw), fg(DIM))));
    }
    // The scheme says nothing, and leaves a narrow card's URL whole.
    let url = row.pr.url.strip_prefix("https://").unwrap_or(&row.pr.url);
    foot.push(Line::from(span(clip(url, iw), fg(DIM))));

    // The actions a line each where the checks leave room, else as keys on one line.
    let fixed = head.len() + checks_head.len() + tail.len() + foot.len() + 1;
    let offers = model.offers();
    let actions = action_lines(model, iw);
    // Where the actions went: a line each in the tail (from this line of it), or as keys.
    let mut actions_at = None;
    let mut keys = None;
    if ih.saturating_sub(fixed) >= check_rows.len() + actions.len() {
        // Past the lines action_lines puts above the offers.
        actions_at = Some(tail.len() + actions.len() - offers.len());
        tail.extend(actions);
    } else if !actions.is_empty() {
        let (line, spots) = action_keys(model, iw);
        foot.insert(0, line);
        keys = Some(spots);
    }
    let fixed = head.len() + checks_head.len() + tail.len() + foot.len() + 1;
    let room = ih.saturating_sub(fixed);
    let total = check_rows.len();
    let mut lines = head;
    lines.extend(checks_head);
    if total <= room {
        lines.extend(check_rows);
    } else if room > 0 {
        lines.extend(check_rows.into_iter().take(room - 1));
        let more = format!("+ {} more", total - (room - 1));
        lines.push(Line::from(span(more, fg(DIM))));
    }
    let tail_at = lines.len();
    lines.extend(tail);
    let gap = ih.saturating_sub(lines.len() + foot.len());
    lines.extend(std::iter::repeat_n(Line::raw(""), gap));
    let foot_at = lines.len();
    lines.extend(foot);
    frame.render_widget(Paragraph::new(lines), inner);
    if let Some(at) = actions_at {
        for (i, offer) in offers.iter().enumerate() {
            if let (Some(line), Some(hit)) = (row_of(inner, tail_at + at + i), offer_key(offer.key))
            {
                hits.add(line, hit);
            }
        }
    }
    if let (Some(spots), Some(line)) = (keys, row_of(inner, foot_at)) {
        hits.spots(line, spots);
    }
}

/// The selected PR in a few lines, under the table (side layout).
fn card(frame: &mut Frame, r: Rect, model: &Model, ctx: Ctx, hits: &mut Hits) {
    hits.add(r, Hit::Pane(Focus::Prs));
    let Some((row, inner)) = selected_box(frame, r, model) else {
        return;
    };
    let iw = usize_of(inner.width);
    let d = &row.pr.derived;
    let entry = model.entry(&row.pr.url);
    let gh = entry.and_then(|e| e.pr.as_ref());
    let mut lines = title_lines(row, iw);
    let when = checked(model, row).unwrap_or_default();
    let pill_w = cells(d.pill.text()) + 2;
    let when = clip(&when, iw.saturating_sub(pill_w + 1));
    lines.push(Line::from(vec![
        pill(d.pill),
        Span::raw(" ".repeat(iw.saturating_sub(pill_w + cells(&when)))),
        span(when, fg(DIM)),
    ]));
    lines.push(Line::from(span(
        clip(&d.headline, iw),
        fg(tone_color(d.tone)),
    )));
    lines.push(stepper(row, iw, ctx.beat));
    let mut facts = Vec::new();
    if d.checks.total > 0 {
        facts.push(format!("{}/{} checks", d.checks.passed, d.checks.total));
    }
    if let Some(pr) = gh
        && let Some(reviews) = reviews_of(pr)
    {
        let pieces = verdict_line(&reviews, iw / 2);
        if !pieces.is_empty() {
            facts.push(
                pieces
                    .into_iter()
                    .map(|p| p.text)
                    .collect::<Vec<_>>()
                    .join(" · "),
            );
        }
    }
    if !facts.is_empty() {
        lines.push(Line::from(span(clip(&facts.join(" · "), iw), fg(DIM))));
    }
    lines.push(by_line(
        row,
        gh.map(|pr| meta_of(pr).author).unwrap_or_default().as_str(),
        model.login_of(row).as_deref(),
        iw,
    ));
    if let Some(error) = &row.pr.error {
        lines.push(Line::from(span(clip(&format!("⚠ {error}"), iw), fg(RED))));
    }
    let (keys, spots) = action_keys(model, iw);
    if let Some(line) = row_of(inner, lines.len()) {
        hits.spots(line, spots);
    }
    lines.push(keys);
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---- Actions ----------------------------------------------------------------------------

fn look_color(look: Look) -> Color {
    match look {
        Look::Merge => GREEN,
        Look::Handoff => PEACH,
        Look::Plain => BODY,
        Look::Busy => YELLOW,
    }
}

fn tone_of(tone: Tone) -> Color {
    match tone {
        Tone::Good => GREEN,
        Tone::Bad => RED,
        Tone::Info => BODY,
    }
}

/// The selected PR's actions, one a line, each with its key (the detail pane).
fn action_lines(model: &Model, iw: usize) -> Vec<Line<'static>> {
    let offers = model.offers();
    if offers.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![Line::raw(""), Line::from(span("ACTIONS", label()))];
    lines.extend(offers.into_iter().map(|o| {
        Line::from(vec![
            span(format!("[{}] ", o.key), fg(ACCENT)),
            span(clip(&o.words, iw.saturating_sub(4)), fg(look_color(o.look))),
        ])
    }));
    lines
}

/// The actions as keys on one line, as many as fit (the narrow card):
/// "m merge  f fix  c comment  o open".
fn action_keys(model: &Model, iw: usize) -> (Line<'static>, Spots) {
    let mut line = Marked::default();
    for o in model.offers() {
        if line.width() + cells(o.key) + 1 + cells(&o.short) > iw {
            break;
        }
        let spans = [
            span(o.key, fg(ACCENT)),
            span(format!(" {}", o.short), fg(look_color(o.look))),
        ];
        match offer_key(o.key) {
            Some(hit) => line.hit(spans, hit),
            None => line.spans.extend(spans),
        }
        line.push(span("  ", Style::new()));
    }
    line.done()
}

/// The last `n` cells of `text`.
fn tail_cells(text: &str, n: usize) -> String {
    let mut used = 0;
    let mut kept: Vec<char> = text
        .chars()
        .rev()
        .take_while(|c| {
            used += c.width().unwrap_or(0);
            used <= n
        })
        .collect();
    kept.reverse();
    kept.into_iter().collect()
}

/// The question on the bottom line, with the keys that answer it (a click on one presses it).
fn ask_line(ask: &Ask, width: usize) -> (Line<'static>, Spots) {
    let key = |k: &str| span(format!("  {k}"), fg(ACCENT));
    let what = |w: &str| span(format!(" {w}"), fg(DIM));
    let mut line = Marked::default();
    match ask {
        Ask::Merge {
            number,
            base,
            run,
            methods,
            ..
        } => {
            let mut answers = vec![('y', "yes".to_owned()), ('n', "no".to_owned())];
            if let Some(method) = run.method().filter(|_| methods.len() > 1) {
                answers.push(('m', next_method(methods, method).as_str().to_owned()));
            }
            let tail: usize = answers.iter().map(|(_, w)| 4 + cells(w)).sum();
            let room = width.saturating_sub(tail);
            line.push(Span::raw(" "));
            line.push(span(
                clip(&merge_question(*run, *number, base), room),
                bold(FG),
            ));
            for (k, w) in answers {
                line.hit([key(&k.to_string()), what(&w)], Hit::Key(Key::Char(k)));
            }
        }
        Ask::Comment {
            number,
            text,
            login,
            ..
        } => {
            let lead = format!(" comment on #{number} › ");
            // "  ⏎ post · alt-⏎ new line · esc cancel", where there is room: the post and the
            // cancel take clicks.
            let parts = ["  ", "⏎ post", " · alt-⏎ new line · ", "esc cancel"];
            let hint = if width >= 70 {
                parts.iter().map(|p| cells(p)).sum()
            } else {
                0
            };
            let room = width.saturating_sub(cells(&lead) + hint + 1);
            line.push(span(lead, bold(ACCENT)));
            if text.is_empty() {
                line.push(span(" ", Style::new().bg(ACCENT)));
                if !login.is_empty() {
                    line.push(span(clip(&format!(" as {login}"), room), fg(DIM)));
                }
            } else {
                line.push(span(tail_cells(&text.replace('\n', "↵"), room), fg(FG)));
                line.push(span(" ", Style::new().bg(ACCENT)));
            }
            line.push(Span::raw(
                " ".repeat(width.saturating_sub(line.width() + hint)),
            ));
            if hint > 0 {
                let [gap, post, alt, cancel] = parts;
                line.push(span(gap, fg(DIM)));
                line.hit([span(post, fg(DIM))], Hit::Key(Key::Enter));
                line.push(span(alt, fg(DIM)));
                line.hit([span(cancel, fg(DIM))], Hit::Key(Key::Esc));
            }
        }
        Ask::Pick { number, kinds, .. } => {
            line.push(span(format!(" Hand off #{number}:"), bold(FG)));
            for kind in kinds {
                line.hit(
                    [
                        key(&kind.key().to_string()),
                        span(format!(" {}", kind.about()), fg(BODY)),
                    ],
                    Hit::Key(Key::Char(kind.key())),
                );
            }
            line.hit([key("esc"), what("cancel")], Hit::Key(Key::Esc));
        }
    }
    line.done()
}

/// The prompt last copied, the whole screen, to read, or to select and copy by hand where
/// the copy can't be confirmed (OSC 52). No border, so a selection takes only the text.
fn prompt_pane(frame: &mut Frame, area: Rect, top: usize, copied: &Copied) {
    let width = usize_of(area.width);
    let height = usize_of(area.height).saturating_sub(2);
    let body = prompt_body(&copied.text, width);
    let top = top.min(body.len().saturating_sub(height));
    let shown = body.len().min(top + height);
    let head = format!(
        " {} prompt for #{} · for {} · j/k scroll · esc closes",
        copied.kind.name(),
        copied.number,
        copied.to
    );
    let foot = format!(
        " lines {}-{shown} of {} · select with the mouse to copy by hand",
        (top + 1).min(shown),
        body.len()
    );
    let mut lines = vec![filled(
        vec![span(clip(&head, width), bold(FG))],
        width,
        PANEL,
    )];
    lines.extend(
        body[top..shown]
            .iter()
            .map(|l| Line::from(span(l.clone(), fg(BODY)))),
    );
    lines.extend(std::iter::repeat_n(
        Line::raw(""),
        height.saturating_sub(shown - top),
    ));
    lines.push(filled(
        vec![span(clip(&foot, width), fg(DIM))],
        width,
        PANEL,
    ));
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).style(Style::new().bg(INK)), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_fits_its_cells() {
        assert_eq!(clip("Retry uploads", 6), "Retry…");
        assert_eq!(clip("short", 9), "short");
        assert_eq!(clip("anything", 0), "");
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(right("7", 3), "  7");
        assert_eq!(cells(&fit("💬 COMMENTS", 5)), 5);
        assert_eq!(
            wrap("Keep the cart across a page refresh", 12, 2),
            ["Keep the", "cart across…"]
        );
        assert_eq!(tilde("/home/ada/src/app", "/home/ada"), "~/src/app");
        assert_eq!(tilde("/home/adam", "/home/ada"), "/home/adam");
        assert_eq!(short_count(4620), "4.6k");
        assert_eq!(short_count(812), "812");
    }

    #[test]
    fn a_glint_runs_along_the_pending_and_rests_off_it() {
        // At the first beat it stands four cells before the run: all yellow.
        assert!(shine(10, 0).all(|c| c == YELLOW));
        // Six beats on, at the third cell, brightest there and fading either side.
        let cells: Vec<Color> = shine(10, 6).collect();
        assert_eq!(cells[2], mix(YELLOW, FG, 0.85));
        assert!(cells[1] != YELLOW && cells[5] == YELLOW && cells[9] == YELLOW);
        for beat in 0..40 {
            assert_eq!(shine(7, beat).count(), 7);
        }
        assert_eq!(mix(YELLOW, FG, 0.0), YELLOW);
        assert_eq!(mix(YELLOW, FG, 1.0), FG);
        assert_eq!(mix(Color::Reset, FG, 0.5), Color::Reset);
    }

    #[test]
    fn the_bar_shares_its_width_as_the_mod_does() {
        assert_eq!(allot(&[1, 1, 4], 6), [1, 1, 4]);
        assert_eq!(allot(&[1, 0, 99], 5), [1, 0, 4]);
        assert_eq!(allot(&[0, 0, 0], 5), [0, 0, 0]);
        assert_eq!(allot(&[3, 0, 0], 7), [7, 0, 0]);
        // Too narrow for a cell each: the first of the largest gives one back.
        assert_eq!(allot(&[1, 1, 1], 2), [0, 1, 1]);
        for width in 0..20 {
            for counts in [[1, 2, 3], [0, 5, 1], [9, 9, 9]] {
                assert_eq!(allot(&counts, width).iter().sum::<usize>(), width);
            }
        }
    }
}
