//! The Changes screen: the staged and unstaged lists on the left, the open
//! file's diff with its hunk and line staging in the centre, and the commit
//! box with the checkpoints on the right. Every fact comes from
//! `ChangesState` and the repository facts; this file only lays it out.

use std::ops::Range;

use gpui::prelude::*;
use gpui::{
    div, px, uniform_list, AnyElement, ClickEvent, Context, Div, Entity, FontWeight, SharedString,
    Stateful, Window,
};
use rvw_core::types::Comment;
use rvw_git::merge::Operation;
use rvw_view::commit_screen::{
    before_commit, commit_label, hunk_open_comments, Entry, LineChoice, Side,
};
use rvw_view::diff::{Line, LineKind};
use rvw_view::selection::Pos;

use crate::changes_state::{ChangesState, DiffRow, ListRow, OpenChange};
use crate::code_scroll;
use crate::git_tools::RepoFacts;
use crate::history_actions::operation_name;
use crate::icon::{self, Icon};
use crate::queue_screen::noun;
use crate::review_screen::LINE_HEIGHT;
use crate::shell::Shell;
use crate::text_area::TextArea;
use crate::theme;

/// The staged and unstaged lists, or why they cannot be shown.
pub fn left(state: &ChangesState, cx: &Context<Shell>) -> Div {
    let body: AnyElement = if let Some(error) = &state.load_error {
        div()
            .px(px(24.))
            .py(px(8.))
            .text_color(theme::rejected())
            .child(error.clone())
            .into_any_element()
    } else if state.lists.staged.is_empty() && state.lists.unstaged.is_empty() {
        notice("No changes: the worktree matches the last commit.").into_any_element()
    } else {
        uniform_list(
            "changes-list",
            state.list_rows.len(),
            cx.processor(list_row_elements),
        )
        .track_scroll(state.list_scroll.clone())
        .flex_1()
        .min_h(px(0.))
        .into_any_element()
    };
    div().flex().flex_col().h_full().py(px(20.)).child(body)
}

/// The rows of the left list in view, built from the shell's current state.
fn list_row_elements(
    this: &mut Shell,
    range: Range<usize>,
    _: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let state = &this.changes;
    let rows = state.list_rows.get(range).unwrap_or_default();
    rows.iter()
        .map(|row| match *row {
            ListRow::Heading(side) => heading_row(side, state.lists.side(side).len(), cx),
            ListRow::File(side, index) => match state.lists.side(side).get(index) {
                Some(entry) => {
                    let selected = state
                        .open
                        .as_ref()
                        .is_some_and(|open| open.side == side && open.path == entry.path);
                    file_row(entry, side, selected, cx)
                }
                None => div().into_any_element(),
            },
        })
        .collect()
}

/// "Staged, N" or "Unstaged, N", with the link that moves the whole list. The
/// link is dim, and does nothing, when the list is empty.
fn heading_row(side: Side, count: usize, cx: &Context<Shell>) -> AnyElement {
    let (label, action, id) = match side {
        Side::Staged => ("Staged", "Unstage all", "changes-unstage-all"),
        Side::Unstaged => ("Unstaged", "Stage all", "changes-stage-all"),
    };
    let mut link = div().id(id).font_weight(FontWeight::MEDIUM).child(action);
    if count > 0 {
        link = link
            .text_color(theme::ink())
            .cursor_pointer()
            .on_click(cx.listener(
                move |this: &mut Shell,
                      _: &ClickEvent,
                      window: &mut Window,
                      cx: &mut Context<Shell>| {
                    this.move_all(side, cx);
                    this.focus_shell(window);
                },
            ));
    } else {
        link = link.text_color(theme::faint());
    }
    div()
        .flex()
        .items_center()
        .h(px(30.))
        .px(px(24.))
        .child(
            div()
                .text_color(theme::muted())
                .child(format!("{label}, {count}")),
        )
        .child(div().flex_1())
        .child(link)
        .into_any_element()
}

/// One file of a list: its letter, its name and directory, the open comments
/// on it, and a click that opens its diff.
fn file_row(entry: &Entry, side: Side, selected: bool, cx: &Context<Shell>) -> AnyElement {
    let (dir, name) = split_path(&entry.path);
    let path = entry.path.clone();
    let on_click = cx.listener(
        move |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
            this.open_change(side, &path, cx);
            this.focus_shell(window);
        },
    );
    div()
        .id(SharedString::from(format!("chg-{side:?}-{}", entry.path)))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(30.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .text_color(theme::text())
        .when(selected, |d| {
            d.bg(theme::selected_panel())
                .text_color(theme::ink())
                .font_weight(FontWeight::MEDIUM)
        })
        .cursor_pointer()
        .on_click(on_click)
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .font_family(theme::code_font())
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(entry.letter.to_string()),
        )
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_size(px(12.))
                .child(name.to_owned()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(theme::code_font())
                .text_size(px(12.))
                .text_color(theme::faint())
                .child(dir.to_owned()),
        )
        .when(entry.open_comments > 0, |d| {
            d.child(
                icon::labelled(
                    Icon::Pending,
                    icon::SMALL,
                    theme::pending(),
                    entry.open_comments.to_string(),
                )
                .flex_none()
                .font_weight(FontWeight::NORMAL),
            )
        })
        .into_any_element()
}

/// The directory and the base name of a path; the directory is empty at the top.
fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// The centre: the open file's header and diff, or a line saying none is open.
pub fn centre(state: &ChangesState, cx: &Context<Shell>) -> Stateful<Div> {
    let root = div()
        .id("changes-centre")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .bg(theme::paper());
    match &state.open {
        None => root.child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme::muted())
                .child("Select a file to see its changes."),
        ),
        Some(open) => root
            .child(header_bar(open, cx))
            .children(whole_note(open))
            .child(diff_body(state, open, cx)),
    }
}

/// The open file's path, its counts, and the buttons that move it or discard
/// its unstaged changes.
fn header_bar(open: &OpenChange, cx: &Context<Shell>) -> Div {
    let side = open.side;
    let move_path = open.path.clone();
    let label = match side {
        Side::Staged => "Unstage file",
        Side::Unstaged => "Stage file",
    };
    let move_button = button("changes-move-file", label).on_click(cx.listener(
        move |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
            this.move_file(side, &move_path, cx);
            this.focus_shell(window);
        },
    ));
    let mut bar = div()
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(48.))
        .px(px(24.))
        .border_b_1()
        .border_color(theme::hairline())
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme::ink())
                .child(open.path.clone()),
        )
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
        .child(move_button);
    if side == Side::Unstaged {
        let discard_path = open.path.clone();
        bar = bar.child(button("changes-discard", "Discard").on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.discard_file(&discard_path, cx);
                this.focus_shell(window);
            },
        )));
    }
    bar
}

/// A secondary control: 28px tall, an 8px radius and a hairline border.
fn button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .h(px(28.))
        .px(px(12.))
        .rounded(theme::CONTROL_RADIUS)
        .border_1()
        .border_color(theme::hairline_control())
        .bg(theme::paper())
        .font_family(theme::ui_font())
        .text_size(px(theme::BASE_FONT_SIZE))
        .text_color(theme::ink())
        .cursor_pointer()
        .child(label)
}

/// The line under the header when the file moves only as a whole.
fn whole_note(open: &OpenChange) -> Option<Div> {
    let text = if open.diff.binary {
        "Binary file: it is staged as a whole."
    } else if open.whole_file_only {
        "This file is staged as a whole."
    } else {
        return None;
    };
    Some(
        div()
            .px(px(24.))
            .py(px(8.))
            .text_color(theme::muted())
            .child(text),
    )
}

/// The diff's rows in a uniform list, or a line when there are none.
fn diff_body(state: &ChangesState, open: &OpenChange, cx: &Context<Shell>) -> AnyElement {
    if open.rows.is_empty() {
        let text = if open.diff.binary {
            ""
        } else {
            "No line changes in this file."
        };
        return notice(text).flex_1().into_any_element();
    }
    let mut list = uniform_list(
        "changes-diff",
        open.rows.len(),
        cx.processor(diff_row_elements),
    )
    .track_scroll(state.diff_scroll.clone())
    .flex_1()
    .min_h(px(0.))
    .on_scroll_wheel(cx.listener(|this, event, _, cx| this.scroll_change_code(event, cx)));
    // A sideways swipe moves the code, not the list up and down.
    list.style().restrict_scroll_to_axis = Some(true);
    list.into_any_element()
}

/// The diff rows in view, built from the shell's current state.
fn diff_row_elements(
    this: &mut Shell,
    range: Range<usize>,
    _: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<AnyElement> {
    let state = &this.changes;
    let Some(open) = state.open.as_ref() else {
        return Vec::new();
    };
    let comments = &this.snapshot.comments;
    let rows = open.rows.get(range).unwrap_or_default();
    rows.iter()
        .map(|row| match *row {
            DiffRow::Header(hunk) => hunk_row(open, hunk, comments, cx),
            DiffRow::Line(pos) => line_row(open, state.choice, pos, state.code_scroll_x, cx),
        })
        .collect()
}

/// A hunk's header: its text, the open comments that touch it, and the link
/// that stages or unstages it. The link is absent for a whole-file change.
fn hunk_row(
    open: &OpenChange,
    hunk: usize,
    comments: &[Comment],
    cx: &Context<Shell>,
) -> AnyElement {
    let header = open
        .diff
        .hunks
        .get(hunk)
        .map_or("", |h| h.header.as_str())
        .to_owned();
    let touching = hunk_open_comments(&open.diff, hunk, &open.path, comments);
    let mut row = div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(12.))
        .h(LINE_HEIGHT)
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
        );
    if touching > 0 {
        let words = noun(
            touching,
            "open comment touches this hunk",
            "open comments touch this hunk",
        );
        row = row.child(
            icon::labelled(
                Icon::Pending,
                icon::SMALL,
                theme::pending(),
                format!("{touching} {words}"),
            )
            .flex_none()
            .text_size(px(12.)),
        );
    }
    if !open.whole_file_only {
        row = row.child(hunk_link(open.side, hunk, cx));
    }
    row.into_any_element()
}

/// "Stage hunk" or "Unstage hunk", by the side the file is on.
fn hunk_link(side: Side, hunk: usize, cx: &Context<Shell>) -> Stateful<Div> {
    let label = match side {
        Side::Staged => "Unstage hunk",
        Side::Unstaged => "Stage hunk",
    };
    div()
        .id(("changes-hunk", hunk))
        .flex_none()
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme::ink())
        .cursor_pointer()
        .child(label)
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.move_hunk(hunk, cx);
                this.focus_shell(window);
            },
        ))
}

/// One diff line: old and new numbers, the sign, and the text. Added and
/// removed lines carry a tint; a chosen line takes the selection colour
/// instead. A click chooses the line, and Shift extends the choice.
fn line_row(
    open: &OpenChange,
    choice: LineChoice,
    pos: Pos,
    scroll_x: gpui::Pixels,
    cx: &Context<Shell>,
) -> AnyElement {
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
    let background = if choice.contains(pos) {
        Some(theme::selected_line())
    } else {
        tint
    };
    div()
        .id(SharedString::from(format!(
            "changes-line-{}-{}",
            pos.hunk, pos.line
        )))
        .w_full()
        .flex()
        .items_center()
        .h(LINE_HEIGHT)
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text())
        .when_some(background, gpui::Styled::bg)
        .cursor_pointer()
        .on_click(cx.listener(
            move |this: &mut Shell,
                  event: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.choose_change_line(pos, event.modifiers().shift, cx);
                this.focus_shell(window);
            },
        ))
        .child(gutter(line.old_no))
        .child(gutter(line.new_no))
        .child(
            div()
                .w(px(16.))
                .flex_none()
                .text_center()
                .text_color(sign_colour)
                .child(sign),
        )
        .child(line_text(line, scroll_x))
        .into_any_element()
}

/// A line number in the faint gutter, right aligned.
fn gutter(number: Option<u32>) -> Div {
    div()
        .w(px(46.))
        .flex_none()
        .pr(px(12.))
        .text_right()
        .text_color(theme::faint())
        .child(number.map_or(String::new(), |n| n.to_string()))
}

/// The text of a line, moved left by `scroll_x` and clipped at the panel's
/// edge rather than wrapped.
fn line_text(line: &Line, scroll_x: gpui::Pixels) -> Div {
    div()
        .flex_1()
        .min_w(px(0.))
        .pl(px(6.))
        .whitespace_nowrap()
        .overflow_hidden()
        .child(code_scroll::shifted(
            line.text.clone().into_any_element(),
            scroll_x,
        ))
}

/// The commit box: the message, amend, the notices about open comments, the
/// errors, the commit button, and the checkpoints.
pub fn right(
    state: &ChangesState,
    facts: &RepoFacts,
    comments: &[Comment],
    cx: &Context<Shell>,
) -> Div {
    let mut column = div()
        .flex()
        .flex_col()
        .h_full()
        .p(theme::PANEL_PADDING)
        .gap(px(18.))
        .children(operation_block(facts, cx))
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child("Commit"),
        );
    if let Some(message) = &state.message {
        column = column.child(message_box(message));
    }
    column
        .child(amend_row(state, cx))
        .children(
            before_commit(&state.lists, comments)
                .into_iter()
                .map(open_comment_notice),
        )
        .children(
            state
                .error
                .as_ref()
                .map(|error| div().text_color(theme::rejected()).child(error.clone())),
        )
        .children(
            state
                .notice
                .as_ref()
                .map(|notice| div().text_color(theme::muted()).child(notice.clone())),
        )
        .child(commit_button(state, cx))
        .child(div().h(px(1.)).bg(theme::hairline()))
        .child(crate::checkpoint_ui::list(facts, cx))
}

/// The merge, cherry-pick, revert or rebase stopped in the worktree, at the top
/// of the right panel. A merge or cherry-pick lists its conflicts and offers
/// the merge tool, or Finish once none is left, and Abort. A revert or rebase
/// is named only: rvw did not start it, so it is finished in a terminal.
/// `None` when no operation is stopped.
fn operation_block(facts: &RepoFacts, cx: &Context<Shell>) -> Option<Div> {
    let operation = facts.operation?;
    let name = operation_name(operation);
    let title = match operation {
        Operation::Merge | Operation::CherryPick => {
            format!("{} in progress", capitalised(name))
        }
        Operation::Revert | Operation::Rebase => format!("A {name} is in progress"),
    };
    let mut block = div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .pb(px(12.))
        .border_b_1()
        .border_color(theme::hairline())
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::ink())
                .child(title),
        );
    match operation {
        Operation::Revert | Operation::Rebase => {
            block = block.child(muted_line("rvw did not start it; finish it in a terminal."));
        }
        Operation::Merge | Operation::CherryPick if facts.conflicted.is_empty() => {
            block = block
                .child(muted_line("No conflicts left. Finish to commit it."))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            button("changes-operation-finish", "Finish").on_click(cx.listener(
                                |this: &mut Shell,
                                 _: &ClickEvent,
                                 window: &mut Window,
                                 cx: &mut Context<Shell>| {
                                    this.finish_operation(cx);
                                    this.focus_shell(window);
                                },
                            )),
                        )
                        .child(abort_button(cx)),
                );
        }
        Operation::Merge | Operation::CherryPick => {
            let mut files = div().flex().flex_col().child(muted_line("Conflicts:"));
            for path in &facts.conflicted {
                files = files.child(
                    div()
                        .font_family(theme::code_font())
                        .text_size(px(theme::CODE_FONT_SIZE))
                        .text_color(theme::rejected())
                        .child(path.clone()),
                );
            }
            block = block.child(files).child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        button("changes-operation-mergetool", "Open merge tool").on_click(
                            cx.listener(
                                |this: &mut Shell,
                                 _: &ClickEvent,
                                 window: &mut Window,
                                 cx: &mut Context<Shell>| {
                                    this.open_merge_tool(cx);
                                    this.focus_shell(window);
                                },
                            ),
                        ),
                    )
                    .child(abort_button(cx)),
            );
        }
    }
    Some(block)
}

/// The Abort control of a stopped merge or cherry-pick.
fn abort_button(cx: &Context<Shell>) -> Stateful<Div> {
    button("changes-operation-abort", "Abort").on_click(cx.listener(
        |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
            this.abort_operation(cx);
            this.focus_shell(window);
        },
    ))
}

/// A dim line of body text in the right panel.
fn muted_line(text: &str) -> Div {
    div().text_color(theme::muted()).child(text.to_owned())
}

/// The text with its first letter in upper case: "merge" becomes "Merge".
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// One notice about open comments on staged files. It never blocks a commit.
fn open_comment_notice(text: String) -> Div {
    div()
        .flex()
        .gap(px(8.))
        .text_color(theme::muted())
        .child(icon::view(Icon::Pending, icon::SIZE, theme::pending()))
        .child(div().flex_1().min_w(px(0.)).child(text))
}

/// The message field in the composer's box. The box takes no click, so a
/// click in it leaves the keyboard with the field.
fn message_box(message: &Entity<TextArea>) -> Div {
    div()
        .px(px(14.))
        .py(px(8.))
        .rounded(theme::CONTROL_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline_control())
        .child(message.clone())
}

/// The amend toggle. Dim, with no click, when there is no commit to amend or
/// HEAD is pushed; the hint says why when it is pushed.
fn amend_row(state: &ChangesState, cx: &Context<Shell>) -> Div {
    let enabled = state.has_head && !state.head_pushed;
    let box_icon = if state.amend {
        Icon::Checked
    } else {
        Icon::Unchecked
    };
    let mut line = div()
        .id("changes-amend")
        .flex()
        .items_center()
        .gap(px(8.))
        .child(icon::view(
            box_icon,
            icon::SIZE,
            if enabled {
                theme::ink()
            } else {
                theme::faint()
            },
        ))
        .child("Amend the last commit");
    if enabled {
        line = line
            .text_color(theme::ink())
            .cursor_pointer()
            .on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                    this.toggle_amend(cx);
                    this.focus_shell(window);
                },
            ));
    } else {
        line = line.text_color(theme::faint());
    }
    let mut column = div().flex().flex_col().gap(px(6.)).child(line);
    if state.head_pushed {
        column = column.child(
            div()
                .text_color(theme::muted())
                .child("Pushed already: amending would need a force push."),
        );
    }
    column
}

/// The primary button: the commit with its file count, or the amend. Dim, with
/// no click, when there is nothing staged and no amend.
fn commit_button(state: &ChangesState, cx: &Context<Shell>) -> Stateful<Div> {
    let ready = !state.lists.staged.is_empty() || state.amend;
    let label = if state.amend {
        "Amend the last commit".to_owned()
    } else {
        commit_label(state.lists.staged.len())
    };
    let button = div()
        .id("changes-commit")
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .h(px(36.))
        .rounded(theme::CONTROL_RADIUS)
        .font_family(theme::ui_font())
        .child(label)
        .child(key_cap("⇧⌘C"));
    if ready {
        button
            .bg(theme::ink())
            .text_color(theme::paper())
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .on_click(cx.listener(
                |this: &mut Shell, _: &ClickEvent, window: &mut Window, cx: &mut Context<Shell>| {
                    this.commit_changes(cx);
                    this.focus_shell(window);
                },
            ))
    } else {
        button
            .bg(theme::selected_panel())
            .text_color(theme::faint())
    }
}

/// A key's name in the key font, as the buttons show it.
fn key_cap(glyph: &str) -> Div {
    div()
        .font_family(theme::code_font())
        .text_size(px(theme::KEY_FONT_SIZE))
        .text_color(theme::faint())
        .child(theme::chord(glyph))
}

/// A dim line of body text, used for empty and unreadable states.
fn notice(text: &str) -> Div {
    div()
        .px(px(24.))
        .py(px(8.))
        .text_color(theme::muted())
        .child(text.to_owned())
}
