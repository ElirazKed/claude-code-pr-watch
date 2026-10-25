//! The full view: one PR full size. Its title and tabs, and what each tab shows: the
//! conversation, the checks with the end of each failed job's log, the files with the threads
//! left on them. Like the rest of the view, it lays out what the model holds and decides
//! nothing.

use prw_core::address::{GhComment, GhThread, short_spot, tidy};
use prw_core::full::{Change, ChangedFile, Full, Item, Review, window};
use prw_core::gh::{CheckItem, ReviewState};
use prw_core::opened::{Log, Opened, Tab};
use prw_core::status::{Progress, check_name, meta_of, progress_of};
use prw_core::time::parse_iso;
use prw_core::ui::{Focus, Hit, Key, Model, ago, repo_of};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

mod markdown;

use markdown::{first_words, markdown};

use super::{
    Ctx, Hits, Marked, cells, check_look, clip, filled, fit, pill, row_of, span, spinner, usize_of,
    wrap,
};
use crate::theme::{ACCENT, BLUE, BODY, CODE, DIM, FG, GREEN, PEACH, RED, SEL, YELLOW, bold, fg};

/// The lines an item shows of a comment, a review or the description while another is
/// selected; the selected one shows whole.
const FOLDED: usize = 6;
/// The lines a thread shows of the code it is on.
const HUNK: usize = 4;
/// The lines a failed job's log shows: while another check is selected, and while it is.
const LOG_FOLDED: usize = 6;
const LOG_OPEN: usize = 20;

/// "3h ago", from a GitHub time; "" where it doesn't read.
fn age(at: &str, now: i64) -> String {
    parse_iso(at).map_or_else(String::new, |at| ago(now.saturating_sub(at)))
}

/// `text` with its tabs as four spaces: a terminal cell counts a tab as none.
fn detab(text: &str) -> String {
    text.replace('\t', "    ")
}

/// What someone wrote, as markdown `width` cells wide, indented `by`: all of it where
/// `is_whole`, else the first [`FOLDED`] lines and a line saying how many more.
fn body_lines(text: &str, by: usize, width: usize, is_whole: bool) -> Vec<Line<'static>> {
    let lines = markdown(text, width.saturating_sub(by));
    let pad = " ".repeat(by);
    let shown = if is_whole { lines.len() } else { FOLDED };
    let more = lines.len().saturating_sub(shown);
    let mut out: Vec<Line<'static>> = lines
        .into_iter()
        .take(shown)
        .map(|l| Line::from([vec![Span::raw(pad.clone())], l.spans].concat()))
        .collect();
    if more > 0 {
        let words = format!(
            "{pad}… {more} more line{}",
            if more == 1 { "" } else { "s" }
        );
        out.push(Line::from(span(words, fg(DIM))));
    }
    out
}

/// Who said it, bold: in the accent where it is one of gh's logins.
fn who(login: Option<&str>, logins: &[&str]) -> Span<'static> {
    let name = login.filter(|l| !l.is_empty()).unwrap_or("ghost");
    let colour = if logins.contains(&name) { ACCENT } else { FG };
    span(format!("@{name}"), bold(colour))
}

/// A line of a diff on the code ground, `by` cells in: `+` green, `-` red, the rest dim.
fn code_line(text: &str, by: usize, width: usize) -> Line<'static> {
    let colour = match text.chars().next() {
        Some('+') => GREEN,
        Some('-') => RED,
        _ => DIM,
    };
    code_in(text, by, width, colour)
}

/// A line of code on the code ground, `by` cells in, in `colour`.
fn code_in(text: &str, by: usize, width: usize, colour: Color) -> Line<'static> {
    let room = width.saturating_sub(by);
    Line::from(vec![
        Span::raw(" ".repeat(by)),
        span(fit(&format!(" {}", detab(text)), room), fg(colour).bg(CODE)),
    ])
}

/// The end of a thread's code: its first comment's hunk, less its `@@` line.
fn hunk_lines(thread: &GhThread, by: usize, width: usize) -> Vec<Line<'static>> {
    let hunk = thread
        .comments
        .nodes
        .first()
        .map_or("", |c| c.diff_hunk.as_str());
    let lines: Vec<&str> = hunk.lines().filter(|l| !l.starts_with("@@")).collect();
    lines[lines.len().saturating_sub(HUNK)..]
        .iter()
        .map(|l| code_line(l, by, width))
        .collect()
}

// ---- The title and the tabs ----------------------------------------------------------

/// The PR's two rows: back, its repo, number and title; its pill, branch and author, and
/// how fresh the read is.
pub(super) fn title(
    frame: &mut Frame,
    r: Rect,
    model: &Model,
    opened: &Opened,
    ctx: Ctx,
    hits: &mut Hits,
) {
    let width = usize_of(r.width);
    let row = model.current_row();
    let gh = model.entry(opened.url()).and_then(|e| e.pr.as_ref());
    let name = row
        .map(|r| r.pr.title.as_str())
        .filter(|t| !t.is_empty())
        .or_else(|| gh.map(|pr| pr.title.as_str()))
        .unwrap_or_default();
    let mut top = Marked::default();
    top.push(Span::raw(" "));
    top.hit([span("‹ back", fg(ACCENT))], Hit::Key(Key::Esc));
    top.push(span(
        format!("  {} #{}  ", repo_of(opened.url()), opened.number()),
        fg(DIM),
    ));
    let used = top.width();
    top.push(span(clip(name, width.saturating_sub(used + 1)), bold(FG)));

    // How fresh, on the right: a read under way, the last one's age, or why a read since
    // failed (with nothing read, the body says why).
    let mut fresh = Vec::new();
    if let Some((why, at)) = opened.failed().filter(|_| opened.read_at().is_some()) {
        let words = format!(
            "⚠ read failed {}: {why}",
            ago(model.now().saturating_sub(at))
        );
        fresh.push(span(clip(&words, width / 2), fg(RED)));
    } else if let Some(at) = opened.read_at() {
        if opened.is_reading() {
            fresh.push(spinner(ctx.beat, 0, DIM));
            fresh.push(Span::raw(" "));
        }
        let words = format!("read {}", ago(model.now().saturating_sub(at)));
        fresh.push(span(words, fg(DIM)));
    }
    let fresh_w: usize = fresh.iter().map(Span::width).sum();
    let mut second = vec![Span::raw(" ")];
    if let Some(row) = row {
        second.push(pill(row.pr.derived.pill));
        second.push(Span::raw("  "));
    }
    if let Some(pr) = gh {
        let meta = meta_of(pr);
        let by = if meta.author.is_empty() {
            String::new()
        } else {
            format!(" · by @{}", meta.author)
        };
        let words = format!("{} → {}{by}", meta.branch, meta.base);
        let used: usize = second.iter().map(Span::width).sum();
        let room = width.saturating_sub(used + fresh_w + 2);
        second.push(span(clip(&words, room), fg(DIM)));
    }
    let used: usize = second.iter().map(Span::width).sum();
    second.push(Span::raw(
        " ".repeat(width.saturating_sub(used + fresh_w + 1)),
    ));
    second.extend(fresh);
    if let Some(line) = row_of(r, 0) {
        hits.spots(line, top.spots.clone());
    }
    frame.render_widget(
        Paragraph::new(vec![Line::from(top.spans), Line::from(second)]),
        r,
    );
}

/// How many items a tab lists, as its label says it, once a read has come.
fn count_of(opened: &Opened, full: &Full, tab: Tab) -> usize {
    match tab {
        // The description is no item of the count.
        Tab::Conversation => opened.items().len().saturating_sub(1),
        Tab::Checks => opened.len(tab),
        Tab::Files => full.file_total,
    }
}

/// The tabs, the one showing marked under it; where it is in its tab, on the right.
pub(super) fn tabs(frame: &mut Frame, r: Rect, opened: &Opened, hits: &mut Hits) {
    let width = usize_of(r.width);
    let mut line = Marked::default();
    let mut marks = vec![Span::raw(" ")];
    line.push(Span::raw(" "));
    for tab in Tab::ALL {
        let label = match opened.full() {
            Some(full) => format!(
                " {} {} {} ",
                tab.key(),
                tab.name(),
                count_of(opened, full, tab)
            ),
            None => format!(" {} {} ", tab.key(), tab.name()),
        };
        let is_on = tab == opened.tab();
        let style = if is_on { bold(FG).bg(SEL) } else { fg(DIM) };
        let mark = if is_on { "▔" } else { " " };
        marks.push(span(mark.repeat(cells(&label)), fg(ACCENT)));
        marks.push(Span::raw(" "));
        line.hit([span(label, style)], Hit::Tab(tab));
        line.push(Span::raw(" "));
    }
    let len = opened.len(opened.tab());
    if opened.full().is_some() && len > 1 {
        let at = format!("{} of {len} ", opened.selected(opened.tab()) + 1);
        let gap = width.saturating_sub(line.width() + cells(&at));
        if gap > 0 {
            line.push(Span::raw(" ".repeat(gap)));
            line.push(span(at, fg(DIM)));
        }
    }
    if let Some(row) = row_of(r, 0) {
        hits.spots(row, line.spots.clone());
    }
    frame.render_widget(
        Paragraph::new(vec![Line::from(line.spans), Line::from(marks)]),
        r,
    );
}

// ---- The body ------------------------------------------------------------------------

/// What the tab showing lists, an item of lines each, and the one selected: the items drawn
/// from where the window starts, the selected one marked in the gutter.
pub(super) fn body(
    frame: &mut Frame,
    r: Rect,
    model: &Model,
    opened: &Opened,
    ctx: Ctx,
    hits: &mut Hits,
) {
    hits.add(r, Hit::Pane(Focus::Prs));
    let width = usize_of(r.width).saturating_sub(3);
    let Some(full) = opened.full() else {
        let line = match opened.failed() {
            Some((why, _)) => vec![span(
                clip(
                    &format!("⚠ Couldn't read #{}: {why}", opened.number()),
                    width,
                ),
                fg(RED),
            )],
            None => vec![
                spinner(ctx.beat, 0, YELLOW),
                span(format!(" Reading #{}…", opened.number()), fg(DIM)),
            ],
        };
        let mut lines = vec![
            Line::raw(""),
            Line::from([vec![Span::raw("  ")], line].concat()),
        ];
        if opened.failed().is_some() {
            lines.push(Line::from(span("  It tries again every 30 s.", fg(DIM))));
        }
        frame.render_widget(Paragraph::new(lines), r);
        return;
    };
    let tab = opened.tab();
    let selected = opened.selected(tab);
    let now = model.now();
    let logins: Vec<&str> = model.poller().map_or_else(Vec::new, |p| {
        std::iter::once(p.logins.active.as_str())
            .chain(p.logins.others.iter().map(String::as_str))
            .collect()
    });
    let mut items: Vec<Vec<Line<'static>>> = match tab {
        Tab::Conversation => opened
            .items()
            .iter()
            .enumerate()
            .map(|(i, &item)| said(item, full, i == selected, width, now, &logins))
            .collect(),
        Tab::Checks => opened
            .checks()
            .iter()
            .enumerate()
            .map(|(i, check)| check_item(check, opened, i == selected, width, now))
            .collect(),
        Tab::Files => full
            .files
            .iter()
            .enumerate()
            .map(|(i, file)| {
                let threads = opened.threads_on(&file.path);
                file_item(file, &threads, i == selected, width)
            })
            .collect(),
    };
    left_out(&mut items, full, tab);
    if items.is_empty() {
        let words = match tab {
            Tab::Checks => "No checks have reported on the head commit.",
            _ => "No files changed.",
        };
        let lines = vec![
            Line::raw(""),
            Line::from(span(format!("  {words}"), fg(DIM))),
        ];
        frame.render_widget(Paragraph::new(lines), r);
        return;
    }
    let room = usize_of(r.height);
    // Each item and the blank line after it; the last one's may fall off the bottom.
    let heights: Vec<usize> = items.iter().map(|lines| lines.len() + 1).collect();
    let top = window(&heights, selected, room + 1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, item) in items.into_iter().enumerate().skip(top) {
        if lines.len() >= room {
            break;
        }
        let gutter = if i == selected { "▌" } else { " " };
        for line in item {
            let Some(at) = row_of(r, lines.len()) else {
                break;
            };
            hits.add(at, Hit::Item(i));
            let mut spans = vec![span(gutter, fg(ACCENT)), Span::raw(" ")];
            spans.extend(line.spans);
            lines.push(Line::from(spans).style(line.style));
        }
        lines.push(Line::raw(""));
    }
    frame.render_widget(Paragraph::new(lines), r);
}

/// What the read left out, said under the item next to it: the oldest of the conversation,
/// the last of the files.
fn left_out(items: &mut [Vec<Line<'static>>], full: &Full, tab: Tab) {
    let note = |left: &[(usize, &str)], whose: &str| {
        let parts: Vec<String> = left
            .iter()
            .filter(|&&(n, _)| n > 0)
            .map(|&(n, what)| format!("{n} {whose} {what}{}", if n == 1 { "" } else { "s" }))
            .collect();
        (!parts.is_empty())
            .then(|| Line::from(span(format!("… {} on GitHub", parts.join(", ")), fg(DIM))))
    };
    match tab {
        Tab::Conversation => {
            let left = [
                (
                    full.comment_total.saturating_sub(full.comments.len()),
                    "comment",
                ),
                (
                    full.review_total.saturating_sub(full.reviews.len()),
                    "review",
                ),
                (
                    full.thread_total.saturating_sub(full.threads.len()),
                    "thread",
                ),
            ];
            if let (Some(line), Some(first)) = (note(&left, "older"), items.first_mut()) {
                first.push(line);
            }
        }
        Tab::Files => {
            let left = [(full.file_total.saturating_sub(full.files.len()), "file")];
            if let (Some(line), Some(last)) = (note(&left, "more"), items.last_mut()) {
                last.push(line);
            }
        }
        Tab::Checks => {}
    }
}

/// One item of the conversation.
fn said(
    item: Item,
    full: &Full,
    is_sel: bool,
    width: usize,
    now: i64,
    logins: &[&str],
) -> Vec<Line<'static>> {
    match item {
        Item::Description => {
            let author = full.author.as_ref().map(|a| a.login.as_str());
            let head = Line::from(vec![
                who(author, logins),
                span(
                    format!(" opened this · {}", age(&full.created_at, now)),
                    fg(DIM),
                ),
            ]);
            let text = tidy(&full.body);
            let lines = if text.is_empty() {
                vec![Line::from(span("No description.", fg(BODY)))]
            } else {
                body_lines(&text, 0, width, is_sel)
            };
            [vec![head], lines].concat()
        }
        Item::Comment(i) => {
            comment_lines(&full.comments[i], "◦", DIM, 0, is_sel, width, now, logins)
        }
        Item::Review(i) => review_lines(&full.reviews[i], is_sel, width, now, logins),
        Item::Thread(i) => thread_lines(&full.threads[i], is_sel, width, now, logins),
    }
}

/// A comment: who and when, then what it says, `by` cells in.
#[expect(
    clippy::too_many_arguments,
    reason = "a comment's look, from each place it shows"
)]
fn comment_lines(
    comment: &GhComment,
    glyph: &str,
    colour: Color,
    by: usize,
    is_sel: bool,
    width: usize,
    now: i64,
    logins: &[&str],
) -> Vec<Line<'static>> {
    let author = comment.author.as_ref().map(|a| a.login.as_str());
    let head = Line::from(vec![
        Span::raw(" ".repeat(by)),
        span(format!("{glyph} "), fg(colour)),
        who(author, logins),
        span(format!(" · {}", age(&comment.created_at, now)), fg(DIM)),
    ]);
    let lines = body_lines(&tidy(&comment.body), by + 2, width, is_sel);
    [vec![head], lines].concat()
}

/// A review: its verdict, then what it says.
fn review_lines(
    review: &Review,
    is_sel: bool,
    width: usize,
    now: i64,
    logins: &[&str],
) -> Vec<Line<'static>> {
    let (glyph, colour, words) = match review.state {
        ReviewState::Approved => ("✓", GREEN, "approved"),
        ReviewState::ChangesRequested => ("●", ACCENT, "requested changes"),
        ReviewState::Dismissed => ("·", DIM, "review dismissed"),
        _ => ("◦", BLUE, "reviewed"),
    };
    let author = review.author.as_ref().map(|a| a.login.as_str());
    let head = Line::from(vec![
        span(format!("{glyph} "), fg(colour)),
        who(author, logins),
        span(format!(" {words}"), fg(colour)),
        span(format!(" · {}", age(&review.submitted_at, now)), fg(DIM)),
    ]);
    let lines = body_lines(&tidy(&review.body), 2, width, is_sel);
    [vec![head], lines].concat()
}

/// A thread: where it is and whether it is open; the code it is on and its comments. A
/// resolved one, unless selected, in one line.
fn thread_lines(
    thread: &GhThread,
    is_sel: bool,
    width: usize,
    now: i64,
    logins: &[&str],
) -> Vec<Line<'static>> {
    let (glyph, colour, tag) = if thread.is_resolved {
        ("✓", DIM, "resolved")
    } else if thread.is_outdated {
        ("●", PEACH, "outdated")
    } else {
        ("●", YELLOW, "open")
    };
    let place = clip(&short_spot(thread), width.saturating_sub(cells(tag) + 3));
    let gap = width.saturating_sub(2 + cells(&place) + cells(tag));
    let style = if thread.is_resolved {
        fg(DIM)
    } else {
        bold(FG)
    };
    let head = Line::from(vec![
        span(format!("{glyph} "), fg(colour)),
        span(place, style),
        Span::raw(" ".repeat(gap)),
        span(tag, fg(colour)),
    ]);
    let comments = &thread.comments.nodes;
    if thread.is_resolved && !is_sel {
        let last = comments.last().map_or_else(String::new, |c| {
            let author = c.author.as_ref().map_or("ghost", |a| a.login.as_str());
            let text = first_words(&tidy(&c.body));
            format!(" · @{author}: {text}")
        });
        let n = thread.comments.total_count.unwrap_or(comments.len());
        let words = format!("  {n} comment{}{last}", if n == 1 { "" } else { "s" });
        return vec![head, Line::from(span(clip(&words, width), fg(DIM)))];
    }
    let mut lines = vec![head];
    lines.extend(hunk_lines(thread, 2, width));
    for comment in comments {
        lines.extend(comment_lines(
            comment, "", DIM, 2, is_sel, width, now, logins,
        ));
    }
    let more = thread
        .comments
        .total_count
        .unwrap_or(0)
        .saturating_sub(comments.len());
    if more > 0 {
        lines.push(Line::from(span(
            format!("    + {more} more on GitHub"),
            fg(DIM),
        )));
    }
    lines
}

/// How long a run took, or has run: "42s", "1m09s", "1h05m".
fn took(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// Whether a log line says what went wrong.
fn is_error_line(line: &str) -> bool {
    let line = line.to_lowercase();
    ["error", "fail", "panicked", "exception", "✖", "✗"]
        .iter()
        .any(|word| line.contains(word))
}

/// One check: what it is and how it went; a failed one's log end, or what it says of itself.
fn check_item(
    check: &CheckItem,
    opened: &Opened,
    is_sel: bool,
    width: usize,
    now: i64,
) -> Vec<Line<'static>> {
    let progress = progress_of(check);
    let (glyph, colour, word) = check_look(progress);
    let name = check_name(check);
    let label = match &check.workflow {
        Some(workflow) if *workflow != name => format!("{workflow} / {name}"),
        _ => name,
    };
    let started = check
        .started_at
        .as_deref()
        .or(check.created_at.as_deref())
        .and_then(parse_iso);
    let ended = check.completed_at.as_deref().and_then(parse_iso);
    let when = match (progress, started, ended) {
        (Progress::Running, Some(from), _) => format!("running {}", took(now - from)),
        (Progress::Passed | Progress::Failed, Some(from), Some(to)) => took(to - from),
        _ => word.to_owned(),
    };
    let room = width.saturating_sub(2 + cells(&when) + 1);
    let name_style = if progress == Progress::Failed {
        fg(FG)
    } else {
        fg(BODY)
    };
    let spans = vec![
        span(format!("{glyph} "), fg(colour)),
        span(fit(&label, room), name_style),
        span(
            format!(" {when}"),
            fg(if progress == Progress::Passed {
                DIM
            } else {
                colour
            }),
        ),
    ];
    let first = if is_sel {
        filled(spans, width, SEL)
    } else {
        Line::from(spans)
    };
    let mut lines = vec![first];
    if progress == Progress::Failed {
        lines.extend(failed_lines(check, opened, is_sel, width));
    }
    lines
}

/// What a failed check says under it: its job's log end, or what it says of itself.
fn failed_lines(
    check: &CheckItem,
    opened: &Opened,
    is_sel: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let job = check.database_id.filter(|_| check.workflow.is_some());
    match job.and_then(|job| opened.log(job)) {
        Some(Log::Read(log)) => {
            let all: Vec<&str> = log.text.lines().collect();
            let n = if is_sel { LOG_OPEN } else { LOG_FOLDED };
            let shown = &all[all.len().saturating_sub(n)..];
            for line in shown {
                // The runner's annotations, as the run's page shows them: without the marker.
                let error = line.strip_prefix("##[error]");
                let line = error
                    .or_else(|| line.strip_prefix("##[warning]"))
                    .or_else(|| line.strip_prefix("##[notice]"))
                    .unwrap_or(line);
                let line = detab(line);
                let colour = if error.is_some() || is_error_line(&line) {
                    RED
                } else {
                    BODY
                };
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    span(
                        fit(&format!(" {line}"), width.saturating_sub(2)),
                        fg(colour).bg(CODE),
                    ),
                ]));
            }
            let words = format!(
                "  last {} of {} lines · f copies them for an agent",
                shown.len(),
                log.total
            );
            lines.push(Line::from(span(clip(&words, width), fg(DIM))));
        }
        Some(Log::Reading) => lines.push(Line::from(span("  reading the log…", fg(DIM)))),
        Some(Log::Failed(why)) => {
            lines.push(Line::from(span(
                clip(&format!("  no log: {why}"), width),
                fg(DIM),
            )));
        }
        None => {
            let about = [&check.title, &check.summary, &check.description]
                .into_iter()
                .flatten()
                .filter(|t| !t.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" · ");
            for l in wrap(&about.replace('\n', " "), width.saturating_sub(2), 2) {
                lines.push(Line::from(span(format!("  {l}"), fg(BODY))));
            }
            if let Some(url) = check.details_url.as_ref().or(check.target_url.as_ref()) {
                lines.push(Line::from(span(clip(&format!("  {url}"), width), fg(DIM))));
            }
        }
    }
    lines
}

/// One changed file: what was done to it and its counts; the selected one with the threads
/// left on it.
fn file_item(
    file: &ChangedFile,
    threads: &[&GhThread],
    is_sel: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let (glyph, colour) = match file.change_type {
        Change::Added => ("A", GREEN),
        Change::Deleted => ("D", RED),
        Change::Renamed | Change::Copied => ("R", BLUE),
        _ => ("M", YELLOW),
    };
    let adds = format!("+{}", file.additions);
    let dels = format!(" −{}", file.deletions);
    let open = threads.iter().filter(|t| !t.is_resolved).count();
    let mark = if open > 0 {
        format!("● {open}  ")
    } else {
        String::new()
    };
    let room = width.saturating_sub(2 + cells(&mark) + cells(&adds) + cells(&dels) + 1);
    let spans = vec![
        span(format!("{glyph} "), fg(colour)),
        span(fit(&file.path, room), fg(FG)),
        Span::raw(" "),
        span(mark, fg(YELLOW)),
        span(adds, fg(GREEN)),
        span(dels, fg(RED)),
    ];
    if !is_sel {
        return vec![Line::from(spans)];
    }
    let mut lines = vec![filled(spans, width, SEL)];
    if threads.is_empty() {
        lines.push(Line::from(span(
            "  No review threads on this file.",
            fg(DIM),
        )));
    }
    for thread in threads {
        let (glyph, colour) = if thread.is_resolved {
            ("✓", DIM)
        } else {
            ("●", YELLOW)
        };
        let first = thread.comments.nodes.first();
        let author = first
            .and_then(|c| c.author.as_ref())
            .map_or("ghost", |a| a.login.as_str());
        let text = first
            .map(|c| first_words(&tidy(&c.body)))
            .unwrap_or_default();
        let place = thread
            .line
            .or(thread.original_line)
            .map_or_else(|| "file".to_owned(), |line| format!("line {line}"));
        let words = format!("{place} · @{author}: {text}");
        lines.push(Line::from(vec![
            span(format!("  {glyph} "), fg(colour)),
            span(clip(&words, width.saturating_sub(4)), fg(BODY)),
        ]));
        lines.extend(hunk_lines(thread, 4, width));
    }
    lines
}
