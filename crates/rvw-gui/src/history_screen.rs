//! The History screen: the search and filters on the left, the commit list
//! with its graph (or one file of the selected commit) in the centre, and the
//! selected commit's details on the right. Every fact comes from
//! `HistoryState`; this file only lays it out.

use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::prelude::*;
use gpui::{
    canvas, div, point, px, uniform_list, AnyElement, Bounds, ClickEvent, Context, Div, FontWeight,
    PathBuilder, Pixels, Point, Stateful, Window,
};
use rvw_git::blame::BlameLine;
use rvw_git::history::{CommitDetail, FileChange};
use rvw_git::search::Field;
use rvw_view::diff::LineKind;
use rvw_view::graph::{Row as GraphRow, Seg};
use rvw_view::history::{ago, short, HistoryRow, Label, LabelKind, MAX_LANES};
use rvw_view::selection::Pos;

use crate::code_scroll;
use crate::history_files::{is_uncommitted, starts_run};
use crate::history_state::{BlameView, CompareView, DiffRow, HistoryState, OpenCommitFile, Source};
use crate::review_screen::LINE_HEIGHT;
use crate::shell::Shell;
use crate::theme;

/// A commit row's height.
const ROW_HEIGHT: Pixels = px(42.);
/// The column header, the hunk header and the loading line.
const HEADER_ROW_HEIGHT: Pixels = px(30.);
/// The width of one graph lane.
const LANE_WIDTH: f32 = 14.;
/// The diameter of a commit's dot.
const DOT: Pixels = px(9.);
const SHA_WIDTH: Pixels = px(58.);
/// The commit list's fixed columns. The subject takes what is left, so every
/// other cell sits at the same x on every row and under its header.
const AUTHOR_WIDTH: Pixels = px(120.);
const AGO_WIDTH: Pixels = px(40.);
const REVIEW_WIDTH: Pixels = px(56.);
/// The gap between the list's cells and the padding of a row.
const COLUMN_GAP: f32 = 12.;
/// The line-number columns and the sign of a diff line; with the 6px before
/// the code they make `HISTORY_CODE_GUTTER`.
const GUTTER: Pixels = px(46.);
const SIGN_WIDTH: Pixels = px(16.);
/// The blame's author and age columns, and the gap after each column; the
/// short id is `SHA_WIDTH`. With the line number and the 6px before the code
/// they make `history_state::BLAME_CODE_GUTTER`.
const BLAME_AUTHOR: Pixels = px(110.);
const BLAME_AGE: Pixels = px(36.);
const BLAME_GAP: Pixels = px(8.);
/// The size of the small text: shas, labels, hints and the file list.
const SMALL: f32 = 12.;

/// The search field, what it matches, and the filters.
pub fn left(state: &HistoryState, cx: &Context<Shell>) -> Div {
    let mut column = div()
        .flex()
        .flex_col()
        .h_full()
        .px(theme::PANEL_PADDING)
        .pt(px(20.))
        .gap(px(22.))
        .child(search_box(state))
        .child(matches_section(state, cx))
        .child(branches_section(state, cx));
    if let Source::File { path } = &state.source {
        column = column.child(file_section(path, cx));
    }
    column.child(placeholders())
}

/// The search field in its box, and the hint under it.
fn search_box(state: &HistoryState) -> Div {
    let mut column = div().flex().flex_col().gap(px(6.));
    if let Some(input) = state.search.as_ref() {
        column = column.child(
            div()
                .flex()
                .items_center()
                .h(px(32.))
                .px(px(10.))
                .rounded(theme::CONTROL_RADIUS)
                .border_1()
                .border_color(theme::hairline_control())
                .bg(theme::paper())
                .child(div().w_full().child(input.clone())),
        );
    }
    column.child(
        div()
            .text_size(px(SMALL))
            .text_color(theme::faint())
            .child("Enter to search, Esc to clear, / to focus"),
    )
}

/// A muted heading over a group of rows.
fn heading(text: &'static str) -> Div {
    div().pb(px(6.)).text_color(theme::muted()).child(text)
}

/// One row of a choice list: 30px high, with the selected one on the panel
/// colour in medium ink. The caller adds the click.
fn choice(id: &'static str, label: &str, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(30.))
        .px(px(8.))
        .rounded(theme::ROW_RADIUS)
        .truncate()
        .text_color(theme::text())
        .when(selected, |d| {
            d.bg(theme::selected_panel())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .cursor_pointer()
        .child(label.to_owned())
}

/// What the search matches: message, author or content.
fn matches_section(state: &HistoryState, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .flex_col()
        .child(heading("Matches"))
        .child(field_choice(
            state,
            Field::Message,
            "history-field-message",
            "Message",
            cx,
        ))
        .child(field_choice(
            state,
            Field::Author,
            "history-field-author",
            "Author",
            cx,
        ))
        .child(field_choice(
            state,
            Field::Content,
            "history-field-content",
            "Content (added or removed text)",
            cx,
        ))
}

/// One field the search matches; a click searches it again.
fn field_choice(
    state: &HistoryState,
    field: Field,
    id: &'static str,
    label: &str,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    choice(id, label, state.field == field).on_click(cx.listener(
        move |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
            this.set_search_field(field, cx);
            this.focus_shell(window);
        },
    ))
}

/// This branch, or every branch, and a click that switches between them.
fn branches_section(state: &HistoryState, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .flex_col()
        .child(heading("Branches"))
        .child(all_choice(
            state.all,
            false,
            "history-this-branch",
            "This branch",
            cx,
        ))
        .child(all_choice(
            state.all,
            true,
            "history-all-branches",
            "All branches",
            cx,
        ))
}

/// One of the two branch choices; `all` is what a click sets.
fn all_choice(
    current: bool,
    all: bool,
    id: &'static str,
    label: &str,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    choice(id, label, current == all).on_click(cx.listener(
        move |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
            this.set_history_all(all, cx);
            this.focus_shell(window);
        },
    ))
}

/// The file the list is about, and a link back to the whole log.
fn file_section(path: &str, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .flex_col()
        .child(heading("File"))
        .child(
            div()
                .px(px(8.))
                .pb(px(6.))
                .font_family(theme::code_font())
                .text_size(px(SMALL))
                .text_color(theme::ink())
                .truncate()
                .child(path.to_owned()),
        )
        .child(blame_link(path, cx))
        .child(show_all_link(cx))
}

/// "Blame this file": the file on disk, blamed in the centre.
fn blame_link(path: &str, cx: &Context<Shell>) -> Stateful<Div> {
    let path = path.to_owned();
    div()
        .id("history-blame-file")
        .flex()
        .items_center()
        .h(px(30.))
        .px(px(8.))
        .rounded(theme::ROW_RADIUS)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::ink())
        .cursor_pointer()
        .child("Blame this file")
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.blame_file(&path, cx);
                this.focus_shell(window);
            },
        ))
}

/// "Show all commits": the log again, with the branch choice kept.
fn show_all_link(cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id("history-show-all")
        .flex()
        .items_center()
        .h(px(30.))
        .px(px(8.))
        .rounded(theme::ROW_RADIUS)
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::ink())
        .cursor_pointer()
        .child("Show all commits")
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                let all = this.history.all;
                this.show_history_source(Source::Log { all }, cx);
                this.focus_shell(window);
            },
        ))
}

/// The filters that need data the app does not have yet. They stay in place,
/// dim, and say what unlocks them.
fn placeholders() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(22.))
        .child(placeholder(
            "Made by",
            "Humans and agents",
            "Needs agent session reports (M5)",
        ))
        .child(placeholder("Review", "Any", "Needs the review ledger (M6)"))
}

/// A dim filter group: its heading, its one value, and why it is not available.
fn placeholder(title: &'static str, value: &'static str, why: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .text_color(theme::faint())
        .child(div().pb(px(6.)).child(title))
        .child(
            div()
                .h(px(30.))
                .px(px(8.))
                .flex()
                .items_center()
                .child(value),
        )
        .child(div().px(px(8.)).text_size(px(SMALL)).child(why))
}

/// The commit list, the blame, or the open file of the selected commit. An
/// open file is shown over the blame.
pub fn centre(state: &HistoryState, cx: &Context<Shell>) -> Stateful<Div> {
    let root = div()
        .id("history-centre")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .bg(theme::paper());
    match (&state.open, &state.blame) {
        (Some(Ok(open)), _) => root
            .child(file_header(open, cx))
            .child(diff_body(state, open, cx)),
        (Some(Err(message)), _) => root.child(back_header(cx)).child(error_text(message)),
        (None, Some(Ok(blame))) => root
            .child(blame_header(&blame.path, cx))
            .child(blame_body(state, blame, cx)),
        (None, Some(Err(message))) => root
            .child(blame_header(file_path(state), cx))
            .child(error_text(message)),
        (None, None) => root.child(commit_list(state, cx)),
    }
}

/// The path the list is about, when it is a file's history.
fn file_path(state: &HistoryState) -> &str {
    match &state.source {
        Source::File { path } => path.as_str(),
        _ => "",
    }
}

/// The path of the open file, when one is open.
fn open_path(state: &HistoryState) -> Option<&str> {
    match &state.open {
        Some(Ok(open)) => Some(open.path.as_str()),
        _ => None,
    }
}

/// The blame's 48px header: the way back to the commits, the word "Blame",
/// and the file's path.
fn blame_header(path: &str, cx: &Context<Shell>) -> Div {
    header_frame()
        .child(blame_back_link(cx))
        .child(div().flex_none().text_color(theme::muted()).child("Blame"))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme::ink())
                .child(path.to_owned()),
        )
}

/// "← Commits" in the blame's header: drops the blame.
fn blame_back_link(cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id("history-blame-back")
        .flex_none()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::ink())
        .cursor_pointer()
        .child("\u{2190} Commits")
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.close_blame(cx);
                this.focus_shell(window);
            },
        ))
}

/// The blame's lines in a uniform list, moved sideways with the code of the
/// diff. Its wheel turns move the code, not the list.
fn blame_body(state: &HistoryState, blame: &BlameView, cx: &Context<Shell>) -> AnyElement {
    if blame.lines.is_empty() {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::muted())
            .child("The file is empty.")
            .into_any_element();
    }
    let mut list = uniform_list("history-blame", blame.lines.len(), cx.processor(blame_rows))
        .track_scroll(state.blame_scroll.clone())
        .flex_1()
        .min_h(px(0.))
        .on_scroll_wheel(cx.listener(|this, event, _, cx| this.scroll_history_code(event, cx)));
    list.style().restrict_scroll_to_axis = Some(true);
    list.into_any_element()
}

/// The blame rows in view, built from the shell's current state. The row of
/// the selected commit is highlighted.
fn blame_rows(
    this: &mut Shell,
    range: Range<usize>,
    _: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let state = &this.history;
    let Some(Ok(blame)) = state.blame.as_ref() else {
        return Vec::new();
    };
    let Some(lines) = blame.lines.get(range.clone()) else {
        return Vec::new();
    };
    let now = now_secs();
    let selected = state
        .history
        .selected_row()
        .map(|row| row.entry.id.as_str());
    let scroll_x = state.code_scroll_x;
    lines
        .iter()
        .zip(range)
        .map(|(line, index)| {
            let is_selected = selected == Some(line.id.as_str());
            blame_row(
                line,
                starts_run(&blame.lines, index),
                is_selected,
                now,
                scroll_x,
                cx,
            )
        })
        .collect()
}

/// One blamed line: the commit's id, author and age on the first line of its
/// run, then the line number and the code. A click selects the line's commit.
fn blame_row(
    line: &BlameLine,
    first: bool,
    selected: bool,
    now: i64,
    scroll_x: Pixels,
    cx: &Context<Shell>,
) -> AnyElement {
    let committed = !is_uncommitted(&line.id);
    let id = line.id.clone();
    div()
        .id(("history-blame-row", line.line))
        .flex()
        .items_center()
        .gap(BLAME_GAP)
        .h(LINE_HEIGHT)
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text())
        .when(selected, |d| d.bg(theme::selected_line()))
        .when(committed, gpui::Styled::cursor_pointer)
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.select_blame_commit(&id, cx);
                this.focus_shell(window);
            },
        ))
        .child(blame_id(line, first))
        .child(blame_author(line, first))
        .child(blame_age(line, first, now))
        .child(gutter(u32::try_from(line.line).ok()))
        .child(code_text(&line.text, scroll_x))
        .into_any_element()
}

/// The short id of a run's first line in faint code, or "uncommitted" in
/// muted text for a line git has not committed. Blank on the other lines.
fn blame_id(line: &BlameLine, first: bool) -> Div {
    let cell = div()
        .w(SHA_WIDTH)
        .flex_none()
        .truncate()
        .text_size(px(SMALL));
    if !first {
        cell
    } else if is_uncommitted(&line.id) {
        cell.text_color(theme::muted()).child("uncommitted")
    } else {
        cell.font_family(theme::code_font())
            .text_color(theme::faint())
            .child(short(&line.id).to_owned())
    }
}

/// The author of a run's first line in muted text; blank on the other lines.
fn blame_author(line: &BlameLine, first: bool) -> Div {
    let author = if first {
        line.author.clone()
    } else {
        String::new()
    };
    div()
        .w(BLAME_AUTHOR)
        .flex_none()
        .truncate()
        .text_color(theme::muted())
        .child(author)
}

/// The age of a run's first line, right aligned; blank on the other lines.
fn blame_age(line: &BlameLine, first: bool, now: i64) -> Div {
    let age = if first {
        ago(now, line.time)
    } else {
        String::new()
    };
    div()
        .w(BLAME_AGE)
        .flex_none()
        .text_right()
        .text_color(theme::faint())
        .child(age)
}

/// The 48px header of the centre, in the code font with a hairline below.
fn header_frame() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(12.))
        .h(px(48.))
        .px(px(24.))
        .border_b_1()
        .border_color(theme::hairline())
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
}

/// The header with only the way back, for a file that could not be read.
fn back_header(cx: &Context<Shell>) -> Div {
    header_frame().child(back_link(cx))
}

/// The open file's header: the way back, the commit, the path and the counts.
fn file_header(open: &OpenCommitFile, cx: &Context<Shell>) -> Div {
    let header = header_frame()
        .child(back_link(cx))
        .child(
            div()
                .flex_none()
                .text_color(theme::muted())
                .child(short(&open.commit).to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme::ink())
                .child(open.path.clone()),
        );
    // A whole-file view counts no changes, so it shows no counts.
    if open.diff.added + open.diff.removed == 0 {
        return header;
    }
    header
        .child(
            div()
                .text_color(theme::done())
                .child(format!("+{}", open.diff.added)),
        )
        .child(
            div()
                .text_color(theme::rejected())
                .child(format!("\u{2212}{}", open.diff.removed)),
        )
}

/// "← Commits": closes the open file and shows the list again.
fn back_link(cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id("history-back")
        .flex_none()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::ink())
        .cursor_pointer()
        .child("\u{2190} Commits")
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.close_commit_file(cx);
                this.focus_shell(window);
            },
        ))
}

/// A line of error text in the rejected colour.
fn error_text(message: &str) -> Div {
    div()
        .px(px(24.))
        .py(px(8.))
        .text_color(theme::rejected())
        .child(message.to_owned())
}

/// The open file's diff in a uniform list, or a line when it has no lines.
fn diff_body(state: &HistoryState, open: &OpenCommitFile, cx: &Context<Shell>) -> AnyElement {
    if open.rows.is_empty() {
        let text = if open.diff.binary {
            "Binary file."
        } else {
            "No line changes in this file."
        };
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::muted())
            .child(text)
            .into_any_element();
    }
    let mut list = uniform_list(
        "history-diff",
        open.rows.len(),
        cx.processor(diff_row_elements),
    )
    .track_scroll(state.diff_scroll.clone())
    .flex_1()
    .min_h(px(0.))
    .on_scroll_wheel(cx.listener(|this, event, _, cx| this.scroll_history_code(event, cx)));
    // A sideways swipe moves the code, not the list up and down.
    list.style().restrict_scroll_to_axis = Some(true);
    list.into_any_element()
}

/// The diff rows in view, built from the shell's current state. Nothing in
/// them is clickable.
fn diff_row_elements(
    this: &mut Shell,
    range: Range<usize>,
    _: &mut Window,
    _: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let state = &this.history;
    let Some(Ok(open)) = state.open.as_ref() else {
        return Vec::new();
    };
    let rows = open.rows.get(range).unwrap_or_default();
    rows.iter()
        .map(|row| match *row {
            DiffRow::Header(hunk) => hunk_row(open, hunk),
            DiffRow::Line(pos) => line_row(open, pos, state.code_scroll_x),
        })
        .collect()
}

/// A hunk's header text, muted, on a 30px row.
fn hunk_row(open: &OpenCommitFile, hunk: usize) -> AnyElement {
    let header = open
        .diff
        .hunks
        .get(hunk)
        .map_or("", |h| h.header.as_str())
        .to_owned();
    div()
        .w_full()
        .flex()
        .items_center()
        .h(HEADER_ROW_HEIGHT)
        .px(px(24.))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(theme::code_font())
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::muted())
                .child(header),
        )
        .into_any_element()
}

/// One diff line: old and new numbers, the sign and the code. Added and
/// removed lines carry a tint. The row takes no click and no selection.
fn line_row(open: &OpenCommitFile, pos: Pos, scroll_x: Pixels) -> AnyElement {
    let Some(line) = open
        .diff
        .hunks
        .get(pos.hunk)
        .and_then(|hunk| hunk.lines.get(pos.line))
    else {
        return div().into_any_element();
    };
    let (tint, sign, sign_colour) = match line.kind {
        LineKind::Added => (Some(theme::added_tint()), "+", theme::done()),
        LineKind::Removed => (Some(theme::removed_tint()), "\u{2212}", theme::rejected()),
        LineKind::Context => (None, " ", theme::text()),
    };
    div()
        .w_full()
        .flex()
        .items_center()
        .h(LINE_HEIGHT)
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text())
        .when_some(tint, gpui::Styled::bg)
        .child(gutter(line.old_no))
        .child(gutter(line.new_no))
        .child(
            div()
                .w(SIGN_WIDTH)
                .flex_none()
                .text_center()
                .text_color(sign_colour)
                .child(sign),
        )
        .child(code_text(&line.text, scroll_x))
        .into_any_element()
}

/// A line number in the faint gutter, right aligned.
fn gutter(number: Option<u32>) -> Div {
    div()
        .w(GUTTER)
        .flex_none()
        .pr(px(12.))
        .text_right()
        .text_color(theme::faint())
        .child(number.map_or(String::new(), |n| n.to_string()))
}

/// Code text, moved left by `scroll_x` and clipped at the panel's edge rather
/// than wrapped. Used by the diff's lines and the blame's.
fn code_text(text: &str, scroll_x: Pixels) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .pl(px(6.))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(code_scroll::shifted(
            text.to_owned().into_any_element(),
            scroll_x,
        ))
}

/// The commit list, or the line that says why there is none.
fn commit_list(state: &HistoryState, cx: &Context<Shell>) -> Div {
    let column = div().flex().flex_col().flex_1().min_h(px(0.));
    if state.history.is_empty() {
        return column.child(empty_list(state));
    }
    let mut column = column
        .child(column_header(
            state.source.draws_graph().then(|| state.history.lanes()),
        ))
        .child(
            uniform_list(
                "history-list",
                state.history.len(),
                cx.processor(history_rows),
            )
            .track_scroll(state.list_scroll.clone())
            .flex_1()
            .min_h(px(0.)),
        );
    if let Some(error) = state.error.as_ref() {
        column = column.child(error_text(error));
    }
    if state.loading {
        column = column.child(
            div()
                .flex()
                .items_center()
                .h(HEADER_ROW_HEIGHT)
                .px(px(24.))
                .text_color(theme::muted())
                .child("Reading more\u{2026}"),
        );
    }
    column
}

/// What the centre says when no commit is listed: the error, the read in
/// progress, or why the source has none.
fn empty_list(state: &HistoryState) -> Div {
    if let Some(error) = state.error.as_ref() {
        return error_text(error);
    }
    let text = if state.loading {
        "Reading the history\u{2026}"
    } else {
        match &state.source {
            Source::Search { .. } => "No commits match.",
            Source::File { .. } => "No commits changed this file.",
            Source::Log { .. } => "No commits yet.",
        }
    };
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme::muted())
        .child(text)
}

/// A fixed-width cell of the commit list, clipped so it never pushes its
/// neighbours.
fn fixed_cell(width: Pixels) -> Div {
    div().w(width).flex_none().overflow_hidden().truncate()
}

/// The list's column titles, over the cells of a row: the same widths, gap
/// and padding, so each title sits above its column. The graph's lanes are
/// allowed for when the log has a graph.
fn column_header(lanes: Option<usize>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(COLUMN_GAP))
        .h(HEADER_ROW_HEIGHT)
        .mx(px(8.))
        .px(px(12.))
        .overflow_hidden()
        .text_size(px(SMALL))
        .text_color(theme::muted())
        .when_some(lanes, |d, lanes| {
            d.child(div().w(lanes_px(lanes)).flex_none())
        })
        .child(fixed_cell(SHA_WIDTH).child("Commit"))
        .child(div().flex_1().min_w(px(0.)).truncate().child("Subject"))
        .child(fixed_cell(AUTHOR_WIDTH).child("Author"))
        .child(fixed_cell(AGO_WIDTH).text_right().child("Date"))
        .child(
            fixed_cell(REVIEW_WIDTH)
                .text_center()
                .text_color(theme::faint())
                .child("Review"),
        )
}

/// The rows of the list in view. It first asks for the next page when the
/// last visible row is near the end of what is read.
fn history_rows(
    this: &mut Shell,
    range: Range<usize>,
    _: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    this.history_row_shown(range.end.saturating_sub(1), cx);
    let now = now_secs();
    let state = &this.history;
    let lanes = state.source.draws_graph().then(|| state.history.lanes());
    let selected = state.history.selected();
    let rows = state.history.rows().get(range.clone()).unwrap_or_default();
    rows.iter()
        .zip(range)
        .map(|(row, index)| history_row(row, index, selected == Some(index), lanes, now, cx))
        .collect()
}

/// One commit row: its graph cell, short id, ref labels, subject, author, age
/// and review placeholder. A click selects it.
fn history_row(
    row: &HistoryRow,
    index: usize,
    selected: bool,
    lanes: Option<usize>,
    now: i64,
    cx: &Context<Shell>,
) -> AnyElement {
    let mut line = div()
        .id(("history-row", index))
        .flex()
        .items_center()
        .gap(px(COLUMN_GAP))
        .h(ROW_HEIGHT)
        .mx(px(8.))
        .px(px(12.))
        .overflow_hidden()
        .rounded(theme::ROW_RADIUS)
        .cursor_pointer()
        .when(selected, |d| d.bg(theme::selected_line()))
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.select_commit(index, cx);
                this.focus_shell(window);
            },
        ));
    if let Some(lanes) = lanes {
        line = line.child(graph_cell(&row.graph, lanes));
    }
    line.child(
        fixed_cell(SHA_WIDTH)
            .font_family(theme::code_font())
            .text_size(px(SMALL))
            .text_color(theme::faint())
            .child(short(&row.entry.id).to_owned()),
    )
    // The subject starts at the same x on every row; the ref labels follow it
    // and the subject gives way to them.
    .child(
        div()
            .flex()
            .flex_1()
            .min_w(px(0.))
            .items_center()
            .gap(px(8.))
            .overflow_hidden()
            .child(
                div()
                    .min_w(px(0.))
                    .truncate()
                    .text_color(theme::text())
                    .child(row.entry.subject.clone()),
            )
            .children(label_group(&row.labels)),
    )
    .child(
        fixed_cell(AUTHOR_WIDTH)
            .text_color(theme::muted())
            .child(row.entry.author.clone()),
    )
    .child(
        fixed_cell(AGO_WIDTH)
            .text_right()
            .text_color(theme::faint())
            .child(ago(now, row.entry.time)),
    )
    .child(
        fixed_cell(REVIEW_WIDTH)
            .text_center()
            .text_color(theme::faint())
            .child("\u{2013}"),
    )
    .into_any_element()
}

/// The x of lane `lane`'s centre line, from the left edge of the graph cell.
/// A lane past the last one drawn is drawn at the last.
fn lane_x(lane: usize) -> Pixels {
    px(LANE_WIDTH / 2.) + lanes_px(lane.min(MAX_LANES - 1))
}

/// The width of `lanes` lanes.
fn lanes_px(lanes: usize) -> Pixels {
    px(LANE_WIDTH) * f32::from(u16::try_from(lanes).unwrap_or(u16::MAX))
}

/// The graph of one commit's row: the lines through the row, and the dot.
/// Every row's cell is `lanes` wide, so the columns after it line up.
fn graph_cell(graph: &GraphRow, lanes: usize) -> Div {
    let top = graph.top.clone();
    let bottom = graph.bottom.clone();
    let drawing = canvas(
        |_, _, _| (),
        move |bounds, (), window, _| paint_graph(window, bounds, &top, &bottom),
    )
    .absolute()
    .inset_0()
    .size_full();
    div()
        .relative()
        .flex_none()
        .h(ROW_HEIGHT)
        .w(lanes_px(lanes))
        .child(drawing)
        .child(
            div()
                .absolute()
                .left(lane_x(graph.lane) - DOT * 0.5)
                .top(ROW_HEIGHT * 0.5 - DOT * 0.5)
                .size(DOT)
                .rounded_full()
                .border(px(1.5))
                .border_color(theme::ink())
                .bg(theme::paper()),
        )
}

/// Draws a row's lines: the top half from the top edge to the middle, the
/// bottom half from the middle to the bottom edge.
fn paint_graph(window: &mut Window, bounds: Bounds<Pixels>, top: &[Seg], bottom: &[Seg]) {
    let left = bounds.origin.x;
    let top_y = bounds.origin.y;
    let mid = top_y + bounds.size.height * 0.5;
    let bottom_y = top_y + bounds.size.height;
    for seg in top {
        stroke_line(
            window,
            point(left + lane_x(seg.from), top_y),
            point(left + lane_x(seg.to), mid),
        );
    }
    for seg in bottom {
        stroke_line(
            window,
            point(left + lane_x(seg.from), mid),
            point(left + lane_x(seg.to), bottom_y),
        );
    }
}

/// One graph line, 1.5px wide in the hairline colour. A line the path builder
/// cannot make is skipped.
fn stroke_line(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>) {
    let mut builder = PathBuilder::stroke(px(1.5));
    builder.move_to(from);
    builder.line_to(to);
    if let Ok(path) = builder.build() {
        window.paint_path(path, theme::hairline_control());
    }
}

/// The ref labels of a commit, in a row with no chips: the checked-out branch
/// in medium ink, other branches in ink, remotes and tags in muted. None when
/// the commit has no labels.
fn label_group(labels: &[Label]) -> Option<Div> {
    if labels.is_empty() {
        return None;
    }
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(6.))
            .text_size(px(SMALL))
            .children(labels.iter().map(label_text)),
    )
}

/// One ref label as text.
fn label_text(label: &Label) -> Div {
    let (colour, text) = match label.kind {
        LabelKind::Branch | LabelKind::DetachedHead => (theme::ink(), label.name.clone()),
        LabelKind::Remote => (theme::muted(), label.name.clone()),
        LabelKind::Tag => (theme::muted(), format!("tag {}", label.name)),
    };
    div()
        .text_color(colour)
        .when(label.head, |d| d.font_weight(FontWeight::MEDIUM))
        .child(text)
}

/// The right panel: the comparison while there is one, else the selected
/// commit's subject, facts, body, parents and files, or why there are none.
pub fn right(state: &HistoryState, cx: &Context<Shell>) -> Div {
    let body = match &state.compare {
        Some(compare) => compare_body(state, compare, cx),
        None => commit_body(state, cx),
    };
    div()
        .flex()
        .flex_col()
        .h_full()
        .p(theme::PANEL_PADDING)
        .children(state.notice.as_ref().map(|notice| {
            div()
                .pb(px(12.))
                .mb(px(12.))
                .border_b_1()
                .border_color(theme::hairline())
                .text_color(theme::text())
                .child(notice.clone())
        }))
        .child(
            // A commit with many files scrolls here, not off the panel.
            div()
                .id("history-detail")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(body),
        )
        .child(
            div()
                .pt(px(12.))
                .text_size(px(SMALL))
                .text_color(theme::faint())
                .child(
                    "Reviews and agent sessions appear here once rvw links them (M5, M6). \
                     Nothing is guessed.",
                ),
        )
}

/// The selected commit's details, or why they could not be read.
fn commit_body(state: &HistoryState, cx: &Context<Shell>) -> Div {
    match &state.detail {
        None if state.history.is_empty() => div(),
        None => div().text_color(theme::muted()).child("Select a commit."),
        Some(Err(message)) => div().text_color(theme::rejected()).child(message.clone()),
        Some(Ok(detail)) => detail_body(state, detail, now_secs(), cx),
    }
}

/// The comparison with a branch or tag: its title, the counts, a link that
/// closes it, and the files that differ. An empty comparison says so.
fn compare_body(
    state: &HistoryState,
    compare: &Result<CompareView, String>,
    cx: &Context<Shell>,
) -> Div {
    let view = match compare {
        Ok(view) => view,
        Err(message) => {
            return div().flex().flex_col().child(close_compare_link(cx)).child(
                div()
                    .pt(px(12.))
                    .text_color(theme::rejected())
                    .child(message.clone()),
            );
        }
    };
    let count = view.files.len();
    let noun = if count == 1 { "file" } else { "files" };
    let column = div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child(format!("Compared with {}", view.from)),
        )
        .child(div().pt(px(6.)).text_color(theme::muted()).child(format!(
            "{} \u{2192} {}, {count} {noun}",
            view.from,
            short(&view.to)
        )))
        .child(close_compare_link(cx));
    if view.files.is_empty() {
        return column.child(
            div()
                .pt(px(12.))
                .text_color(theme::muted())
                .child("No differences."),
        );
    }
    let open = open_path(state);
    column.children(view.files.iter().enumerate().map(|(index, file)| {
        let selected = open == Some(file.path.as_str());
        file_row(
            "history-compare-file",
            index,
            file,
            selected,
            Shell::open_compare_file,
            cx,
        )
    }))
}

/// "Close comparison": drops the comparison from the right panel.
fn close_compare_link(cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id("history-compare-close")
        .flex()
        .items_center()
        .h(px(30.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::ink())
        .cursor_pointer()
        .child("Close comparison")
        .on_click(cx.listener(
            |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                this.close_compare(cx);
                this.focus_shell(window);
            },
        ))
}

/// The details of one commit, top to bottom.
fn detail_body(state: &HistoryState, detail: &CommitDetail, now: i64, cx: &Context<Shell>) -> Div {
    let subject = detail.message.lines().next().unwrap_or("");
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child(subject.to_owned()),
        )
        .child(meta_line(detail, now))
        .children(body_lines(&detail.message))
        .child(parents_row(&detail.entry.parents))
        .child(files_section(state, detail, cx))
}

/// A middle dot between the facts of a line.
fn dot_sep() -> Div {
    div().child("\u{00b7}")
}

/// The short id, author, age and, when it differs, the committer.
fn meta_line(detail: &CommitDetail, now: i64) -> Div {
    let entry = &detail.entry;
    let age = ago(now, entry.time);
    let when = if age == "now" {
        age
    } else {
        format!("{age} ago")
    };
    let mut line = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(6.))
        .mt(px(6.))
        .text_color(theme::muted())
        .child(
            div()
                .font_family(theme::code_font())
                .text_size(px(SMALL))
                .child(short(&entry.id).to_owned()),
        )
        .child(dot_sep())
        .child(entry.author.clone())
        .child(dot_sep())
        .child(when);
    if detail.committer != entry.author {
        line = line
            .child(dot_sep())
            .child(format!("committed by {}", detail.committer));
    }
    line
}

/// The message after its first line, trimmed, one div per line with empty
/// lines as 8px spacers. None when there is no body.
fn body_lines(message: &str) -> Option<Div> {
    let body = message.split_once('\n').map_or("", |(_, rest)| rest).trim();
    if body.is_empty() {
        return None;
    }
    Some(
        div()
            .mt(px(8.))
            .flex()
            .flex_col()
            .text_color(theme::text())
            .children(body.lines().map(|line| {
                if line.is_empty() {
                    div().h(px(8.))
                } else {
                    div().child(line.to_owned())
                }
            })),
    )
}

/// "Parents" with each parent's short id, or "Merge of" for two or more.
fn parents_row(parents: &[String]) -> Div {
    let label = match parents.len() {
        0 => "Root commit: no parent",
        1 => "Parent",
        _ => "Merge of",
    };
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
        .mt(px(18.))
        .text_color(theme::muted())
        .child(label)
        .children(parents.iter().map(|id| {
            div()
                .font_family(theme::code_font())
                .text_size(px(SMALL))
                .text_color(theme::text())
                .child(short(id).to_owned())
        }))
}

/// "Files, N" and one row per file. The open file is selected.
fn files_section(state: &HistoryState, detail: &CommitDetail, cx: &Context<Shell>) -> Div {
    let open = open_path(state);
    div()
        .flex()
        .flex_col()
        .mt(px(22.))
        .child(
            div()
                .pb(px(6.))
                .text_color(theme::muted())
                .child(format!("Files, {}", detail.files.len())),
        )
        .children(detail.files.iter().enumerate().map(|(index, file)| {
            let selected = open == Some(file.path.as_str());
            file_row(
                "history-file",
                index,
                file,
                selected,
                Shell::open_commit_file,
                cx,
            )
        }))
}

/// One changed file: its status letter, its name and directory, and its
/// counts. A click calls `open` with its path, which opens its diff in the
/// centre. `id` names the row's list, so its rows have unique ids.
fn file_row(
    id: &'static str,
    index: usize,
    file: &FileChange,
    selected: bool,
    open: fn(&mut Shell, &str, &mut Context<Shell>),
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let path = file.path.clone();
    let (dir, name) = file_name(file);
    div()
        .id((id, index))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(28.))
        .px(px(8.))
        .rounded(theme::ROW_RADIUS)
        .text_color(theme::text())
        .when(selected, |d| {
            d.bg(theme::selected_panel())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .cursor_pointer()
        .child(status_letter(file.status))
        .child(
            div()
                .min_w(px(0.))
                .truncate()
                .font_family(theme::code_font())
                .text_size(px(SMALL))
                .child(name),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(theme::code_font())
                .text_size(px(SMALL))
                .text_color(theme::faint())
                .child(dir),
        )
        .child(counts(file))
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                open(this, &path, cx);
                this.focus_shell(window);
            },
        ))
}

/// The directory and the name of a changed file. A renamed or copied file
/// has no directory and shows "old → new" as its name.
fn file_name(file: &FileChange) -> (String, String) {
    if let Some(old) = &file.old_path {
        return (String::new(), format!("{old} \u{2192} {}", file.path));
    }
    let (dir, name) = split_path(&file.path);
    (dir.to_owned(), name.to_owned())
}

/// The directory and the base name of a path; the directory is empty at the top.
fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// The status letter in its colour: added green, deleted red, a modification
/// in text colour, and anything else (a rename, copy or type change) muted.
fn status_letter(status: u8) -> Div {
    let colour = match status {
        b'A' => theme::done(),
        b'D' => theme::rejected(),
        b'M' => theme::text(),
        _ => theme::muted(),
    };
    div()
        .w(px(12.))
        .flex_none()
        .font_family(theme::code_font())
        .text_size(px(SMALL))
        .text_color(colour)
        .child(char::from(status).to_string())
}

/// The added and removed counts in green and red, or "binary" for a file git
/// cannot count.
fn counts(file: &FileChange) -> Div {
    match (file.added, file.deleted) {
        (Some(added), Some(deleted)) => div()
            .flex()
            .flex_none()
            .gap(px(6.))
            .text_size(px(SMALL))
            .child(div().text_color(theme::done()).child(format!("+{added}")))
            .child(
                div()
                    .text_color(theme::rejected())
                    .child(format!("\u{2212}{deleted}")),
            ),
        _ => div()
            .flex_none()
            .text_size(px(SMALL))
            .text_color(theme::faint())
            .child("binary"),
    }
}

/// Seconds since the Unix epoch now; 0 when the clock is before it.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
