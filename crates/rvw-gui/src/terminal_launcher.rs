//! The new-terminal palette (⌘T) and the quit question. The rows and their
//! order are `rvw_view::terminals`; this file draws the card, and starts the terminal a row names. The
//! quit question lists the running terminals before the window closes.

use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Div, Entity, ScrollHandle, SharedString, Stateful, Subscription,
    Window,
};
use rvw_view::terminals::{self, LaunchEntry};

use crate::shell::Shell;
use crate::terminal_dock::TerminalView;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::tools;

/// An open new-terminal palette.
pub struct Launcher {
    input: Entity<TextInput>,
    entries: Vec<LaunchEntry>,
    highlighted: usize,
    scroll: ScrollHandle,
    _events: Subscription,
    _typing: Subscription,
}

impl Shell {
    /// ⌘T: opens the palette, or closes it when it is open.
    pub(crate) fn open_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.launcher.is_some() {
            self.close_launcher(window, cx);
            return;
        }
        if self.composer.is_some() || self.submit.is_some() || self.quit_ask.is_some() {
            return;
        }
        self.tools.close_all();
        self.finder = None;
        let input = cx.new(|cx| TextInput::new("Run a command, or press Enter for a shell", cx));
        let events = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, event: &TextInputEvent, window, cx| match event {
                TextInputEvent::Submit => {
                    let at = this.launcher.as_ref().map(|open| open.highlighted);
                    if let Some(at) = at {
                        this.run_launcher_row(at, window, cx);
                    }
                }
                TextInputEvent::Cancel => this.close_launcher(window, cx),
            },
        );
        let typing = cx.observe(&input, |this: &mut Self, _, cx: &mut Context<Self>| {
            this.refresh_launcher(cx);
        });
        input.read(cx).focus(window);
        self.launcher = Some(Launcher {
            input,
            entries: Vec::new(),
            highlighted: 0,
            scroll: ScrollHandle::new(),
            _events: events,
            _typing: typing,
        });
        self.refresh_launcher(cx);
    }

    /// Closes the palette and gives the keyboard back to the window.
    pub(crate) fn close_launcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.launcher.take().is_some() {
            self.focus_shell(window);
            cx.notify();
        }
    }

    fn refresh_launcher(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.launcher.as_mut() else {
            return;
        };
        let query = open.input.read(cx).text().to_owned();
        open.entries = terminals::launch_entries(&query);
        open.highlighted = 0;
        open.scroll.scroll_to_item(0);
        cx.notify();
    }

    pub(crate) fn move_launcher(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(open) = self.launcher.as_mut() {
            let last = open.entries.len().saturating_sub(1);
            open.highlighted = if down {
                (open.highlighted + 1).min(last)
            } else {
                open.highlighted.saturating_sub(1)
            };
            open.scroll.scroll_to_item(open.highlighted);
        }
        cx.notify();
    }

    /// Starts the terminal of row `index` in the workspace root, shows the
    /// dock with it active, and closes the palette.
    pub(crate) fn run_launcher_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let launch = self
            .launcher
            .as_ref()
            .and_then(|open| open.entries.get(index))
            .map(|entry| entry.launch.clone());
        let Some(launch) = launch else {
            return;
        };
        self.launcher = None;
        self.start_terminal(&launch, window, cx);
        cx.notify();
    }

    /// Asks before the window closes with terminals running; closes at once
    /// otherwise. The processes are looked at now.
    pub(crate) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ask = self
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.update(cx, TerminalView::quit_guard));
        match ask {
            None => cx.quit(),
            Some(message) => {
                self.launcher = None;
                self.quit_ask = Some(message);
                self.focus_shell(window);
                cx.notify();
            }
        }
    }

    /// Whether the platform may close the window now.
    pub(crate) fn close_allowed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.quit_ask.is_some() {
            return false;
        }
        self.request_quit(window, cx);
        // `request_quit` quits itself when nothing runs; the window then
        // closes with the app. With a question open it stays.
        self.quit_ask.is_none()
    }

    pub(crate) fn cancel_quit(&mut self, cx: &mut Context<Self>) {
        if self.quit_ask.take().is_some() {
            cx.notify();
        }
    }

    /// Enter or Esc while the question is open. True when the key was taken.
    pub(crate) fn quit_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        if self.quit_ask.is_none() {
            return false;
        }
        match key {
            "enter" => cx.quit(),
            "escape" => self.cancel_quit(cx),
            _ => {}
        }
        true
    }
}

pub fn overlay(open: &Launcher, cx: &Context<Shell>) -> Div {
    let rows =
        open.entries.iter().enumerate().map(|(index, entry)| {
            row(index, entry, index == open.highlighted, cx).into_any_element()
        });
    let list = div()
        .id("launcher-results")
        .flex()
        .flex_col()
        .py(px(6.))
        .max_h(px(420.))
        .overflow_y_scroll()
        .track_scroll(&open.scroll)
        .children(rows);
    tools::overlay(tools::card(
        div().child(open.input.clone()),
        list,
        &[
            "\u{2191}\u{2193} move",
            "\u{21b5} run in the workspace root",
            "esc close",
        ],
    ))
}

fn row(index: usize, entry: &LaunchEntry, highlighted: bool, cx: &Context<Shell>) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("launch-{index}")))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(32.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .text_color(theme::text())
        .when(highlighted, |d| d.bg(theme::hairline()))
        .cursor_pointer()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .font_family(theme::code_font())
                .child(entry.title.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(entry.note),
        )
        .on_click(cx.listener(
            move |this: &mut Shell,
                  _: &ClickEvent,
                  window: &mut Window,
                  cx: &mut Context<Shell>| {
                this.run_launcher_row(index, window, cx);
            },
        ))
}

/// The quit question: what runs, and the two ways out.
pub fn quit_overlay(message: &str, cx: &Context<Shell>) -> Div {
    let button = |id: &'static str, label: &'static str, strong: bool| {
        div()
            .id(id)
            .px(px(14.))
            .h(px(30.))
            .flex()
            .items_center()
            .rounded(theme::CONTROL_RADIUS)
            .cursor_pointer()
            .border_1()
            .border_color(theme::hairline())
            .text_color(if strong {
                theme::rejected()
            } else {
                theme::text()
            })
            .child(label)
    };
    let card = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .w(px(520.))
        .p(px(24.))
        .rounded(theme::WINDOW_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline())
        .child(div().text_color(theme::ink()).child(message.to_owned()))
        .child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(button("quit-cancel", "Keep working (Esc)", false).on_click(
                    cx.listener(|this: &mut Shell, _: &ClickEvent, _, cx| this.cancel_quit(cx)),
                ))
                .child(
                    button("quit-confirm", "Quit (Enter)", true)
                        .on_click(cx.listener(|_: &mut Shell, _: &ClickEvent, _, cx| cx.quit())),
                ),
        );
    tools::overlay(card)
}
