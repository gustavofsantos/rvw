//! The Export audit card: a centred card with one field, the path of the
//! audit file, prefilled with `audit-<branch>.md` in the workspace. It
//! states the range it covers and that every lane is in it. Enter writes the
//! file through `export_file`, Esc closes. A file that exists is refused, and
//! a second Enter on the same path overwrites it.

use std::path::Path;

use gpui::prelude::*;
use gpui::{Context, Div, Entity, Subscription, Window, div, px};
use rvw_git::gitx;

use crate::export_file::{self, EXISTS, Format};
use crate::shell::Shell;
use crate::text_input::{TextInput, TextInputEvent};
use crate::theme;
use crate::tools;

/// An open card. Its path is read from the field when Enter is pressed.
pub struct ExportCard {
    input: Entity<TextInput>,
    /// The range the export covers, as the card names it.
    range: String,
    /// The typed path that was refused for existing; Enter again on it
    /// overwrites.
    armed: Option<String>,
    /// The refusal or failure, one line.
    error: Option<String>,
    _events: Subscription,
}

impl Shell {
    /// Opens the card with focus in its field, closing any other picker. A
    /// second call while it is open does nothing.
    pub fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools.export.is_some() {
            return;
        }
        self.tools.close_all();
        self.finder = None;
        self.git.ask = None;
        let workspace = self.identity.workspace.clone();
        let dir = Path::new(&workspace);
        let branch = gitx::branch(dir);
        let default = gitx::default_branch(dir);
        let range = format!("{}..HEAD", default.as_deref().unwrap_or("<default>"));
        let proposed = export_file::target(&workspace, &export_file::default_file_name(&branch));
        let input = cx.new(|cx| TextInput::new("Path of the audit file", cx));
        input.update(cx, |field, cx| {
            field.set_text(&proposed.to_string_lossy(), cx);
        });
        let events = cx.subscribe(
            &input,
            |this: &mut Self, _, event: &TextInputEvent, cx: &mut Context<Self>| match event {
                TextInputEvent::Submit => this.write_export(cx),
                TextInputEvent::Cancel => this.close_export(cx),
            },
        );
        input.read(cx).focus(window);
        self.tools.export = Some(ExportCard {
            input,
            range,
            armed: None,
            error: None,
            _events: events,
        });
        cx.notify();
    }

    pub fn close_export(&mut self, cx: &mut Context<Self>) {
        self.tools.export = None;
        cx.notify();
    }

    /// Enter: writes the file the field names. An existing file is refused
    /// the first time; the same path again overwrites it. Success closes the
    /// card and says what was written on the status line.
    fn write_export(&mut self, cx: &mut Context<Self>) {
        let workspace = self.identity.workspace.clone();
        let Some(card) = self.tools.export.as_mut() else {
            return;
        };
        let typed = card.input.read(cx).text().trim().to_owned();
        let target = export_file::target(&workspace, &typed);
        let overwrite = target.exists() && card.armed.as_deref() == Some(typed.as_str());
        if target.exists() && !overwrite {
            card.armed = Some(typed);
            card.error = Some(EXISTS.to_owned());
            cx.notify();
            return;
        }
        match export_file::export(&self.service, &workspace, &typed, overwrite) {
            Ok(path) => {
                self.tools.export = None;
                self.git.network_result = Some(format!("Wrote {}", path.display()));
            }
            Err(message) => {
                if let Some(card) = self.tools.export.as_mut() {
                    card.armed = None;
                    card.error = Some(message);
                }
            }
        }
        cx.notify();
    }
}

pub fn overlay(card: &ExportCard, cx: &Context<Shell>) -> Div {
    let typed = card.input.read(cx).text().to_owned();
    let mut facts = vec![format!("Range {}", card.range), "all lanes".to_owned()];
    if let Ok(format) = Format::from_path(Path::new(&typed)) {
        facts.push(format.name().to_owned());
    }
    let mut body = div().flex().flex_col().px(px(24.)).py(px(12.)).gap(px(6.));
    body = body.child(
        div()
            .text_size(px(theme::KEY_FONT_SIZE))
            .text_color(theme::muted())
            .child(facts.join(" \u{00b7} ")),
    );
    if let Some(error) = card.error.as_deref() {
        body = body.child(div().text_color(theme::rejected()).child(error.to_owned()));
    }
    tools::overlay(tools::card(
        div().child(card.input.clone()),
        body,
        &["\u{21b5} write", "esc close"],
    ))
}
