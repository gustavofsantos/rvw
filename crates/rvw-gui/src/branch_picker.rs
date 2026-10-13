//! The branch picker: a centred card with one field and the local branches
//! ranked by it. The ranking is `rvw_view::branches`, the switch is
//! `gitx::switch`; this file keeps the picker's state and draws it.

use std::path::Path;

use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Div, Entity, FontWeight, SharedString, Stateful, Subscription,
    Window,
};
use rvw_git::gitx::{self, BranchInfo};
use rvw_view::branches::{self, Hit};

use crate::shell::Shell;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::tools;

/// An open picker. The branches are read once, when it opens.
pub struct BranchPicker {
    input: Entity<TextInput>,
    branches: Result<Vec<BranchInfo>, String>,
    hits: Vec<Hit>,
    highlighted: usize,
    /// Git's refusal of the last switch, first line.
    error: Option<String>,
    _events: Subscription,
    _typing: Subscription,
}

impl Shell {
    /// Opens the picker with focus in its field, closing any other picker.
    pub fn open_branches(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools.branches.is_some() {
            return;
        }
        self.tools.close_all();
        self.finder = None;
        let found =
            gitx::branches(Path::new(&self.snapshot.workspace)).map_err(|e| first_line(&e.stderr));
        let input = cx.new(|cx| TextInput::new("Switch to a branch", cx));
        let events = cx.subscribe(
            &input,
            |this: &mut Self, _, event: &TextInputEvent, cx: &mut Context<Self>| match event {
                TextInputEvent::Submit => this.switch_highlighted(cx),
                TextInputEvent::Cancel => this.close_branches(cx),
            },
        );
        let typing = cx.observe(&input, |this: &mut Self, _, cx: &mut Context<Self>| {
            this.rank_branches(cx);
        });
        input.read(cx).focus(window);
        self.tools.branches = Some(BranchPicker {
            input,
            branches: found,
            hits: Vec::new(),
            highlighted: 0,
            error: None,
            _events: events,
            _typing: typing,
        });
        self.rank_branches(cx);
    }

    pub fn close_branches(&mut self, cx: &mut Context<Self>) {
        self.tools.branches = None;
        cx.notify();
    }

    fn rank_branches(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.tools.branches.as_mut() else {
            return;
        };
        let query = picker.input.read(cx).text().to_owned();
        let names: Vec<String> = match &picker.branches {
            Ok(list) => list.iter().map(|b| b.name.clone()).collect(),
            Err(_) => Vec::new(),
        };
        picker.hits = branches::rank(&query, &names);
        picker.highlighted = 0;
        picker.error = None;
        cx.notify();
    }

    pub(crate) fn move_branch(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(picker) = self.tools.branches.as_mut() {
            let last = picker.hits.len().saturating_sub(1);
            picker.highlighted = if down {
                (picker.highlighted + 1).min(last)
            } else {
                picker.highlighted.saturating_sub(1)
            };
        }
        cx.notify();
    }

    fn switch_highlighted(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.tools.branches.as_ref().map(|p| p.highlighted) {
            self.switch_to(index, cx);
        }
    }

    /// Switches to the branch of row `index`. The current branch only closes
    /// the picker; a refusal stays in the picker under the field.
    pub fn switch_to(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(picker) = self.tools.branches.as_ref() else {
            return;
        };
        let Some(hit) = picker.hits.get(index) else {
            return;
        };
        let name = hit.name.clone();
        let current = picker
            .branches
            .as_ref()
            .is_ok_and(|list| list.iter().any(|b| b.current && b.name == name));
        if current {
            self.close_branches(cx);
            return;
        }
        let outcome = gitx::switch(Path::new(&self.snapshot.workspace), &name)
            .map_err(|e| first_line(&e.stderr))
            .and_then(|()| self.reload_for_new_base(cx));
        match outcome {
            Ok(()) => self.close_branches(cx),
            Err(message) => {
                if let Some(picker) = self.tools.branches.as_mut() {
                    picker.error = Some(message);
                }
                cx.notify();
            }
        }
    }
}

/// Git's message cut to one trimmed line.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("git failed")
        .to_owned()
}

pub fn overlay(picker: &BranchPicker, cx: &Context<Shell>) -> Div {
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
    let list =
        match &picker.branches {
            Err(message) => div()
                .px(px(24.))
                .py(px(16.))
                .text_color(theme::rejected())
                .child(message.clone()),
            Ok(_) if picker.hits.is_empty() => div()
                .px(px(24.))
                .py(px(16.))
                .text_color(theme::muted())
                .child("No branch matches."),
            Ok(list) => div().flex().flex_col().py(px(6.)).children(
                picker.hits.iter().enumerate().filter_map(|(row, hit)| {
                    let info = list.get(hit.index)?;
                    Some(branch_row(row, hit, info, row == picker.highlighted, cx))
                }),
            ),
        };
    tools::overlay(tools::card(
        div().child(picker.input.clone()),
        body.child(list),
        &["\u{2191}\u{2193} move", "\u{21b5} switch", "esc close"],
    ))
}

fn branch_row(
    row: usize,
    hit: &Hit,
    info: &BranchInfo,
    highlighted: bool,
    cx: &Context<Shell>,
) -> Stateful<Div> {
    let name = tools::matched_text(&hit.name, &hit.matches)
        .flex_none()
        .font_family(theme::code_font())
        .text_size(px(theme::CODE_FONT_SIZE))
        .text_color(theme::text());
    let mut label = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .flex_1()
        .min_w(px(0.))
        .child(name);
    if info.current {
        label = label.child(
            div()
                .flex_none()
                .text_size(px(theme::KEY_FONT_SIZE))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::done())
                .child("current"),
        );
    }
    if let Some(upstream) = info.upstream.as_deref() {
        label = label.child(
            div()
                .flex_none()
                .font_family(theme::code_font())
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(upstream.to_owned()),
        );
    }
    div()
        .id(SharedString::from(format!("branch-{row}")))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(40.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .when(highlighted, |d| d.bg(theme::hairline()))
        .cursor_pointer()
        .on_click(cx.listener(
            move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
                this.switch_to(row, cx);
            },
        ))
        .child(label)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .text_color(theme::muted())
                .child(info.subject.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(info.when.clone()),
        )
}
