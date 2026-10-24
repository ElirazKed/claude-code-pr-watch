//! What people write on a PR, drawn as GitHub draws its markdown: tui-markdown reads it, in
//! the theme's colours; this wraps it to the width it has, each list item and quote hung
//! under itself, and tidies what a terminal can't show (a link's address, HTML tags).

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use tui_markdown::{AlertKind, Options, StyleSheet, from_str_with_options};
use unicode_width::UnicodeWidthChar;

use super::{code_in, code_line, detab, span};
use crate::theme::{ACCENT, BLUE, BODY, DIM, FG, GREEN, RED, YELLOW, bold, fg};

/// What a code block's fence is drawn as, so [`markdown`] can tell a block's language and
/// leave the fences out: an invisible separator, which no one writes.
const FENCE: &str = "\u{2063}";

/// The theme, as tui-markdown asks for it. Its markers are left out: a heading shows bold, a
/// code block on the code ground.
#[derive(Debug, Clone, Copy)]
struct Look;

impl StyleSheet for Look {
    fn heading(&self, level: u8) -> Style {
        if level <= 2 { bold(FG) } else { bold(BODY) }
    }

    fn heading_marker(&self, _level: u8) -> &'static str {
        ""
    }

    fn code(&self) -> Style {
        fg(FG).add_modifier(Modifier::REVERSED)
    }

    fn code_block_fence(&self) -> &'static str {
        FENCE
    }

    fn link(&self) -> Style {
        fg(BLUE).add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        fg(DIM)
    }

    fn html(&self) -> Style {
        fg(DIM).add_modifier(Modifier::HIDDEN)
    }

    fn alert(&self, kind: AlertKind) -> Style {
        fg(match kind {
            AlertKind::Note => BLUE,
            AlertKind::Tip => GREEN,
            AlertKind::Important => ACCENT,
            AlertKind::Warning => YELLOW,
            AlertKind::Caution => RED,
        })
    }

    fn alert_icon(&self, _kind: AlertKind) -> &'static str {
        ""
    }

    fn table_header(&self) -> Style {
        bold(FG)
    }

    fn table_border(&self) -> Style {
        fg(DIM)
    }

    fn list_marker(&self) -> Style {
        fg(DIM)
    }

    fn image_alt(&self) -> Style {
        fg(DIM).add_modifier(Modifier::ITALIC)
    }
}

/// Inline code, once it is drawn: the marker [`Look::code`] stands in for.
fn inline_code() -> Style {
    fg(YELLOW)
}

/// `text`, markdown, in lines of at most `width` cells; runs of blank lines as one.
pub(super) fn markdown(text: &str, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let options = Options::new(Look).table_width(u16::try_from(width).unwrap_or(u16::MAX));
    let drawn = from_str_with_options(text, &options);
    // The language of the code block the line is in, from its opening fence.
    let mut lang: Option<String> = None;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for line in &drawn.lines {
        if line.style != Look.code() {
            lines.extend(shown(line, width));
            continue;
        }
        let text = plain(line);
        // A block in a quote is drawn after the quote's marks: `> `.
        let (quote, code) = text.split_at(quote_len(&text));
        if let Some(info) = code.strip_prefix(FENCE) {
            // The language is the info string's first word: ```` ```diff title=x ````.
            lang = if lang.is_some() {
                None
            } else {
                Some(
                    info.split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                )
            };
        } else {
            let is_diff = matches!(lang.as_deref(), Some("diff" | "patch"));
            // The marks stand where the code ground's own lead space would sit.
            let marks = quote.trim_end();
            let mut drawn = if is_diff {
                code_line(code, marks.len(), width)
            } else {
                code_in(code, marks.len(), width, BODY)
            };
            if let Some(lead) = drawn.spans.first_mut() {
                *lead = span(marks.to_owned(), fg(DIM));
            }
            lines.push(drawn);
        }
    }
    let mut out: Vec<Line<'static>> = Vec::new();
    for line in lines {
        let is_blank = line.width() == 0;
        if is_blank && out.last().is_none_or(|l| l.width() == 0) {
            continue;
        }
        out.push(line);
    }
    if out.last().is_some_and(|l| l.width() == 0) {
        out.pop();
    }
    out
}

/// How wide [`first_words`] lays a line out: more than a summary shows of it.
const SUMMARY: usize = 300;
/// How much of a comment [`first_words`] reads for its first line.
const SUMMARY_READ: usize = 2000;

/// The first line of `text` that says something, as plain words: a thread's summary. A rule
/// or a table's border says nothing.
pub(super) fn first_words(text: &str) -> String {
    let end = text
        .char_indices()
        .nth(SUMMARY_READ)
        .map_or(text.len(), |(at, _)| at);
    markdown(&text[..end], SUMMARY)
        .into_iter()
        .map(|line| plain(&line).trim().to_owned())
        .find(|words| words.chars().any(says_something))
        .unwrap_or_default()
}

/// How many bytes of `text` are a quote's marks as tui-markdown draws them: `>`s, then a
/// space. None where it is not quoted, so a code line's own indent stays.
fn quote_len(text: &str) -> usize {
    let marks = text.bytes().take_while(|&b| b == b'>').count();
    if marks > 0 && text.as_bytes().get(marks) == Some(&b' ') {
        marks + 1
    } else {
        0
    }
}

/// Whether `c` says something in a summary: not a space, a quote's mark or a box-drawing
/// stroke (a rule, a table's border), so an emoji alone (`🚀`) or `?` still does.
fn says_something(c: char) -> bool {
    !c.is_whitespace() && c != '>' && !('\u{2500}'..='\u{257F}').contains(&c)
}

fn plain(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

/// One line as tui-markdown drew it, as it shows `width` cells wide: none where it was only
/// HTML tags.
fn shown(line: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    let text = plain(line);
    // A table, as it is drawn in a list item or a quote too: boxed already, so cut, not wrapped.
    if text
        .trim_start_matches(['>', ' '])
        .starts_with(['┌', '│', '├', '└'])
    {
        let spans: Vec<Span<'static>> = line.spans.iter().map(|s| owned(s, line.style)).collect();
        return vec![Line::from(clip_spans(&spans, width))];
    }
    if matches!(text.trim(), "---" | "***" | "___") {
        return vec![Line::from(span("─".repeat(width), fg(DIM)))];
    }
    let spans = unlinked(&line.spans);
    let had_words = !text.trim().is_empty();
    let spans: Vec<Span<'static>> = spans
        .iter()
        .filter_map(|s| untagged(s, line.style))
        .collect();
    let chars = styled_chars(&spans);
    if had_words && chars.iter().all(|(c, _)| c.is_whitespace()) {
        return Vec::new();
    }
    let (prefix, words) = chars.split_at(prefix_len(&chars));
    let prefix = marked(prefix);
    wrap_chars(&prefix, words, width)
}

/// `style` less the marks [`Look`] tells code and HTML by.
fn unmarked(style: Style) -> Style {
    Style {
        add_modifier: style.add_modifier - (Modifier::REVERSED | Modifier::HIDDEN),
        ..style
    }
}

/// `s` owned, over the line's own style: inline code in its colour.
fn owned(s: &Span<'_>, under: Style) -> Span<'static> {
    let style = if s.style.add_modifier.contains(Modifier::REVERSED) {
        inline_code()
    } else {
        unmarked(fg(BODY).patch(under).patch(s.style))
    };
    Span::styled(detab(&s.content), style)
}

/// The spans less each link's address, tui-markdown's ` (address)` after its words: the
/// words, underlined, say it is a link. Those words may be code or an image's, not
/// underlined, so only the address is told by its look.
fn unlinked<'a>(spans: &'a [Span<'a>]) -> Vec<&'a Span<'a>> {
    // A link in bold is bold too: its colour and underline tell it.
    let is_link = |s: &Span<'_>| {
        s.style.fg == Look.link().fg && s.style.add_modifier.contains(Modifier::UNDERLINED)
    };
    let mut out: Vec<&Span<'_>> = Vec::new();
    let mut i = 0;
    while i < spans.len() {
        let is_address = spans[i].content == " ("
            && spans.get(i + 1).is_some_and(is_link)
            && spans.get(i + 2).is_some_and(|s| s.content == ")")
            && i > 0;
        if is_address {
            i += 3;
        } else {
            out.push(&spans[i]);
            i += 1;
        }
    }
    out
}

/// A span with its HTML tags taken out: a `<summary>` as `▸ `, a `<br>` as a space; none
/// where nothing is left.
fn untagged(s: &Span<'_>, under: Style) -> Option<Span<'static>> {
    if !s.style.add_modifier.contains(Modifier::HIDDEN) {
        return Some(owned(s, under));
    }
    let mut words = String::new();
    let mut rest = s.content.as_ref();
    while let Some(at) = rest.find('<') {
        words.push_str(&rest[..at]);
        // `a < b` is words, not a tag: a tag opens on a letter, `/` or `!`.
        let is_tag = rest[at + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!'));
        if !is_tag {
            words.push('<');
            rest = &rest[at + 1..];
            continue;
        }
        let Some(end) = rest[at..].find('>') else {
            words.push_str(&rest[at..]);
            rest = "";
            break;
        };
        let tag = rest[at + 1..at + end].trim().to_ascii_lowercase();
        if tag == "summary" {
            words.push_str("▸ ");
        } else if tag.starts_with("br") {
            words.push(' ');
        }
        rest = &rest[at + end + 1..];
    }
    words.push_str(rest);
    (!words.is_empty()).then(|| span(words, unmarked(fg(BODY).patch(under))))
}

fn styled_chars(spans: &[Span<'static>]) -> Vec<(char, Style)> {
    spans
        .iter()
        .flat_map(|s| s.content.chars().map(move |c| (c, s.style)))
        .collect()
}

/// How many of `chars` lead the line rather than say something: the quote marks, then a
/// list item's marker and its task box.
fn prefix_len(chars: &[(char, Style)]) -> usize {
    let at = |i: usize| chars.get(i).map(|&(c, _)| c);
    let mut i = chars
        .iter()
        .take_while(|&&(c, _)| matches!(c, '>' | ' '))
        .count();
    let digits = chars[i..]
        .iter()
        .take_while(|(c, _)| c.is_ascii_digit())
        .count();
    let is_bullet = digits == 0 && matches!(at(i), Some('-' | '*' | '+')) && at(i + 1) == Some(' ');
    let is_number =
        digits > 0 && matches!(at(i + digits), Some('.' | ')')) && at(i + digits + 1) == Some(' ');
    if is_bullet {
        i += 2;
    } else if is_number {
        i += digits + 2;
    } else {
        return i;
    }
    let is_task = at(i) == Some('[')
        && matches!(at(i + 1), Some(' ' | 'x' | 'X'))
        && at(i + 2) == Some(']')
        && at(i + 3) == Some(' ');
    if is_task { i + 4 } else { i }
}

/// A line's lead as GitHub shows it: `•` for a bullet, `✓` or `○` for a task.
fn marked(prefix: &[(char, Style)]) -> Vec<(char, Style)> {
    let quote = prefix
        .iter()
        .take_while(|&&(c, _)| matches!(c, '>' | ' '))
        .count();
    let rest: String = prefix[quote..].iter().map(|&(c, _)| c).collect();
    let mark = match rest.as_str() {
        "- [x] " | "* [x] " | "+ [x] " | "- [X] " | "* [X] " | "+ [X] " => ("✓ ", fg(GREEN)),
        "- [ ] " | "* [ ] " | "+ [ ] " => ("○ ", fg(DIM)),
        "- " | "* " | "+ " => ("• ", fg(DIM)),
        _ => return prefix.to_vec(),
    };
    prefix[..quote]
        .iter()
        .copied()
        .chain(mark.0.chars().map(|c| (c, mark.1)))
        .collect()
}

fn width_of(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// `words` after `prefix`, in lines of at most `width` cells: broken at spaces, a word wider
/// than a line broken where it must. The lines after the first hang under the first's words,
/// a quote's marks carried down.
fn wrap_chars(
    prefix: &[(char, Style)],
    words: &[(char, Style)],
    width: usize,
) -> Vec<Line<'static>> {
    let hang: Vec<(char, Style)> = prefix
        .iter()
        .map(|&(c, s)| if c == '>' { (c, s) } else { (' ', s) })
        .collect();
    let hang_cells: usize = hang.iter().map(|&(c, _)| width_of(c)).sum();
    let hang = if hang_cells < width { hang } else { Vec::new() };
    let mut lines: Vec<Vec<(char, Style)>> = vec![prefix.to_vec()];
    let mut used: usize = prefix.iter().map(|&(c, _)| width_of(c)).sum();
    let mut is_start = true;
    for (gap, word) in split_words(words) {
        let word = mentioned(word);
        let cells: usize = word.iter().map(|&(c, _)| width_of(c)).sum();
        if !is_start && used + 1 + cells > width {
            lines.push(hang.clone());
            used = hang.iter().map(|&(c, _)| width_of(c)).sum();
            is_start = true;
        }
        if !is_start {
            lines.last_mut().expect("a line").push((' ', gap));
            used += 1;
        }
        for &(c, s) in &word {
            let w = width_of(c);
            if used + w > width && used > 0 {
                lines.push(hang.clone());
                used = hang.iter().map(|&(c, _)| width_of(c)).sum();
                if used + w > width {
                    lines.last_mut().expect("a line").clear();
                    used = 0;
                }
            }
            lines.last_mut().expect("a line").push((c, s));
            used += w;
        }
        is_start = false;
    }
    lines
        .into_iter()
        .map(|chars| Line::from(regroup(&chars)))
        .collect()
}

/// The words of `chars`, each with the style of the space before it.
fn split_words(chars: &[(char, Style)]) -> Vec<(Style, Vec<(char, Style)>)> {
    let mut out = Vec::new();
    let mut gap = Style::default();
    let mut word = Vec::new();
    let mut is_gap_set = false;
    for &(c, s) in chars {
        if c == ' ' {
            if !word.is_empty() {
                out.push((gap, std::mem::take(&mut word)));
                is_gap_set = false;
            }
            if !is_gap_set {
                gap = s;
                is_gap_set = true;
            }
        } else {
            word.push((c, s));
        }
    }
    if !word.is_empty() {
        out.push((gap, word));
    }
    out
}

/// A word in the accent where it names someone (`@bob`) or a PR or issue (`#12`), in plain
/// words or a quote's (not code, a link or bold).
fn mentioned(word: Vec<(char, Style)>) -> Vec<(char, Style)> {
    let text: String = word.iter().map(|&(c, _)| c).collect();
    let name = text.trim_end_matches(['.', ',', ':', ';', ')', '!', '?']);
    let is_plain = word.iter().all(|&(_, s)| s == fg(BODY) || s == fg(DIM));
    let is_mention = name.len() > 1
        && (name.strip_prefix('@').is_some_and(|n| {
            n.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/'))
        }) || name
            .strip_prefix('#')
            .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit())));
    if !(is_plain && is_mention) {
        return word;
    }
    let n = name.chars().count();
    word.into_iter()
        .enumerate()
        .map(|(i, (c, s))| if i < n { (c, fg(ACCENT)) } else { (c, s) })
        .collect()
}

/// Runs of one style, as spans.
fn regroup(chars: &[(char, Style)]) -> Vec<Span<'static>> {
    chars
        .chunk_by(|a, b| a.1 == b.1)
        .map(|run| Span::styled(run.iter().map(|&(c, _)| c).collect::<String>(), run[0].1))
        .collect()
}

/// `spans` cut to `width` cells.
fn clip_spans(spans: &[Span<'static>], width: usize) -> Vec<Span<'static>> {
    let chars = styled_chars(spans);
    let mut used = 0;
    let kept: Vec<(char, Style)> = chars
        .into_iter()
        .take_while(|&(c, _)| {
            used += width_of(c);
            used <= width
        })
        .collect();
    regroup(&kept)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn text(lines: &[Line<'_>]) -> Vec<String> {
        lines.iter().map(|l| plain(l)).collect()
    }

    #[test]
    fn a_heading_shows_bold_without_its_marks_and_a_link_without_its_address() {
        let lines = markdown(
            "## Why\n\nSee [the docs](https://x.y) and <https://a.b>.",
            80,
        );
        assert_eq!(text(&lines), ["Why", "", "See the docs and https://a.b."]);
        assert_eq!(lines[0].spans[0].style, bold(FG));
        let docs = lines[2].spans.iter().find(|s| s.content == "the docs");
        assert_eq!(docs.map(|s| s.style), Some(Look.link()));
    }

    #[test]
    fn a_link_in_bold_shows_without_its_address_too() {
        let lines = markdown("**See [the guide](https://x.y)** first.", 80);
        assert_eq!(text(&lines), ["See the guide first."]);
    }

    #[test]
    fn a_link_on_code_or_an_image_shows_without_its_address_too() {
        let lines = markdown(
            "See [`retry.ts`](https://x.y/retry), [![ci](https://i)](https://c).",
            80,
        );
        assert_eq!(text(&lines), ["See retry.ts, [img] ci."]);
    }

    #[test]
    fn a_table_in_a_list_or_a_quote_keeps_its_columns() {
        let md = "- item\n\n  | area | change |\n  |---|---|\n  | x | wide     cell |\n\n> | a | b |\n> |---|---|\n> | 1 | 2 |";
        let lines = text(&markdown(md, 40));
        assert!(
            lines.contains(&"  │ x    │ wide     cell │".to_owned()),
            "{lines:?}"
        );
        assert!(lines.contains(&"> │ 1 │ 2 │".to_owned()), "{lines:?}");
    }

    #[test]
    fn lists_show_bullets_and_tasks_and_hang_under_their_words() {
        let md = "- [x] backoff capped at eight seconds\n- [ ] tests\n- plain item\n1. first";
        let lines = markdown(md, 20);
        assert_eq!(
            text(&lines),
            [
                "✓ backoff capped at",
                "  eight seconds",
                "○ tests",
                "• plain item",
                // Another list, as GitHub sets it apart.
                "",
                "1. first"
            ]
        );
    }

    #[test]
    fn a_quote_carries_its_mark_down_each_line() {
        let lines = markdown("> one two three four five", 12);
        assert_eq!(text(&lines), ["> one two", "> three four", "> five"]);
    }

    #[test]
    fn html_tags_go_and_a_summary_says_it_folds() {
        let md =
            "<details>\n<summary>Coverage</summary>\n\nAll **92%**.\n\n</details>\n\nok<br>done";
        assert_eq!(
            text(&markdown(md, 40)),
            ["▸ Coverage", "", "All 92%.", "", "ok done"]
        );
        let md = "<div>\nif a < b and c > d, ok\n</div>";
        assert_eq!(text(&markdown(md, 40)), ["if a < b and c > d, ok"]);
    }

    #[test]
    fn code_shows_on_the_code_ground_and_inline_code_in_its_own_colour() {
        let lines = markdown("Run `make`:\n\n```\n\tmake all\n```", 20);
        assert_eq!(
            text(&lines),
            ["Run make:", "", &format!("{:<20}", "     make all")]
        );
        let make = lines[0].spans.iter().find(|s| s.content == "make");
        assert_eq!(make.map(|s| s.style), Some(inline_code()));
        assert_eq!(lines[2].spans[1].style, fg(BODY).bg(crate::theme::CODE));
    }

    #[test]
    fn only_a_diff_shows_its_lines_as_added_and_taken_out() {
        let lines = markdown("```yaml\n- name: x\n```\n\n```diff\n- old\n+ new\n```", 20);
        assert_eq!(lines.len(), 4, "{:?}", text(&lines));
        let colour = |i: usize| {
            lines[i]
                .spans
                .iter()
                .find_map(|s| s.style.fg.filter(|_| s.width() > 0))
        };
        assert_eq!(colour(0), Some(BODY));
        assert_eq!(colour(2), Some(RED));
        assert_eq!(colour(3), Some(GREEN));
        let quoted = markdown("> ```diff title=x\n> - old\n> ```", 20);
        assert_eq!(text(&quoted).len(), 1, "{:?}", text(&quoted));
        assert!(quoted[0].spans.iter().any(|s| s.style.fg == Some(RED)));
    }

    #[test]
    fn mentions_and_references_show_in_the_accent() {
        let lines = markdown("cc @bob, see #12. Not `@code`.", 80);
        let accented: Vec<String> = lines[0]
            .spans
            .iter()
            .filter(|s| s.style == fg(ACCENT))
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(accented, ["@bob", "#12"]);
        let quoted = markdown("> cc @bob", 80);
        assert!(
            quoted[0]
                .spans
                .iter()
                .any(|s| s.content == "@bob" && s.style == fg(ACCENT))
        );
    }

    #[test]
    fn a_table_fits_the_width_it_has() {
        let md = "| Area | What changed |\n|---|---|\n| upload | retries with a backoff that is capped |";
        let lines = markdown(md, 30);
        assert!(lines.iter().all(|l| l.width() <= 30), "{:?}", text(&lines));
        assert!(text(&lines)[0].starts_with('┌'));
    }

    #[test]
    fn the_first_words_are_a_threads_summary() {
        assert_eq!(
            first_words("\n**Nit**: rename [it](https://x)\n\nmore"),
            "Nit: rename it"
        );
        assert_eq!(first_words("```\nlet x = 1;\n```"), "let x = 1;");
        assert_eq!(
            first_words("---\n\n| a | b |\n|---|---|\n| 1 | 2 |"),
            "│ a │ b │"
        );
        assert_eq!(first_words("🚀"), "🚀");
        assert_eq!(first_words("> ```diff\n> - old\n> ```"), "> - old");
    }

    fn word() -> impl Strategy<Value = String> {
        prop_oneof![
            "[a-z]{1,12}",
            "[A-Z0-9]{1,30}",
            Just("**bold**".to_owned()),
            Just("`code`".to_owned()),
            Just("@bob".to_owned()),
            Just("日本語".to_owned()),
        ]
    }

    proptest! {
        /// However it wraps, no line is wider than the width, and every word's letters come
        /// out, in order.
        #[test]
        fn wrapping_keeps_each_letter_and_the_width(
            words in prop::collection::vec(word(), 0..40),
            (lead, kept) in prop_oneof![
                Just(("", "")),
                Just(("- ", "")),
                Just(("> ", "")),
                Just(("1. ", "1")),
                Just(("- [x] ", "")),
            ],
            width in 4usize..60,
        ) {
            let md = format!("{lead}{}", words.join(" "));
            let lines = markdown(&md, width);
            for line in &lines {
                prop_assert!(line.width() <= width, "{:?}", text(&lines));
            }
            let letters = |s: &str| -> String {
                s.chars().filter(|c| c.is_alphanumeric()).collect()
            };
            let said: String = lines.iter().map(|l| letters(&plain(l))).collect();
            let meant: String = std::iter::once(kept).chain(words.iter().map(String::as_str)).map(letters).collect();
            prop_assert_eq!(said, meant);
        }
    }
}
