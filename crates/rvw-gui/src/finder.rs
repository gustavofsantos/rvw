//! The find-a-file overlay (⌘P): a card with one field and the best matches.
//! The matching and the order are `rvw_view::finder`; this file lays the card
//! out, moves the highlight, and opens the file a row names.

use gpui::prelude::*;
use gpui::{
    actions, div, px, rgba, App, ClickEvent, Context, Div, Entity, FontWeight, KeyBinding,
    SharedString, Stateful, Subscription, Window,
};
use rvw_view::files::Marks;
use rvw_view::finder::{self as picker, Hit};
use rvw_view::keys::Screen;

use crate::shell::Shell;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;

actions!(rvw_finder, [OpenFinder]);

/// Binds ⌘P (Ctrl-P off macOS) to open the finder. Call once at startup.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("secondary-p", OpenFinder, None)]);
}

/// An open finder: its field, the matches for the field's text, and which
/// match is highlighted. The subscriptions turn the field's Enter and Esc, and
/// its edits, into shell calls; they live as long as the finder does.
pub struct Finder {
    pub input: Entity<TextInput>,
    pub hits: Vec<Hit>,
    pub highlighted: usize,
    _events: Subscription,
    _typing: Subscription,
}

impl Shell {
    /// Opens the finder with focus in its field. Does nothing when it is open.
    pub fn open_finder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finder.is_some() {
            return;
        }
        self.tools.close_all();
        self.ensure_listing();
        let input = cx.new(|cx| TextInput::new("Find a file", cx));
        let events = cx.subscribe(
            &input,
            |this: &mut Self, _, event: &TextInputEvent, cx: &mut Context<Self>| match event {
                TextInputEvent::Submit => this.open_highlighted(cx),
                TextInputEvent::Cancel => this.close_finder(cx),
            },
        );
        let typing = cx.observe(&input, |this: &mut Self, _, cx: &mut Context<Self>| {
            this.refresh_hits(cx);
        });
        input.read(cx).focus(window);
        self.finder = Some(Finder {
            input,
            hits: Vec::new(),
            highlighted: 0,
            _events: events,
            _typing: typing,
        });
        self.refresh_hits(cx);
    }

    pub fn close_finder(&mut self, cx: &mut Context<Self>) {
        self.finder = None;
        cx.notify();
    }

    /// Re-ranks the files against the field's text and highlights the first row.
    fn refresh_hits(&mut self, cx: &mut Context<Self>) {
        let Some(finder) = self.finder.as_ref() else {
            return;
        };
        let query = finder.input.read(cx).text().to_owned();
        let marks = Marks::new(&self.snapshot.changes, &self.snapshot.comments);
        let hits = match &self.listing {
            Some(Ok(list)) => picker::hits(&query, &list.paths, &marks),
            _ => Vec::new(),
        };
        if let Some(finder) = self.finder.as_mut() {
            finder.hits = hits;
            finder.highlighted = 0;
        }
        cx.notify();
    }

    /// Down or up arrow: moves the highlight within the rows shown.
    pub fn move_highlight(&mut self, down: bool, cx: &mut Context<Self>) {
        if let Some(finder) = self.finder.as_mut() {
            let last = finder.hits.len().saturating_sub(1);
            finder.highlighted = if down {
                (finder.highlighted + 1).min(last)
            } else {
                finder.highlighted.saturating_sub(1)
            };
        }
        cx.notify();
    }

    fn open_highlighted(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self.finder.as_ref().map(|finder| finder.highlighted) {
            self.open_hit(index, cx);
        }
    }

    /// Opens the file of row `index`: the Files tab, its folders shown, and the
    /// file selected, as a click in the tree does. Closes the finder.
    pub fn open_hit(&mut self, index: usize, cx: &mut Context<Self>) {
        let path = self
            .finder
            .as_ref()
            .and_then(|finder| finder.hits.get(index))
            .map(|hit| hit.path.clone());
        let Some(path) = path else {
            return;
        };
        self.finder = None;
        self.show(Screen::Files, cx);
        if let Some(Ok(list)) = self.listing.as_mut() {
            list.tree.reveal(&path);
        }
        self.select_file(&path, cx);
    }
}

/// The finder over the window: a dimmed scrim with the card near the top.
pub fn overlay(finder: &Finder, cx: &Context<Shell>) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .justify_center()
        .pt(px(104.))
        .bg(rgba(0x1614_1433))
        .occlude()
        .child(card(finder, cx))
}

fn card(finder: &Finder, cx: &Context<Shell>) -> Div {
    div()
        .flex()
        .flex_col()
        .w(px(860.))
        .overflow_hidden()
        .rounded(theme::WINDOW_RADIUS)
        .bg(theme::paper())
        .border_1()
        .border_color(theme::hairline())
        .child(
            div()
                .flex()
                .items_center()
                .h(px(52.))
                .px(px(20.))
                .border_b_1()
                .border_color(theme::hairline())
                .child(div().flex_1().min_w(px(0.)).child(finder.input.clone())),
        )
        .child(results(finder, cx))
        .child(footer())
}

fn results(finder: &Finder, cx: &Context<Shell>) -> Div {
    let rows: Vec<Stateful<Div>> = finder
        .hits
        .iter()
        .enumerate()
        .map(|(index, hit)| hit_row(index, hit, index == finder.highlighted, cx))
        .collect();
    if rows.is_empty() {
        return div()
            .px(px(24.))
            .py(px(16.))
            .text_color(theme::muted())
            .child("No file matches.");
    }
    div().flex().flex_col().py(px(6.)).children(rows)
}

/// One match: the file name in mono with the matched characters in bold, the
/// directory in faint, and the open comment count when there is one.
fn hit_row(index: usize, hit: &Hit, highlighted: bool, cx: &Context<Shell>) -> Stateful<Div> {
    let name =
        div()
            .flex()
            .font_family(theme::code_font())
            .text_size(px(theme::CODE_FONT_SIZE))
            .text_color(theme::text())
            .children(picker::runs(&hit.name, &hit.name_matches).into_iter().map(
                |(text, matched)| {
                    div()
                        .when(matched, |d| {
                            d.font_weight(FontWeight::SEMIBOLD).text_color(theme::ink())
                        })
                        .child(text)
                },
            ));
    let mut label = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.))
        .gap(px(2.))
        .child(name);
    if !hit.dir.is_empty() {
        label = label.child(
            div()
                .truncate()
                .font_family(theme::code_font())
                .text_size(px(theme::KEY_FONT_SIZE))
                .text_color(theme::faint())
                .child(hit.dir.clone()),
        );
    }
    let on_click = cx.listener(
        move |this: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
            this.open_hit(index, cx);
        },
    );
    let mut row = div()
        .id(SharedString::from(format!("hit-{index}")))
        .flex()
        .items_center()
        .gap(px(10.))
        .h(px(44.))
        .mx(px(8.))
        .px(px(12.))
        .rounded(theme::ROW_RADIUS)
        .when(highlighted, |d| d.bg(theme::hairline()))
        .on_click(on_click)
        .child(label);
    if hit.mark.open_comments > 0 {
        row = row.child(
            div()
                .flex_none()
                .text_color(theme::pending())
                .child(format!("○ {}", hit.mark.open_comments)),
        );
    }
    row
}

fn footer() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .h(px(36.))
        .px(px(20.))
        .border_t_1()
        .border_color(theme::hairline())
        .font_family(theme::code_font())
        .text_size(px(theme::KEY_FONT_SIZE))
        .text_color(theme::faint())
        .child("↑↓ move")
        .child("↵ open")
        .child("esc close")
}
