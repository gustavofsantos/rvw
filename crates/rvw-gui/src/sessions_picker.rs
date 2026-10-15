//! The Sessions picker: a centred card listing the sessions agents reported
//! in this workspace, newest first. The rows are
//! `rvw_view::provenance_sessions`; choosing one shows its commits in the
//! History screen. It has no field: the arrows move, Enter opens and Esc
//! closes, all through `Shell::tools_key`.

use std::time::{SystemTime, UNIX_EPOCH};

use gpui::prelude::*;
use gpui::{ClickEvent, Context, Div, ScrollHandle, SharedString, Stateful, Window, div, px};
use rvw_view::provenance_sessions::{self, Row};

use crate::data;
use crate::shell::Shell;
use crate::theme;
use crate::tools;

/// An open picker. The sessions are read once, when it opens.
pub struct SessionsPicker {
    rows: Vec<Row>,
    /// What the list says when no session was reported.
    empty: Option<&'static str>,
    /// Why the sessions could not be read, one line.
    error: Option<String>,
    /// The row Enter takes.
    pub(crate) highlighted: usize,
    /// Keeps the highlighted row in view when the arrows move it.
    scroll: ScrollHandle,
}

impl Shell {
    /// Opens the picker, closing any other picker. A second call does nothing.
    pub fn open_sessions(&mut self, cx: &mut Context<Self>) {
        if self.tools.sessions.is_some() {
            return;
        }
        self.tools.close_all();
        self.finder = None;
        self.git.ask = None;
        let (rows, empty, error) = match data::sessions(&self.service, &self.identity.workspace) {
            Ok(sessions) => (
                provenance_sessions::rows(&sessions, now_secs()),
                provenance_sessions::empty_state(&sessions),
                None,
            ),
            Err(message) => (
                Vec::new(),
                None,
                Some(
                    message
                        .lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty())
                        .unwrap_or("The sessions could not be read.")
                        .to_owned(),
                ),
            ),
        };
        self.tools.sessions = Some(SessionsPicker {
            rows,
            empty,
            error,
            highlighted: 0,
            scroll: ScrollHandle::new(),
        });
        cx.notify();
    }

    pub fn close_sessions(&mut self, cx: &mut Context<Self>) {
        self.tools.sessions = None;
        cx.notify();
    }

    /// Down or up arrow: moves the highlight, stopping at the ends.
    pub(crate) fn move_session(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(picker) = self.tools.sessions.as_mut() {
            let last = picker.rows.len().saturating_sub(1);
            picker.highlighted = if down {
                (picker.highlighted + 1).min(last)
            } else {
                picker.highlighted.saturating_sub(1)
            };
            picker.scroll.scroll_to_item(picker.highlighted);
        }
        cx.notify();
    }

    /// Shows the commits of row `index` in the History screen and closes the
    /// picker. An index with no row does nothing.
    pub(crate) fn choose_session(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(session_id) = self
            .tools
            .sessions
            .as_ref()
            .and_then(|picker| picker.rows.get(index))
            .map(|row| row.session_id.clone())
        else {
            return;
        };
        self.close_sessions(cx);
        self.show_session_history(&session_id, cx);
    }
}

/// Seconds since the epoch, for the ages in the rows.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

pub fn overlay(picker: &SessionsPicker, cx: &Context<Shell>) -> Div {
    let mut body = div().flex().flex_col();
    if let Some(error) = picker.error.as_deref() {
        body = body.child(
            div()
                .px(px(20.))
                .py(px(8.))
                .text_color(theme::rejected())
                .child(error.to_owned()),
        );
    }
    let list = picker.empty.map_or_else(|| results(picker, cx), empty_line);
    tools::overlay(tools::card(
        div().text_color(theme::ink()).child("Sessions"),
        body.child(list),
        &["\u{2191}\u{2193} move", "\u{21b5} open", "esc close"],
    ))
}

/// The line the list shows when no session was reported.
fn empty_line(text: &str) -> Div {
    div()
        .px(px(24.))
        .py(px(16.))
        .text_color(theme::muted())
        .child(text.to_owned())
}

/// The rows, scrolled to the highlighted one.
fn results(picker: &SessionsPicker, cx: &Context<Shell>) -> Div {
    // The rows are the scrolled element's own children:
    // `scroll_to_item` finds the highlighted one by its index.
    let rows: Vec<_> = picker
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            session_row(index, row, index == picker.highlighted, cx).into_any_element()
        })
        .collect();
    div().flex().flex_col().child(
        div()
            .id("sessions-results")
            .flex()
            .flex_col()
            .max_h(px(560.))
            .py(px(6.))
            .overflow_y_scroll()
            .track_scroll(&picker.scroll)
            .children(rows),
    )
}

/// One session: who ran it and its summary, then its commit count and age.
fn session_row(index: usize, row: &Row, highlighted: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let mut label = div()
        .flex()
        .items_center()
        .gap(px(12.))
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_size(px(theme::CODE_FONT_SIZE))
                .text_color(theme::text())
                .child(row.who.clone()),
        );
    if !row.summary.is_empty() {
        label = label.child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme::muted())
                .child(row.summary.clone()),
        );
    }
    let facts: Vec<&str> = [row.commits.as_str(), row.age.as_str()]
        .into_iter()
        .filter(|fact| !fact.is_empty())
        .collect();
    div()
        .id(SharedString::from(format!("session-{index}")))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(32.))
        .mx(px(6.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .cursor_pointer()
        .when(highlighted, |d| d.bg(theme::hairline()))
        .hover(|style| style.bg(theme::hairline()))
        .child(label)
        .child(
            div()
                .flex_none()
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(facts.join(" \u{00b7} ")),
        )
        .on_click(cx.listener(
            move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.choose_session(index, cx);
            },
        ))
}
