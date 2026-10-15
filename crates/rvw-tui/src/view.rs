//! Port of internal/tui/view.go and the overlays of picker.go: the screen as
//! styled rows. Every row is a list of pieces, one list per terminal row, so a
//! background can span the pieces of a row.

use std::collections::{HashMap, HashSet};

use ratatui::style::{Color, Modifier, Style};
use rvw_core::types::{Comment, Status};
use rvw_render::terminal::safe;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::changes::Compare;
use crate::fuzzy::Ranked;
use crate::model::{Detail, FileView, Model, Pane, Picker, PickerKind, Sidebar};
use crate::pages::wrap;
use crate::rail::{covering, covers, rail_at};
use crate::style::{self, Piece, Sign, piece, width};
use crate::tree::{Node, Row};

/// The most rows the bottom bar's comment list takes.
const MAX_BAR: usize = 5;

/// The help overlay's rows: a heading has an empty key.
const HELP_KEYS: &[(&str, &str)] = &[
    ("", "Global"),
    ("<leader>p", "go to file"),
    ("<leader>l", "open comments"),
    ("Tab", "switch pane"),
    ("s", "submit a review"),
    ("t  b", "files/changes/reviews, compare"),
    ("r", "reload files, changes and comments"),
    ("q", "quit (everything is saved)"),
    ("", "Tree"),
    ("j/k  gg/G", "move"),
    ("l  ↵", "open file, expand directory"),
    ("h", "collapse, go to parent"),
    ("", "Viewer"),
    ("j/k  C-d/C-u", "move, half page"),
    ("gg/G  NG  :N", "first, last, line N"),
    ("]c  [c", "next, previous comment"),
    ("]h  [h", "next, previous change, around"),
    ("V  c", "select lines, comment on them"),
    ("o  esc", "review page: open file, close"),
    ("e", "edit the comment on this line"),
    ("", "Mouse"),
    ("click", "open file, toggle directory, move"),
    ("drag", "select lines"),
    ("", "Overlays"),
    ("C-n/C-p ↵ esc", "move, open, close"),
];

/// The submit menu's choices, in order.
const SUBMIT_LABELS: [&str; 3] = ["comment", "approve", "request changes"];

/// A count as the signed integer the line arithmetic uses.
fn num(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// `n` spaces in `st`.
fn spaces(n: usize, st: Style) -> Piece {
    piece(" ".repeat(n), st)
}

/// Draws pieces the way the viewer shows them: empty pieces are dropped,
/// control characters are replaced, and `bg` (when set) colors every piece.
fn render(ps: Vec<Piece>, bg: Option<Color>) -> Vec<Piece> {
    ps.into_iter()
        .filter(|p| !p.text.is_empty())
        .map(|p| Piece {
            text: safe(&p.text),
            style: bg.map_or(p.style, |c| p.style.bg(c)),
        })
        .collect()
}

/// Go's `ansi.Truncate`: `s` when it fits `w` columns; otherwise the longest
/// prefix that leaves room for `tail`, then `tail`.
fn truncate(s: &str, w: usize, tail: &str) -> String {
    if s.width() <= w {
        return s.to_owned();
    }
    let tw = tail.width();
    if tw > w {
        return String::new();
    }
    let limit = w - tw;
    let mut out = String::new();
    let mut cur = 0;
    for c in s.chars() {
        cur += c.width().unwrap_or(0);
        if cur > limit {
            out.push_str(tail);
            break;
        }
        out.push(c);
    }
    out
}

/// Go's `ansi.TruncateLeft`: `s` without its first `n` columns; the prefix is
/// written before the first kept character. `s` itself when `n` is zero.
fn drop_left(s: &str, n: usize, prefix: &str) -> String {
    if n == 0 {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut cur = 0;
    let mut kept = false;
    for c in s.chars() {
        cur += c.width().unwrap_or(0);
        if cur > n {
            if !kept {
                kept = true;
                out.push_str(prefix);
            }
            out.push(c);
        }
    }
    out
}

/// Pads or truncates plain text to exactly `w` columns.
fn fit(s: &str, w: usize) -> String {
    let sw = s.width();
    if sw <= w {
        return format!("{s}{}", " ".repeat(w - sw));
    }
    let t = truncate(s, w, "…");
    let pad = w.saturating_sub(t.width());
    format!("{t}{}", " ".repeat(pad))
}

/// Truncates pieces to `w` columns, marking the cut with "…", and pads them
/// to exactly `w`.
fn fit_pieces(ps: Vec<Piece>, w: usize) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut left = w;
    for p in ps {
        let pw = p.text.width();
        if pw <= left {
            left -= pw;
            out.push(p);
            continue;
        }
        let t = truncate(&p.text, left, "…");
        left -= t.width();
        out.push(Piece {
            text: t,
            style: p.style,
        });
        break;
    }
    out.push(spaces(left, style::plain()));
    out
}

/// A path cut from the left to fit `w` columns, keeping the file name and
/// preferring a directory boundary: "…/handlers/v2/x.go". Returns the text
/// and how many characters were dropped.
pub(crate) fn truncate_left(s: &str, w: i64) -> (String, usize) {
    let sw = i64::try_from(s.width()).unwrap_or(i64::MAX);
    if sw <= w {
        return (s.to_owned(), 0);
    }
    if w < 1 {
        return (String::new(), s.chars().count());
    }
    let rs: Vec<char> = s.chars().collect();
    let mut cut = 0;
    while cut < rs.len() {
        let rest: String = rs[cut..].iter().collect();
        if num(rest.width()) < w {
            break;
        }
        cut += 1;
    }
    if cut < rs.len()
        && rs[cut] != '/'
        && let Some(k) = rs[cut..].iter().position(|&c| c == '/')
    {
        cut += k;
    }
    let rest: String = rs[cut..].iter().collect();
    (format!("…{rest}"), cut)
}

/// The text with the characters at `pos` (char indices) styled as matches.
pub(crate) fn highlight_matches(text: &str, pos: &[usize]) -> Vec<Piece> {
    let set: HashSet<usize> = pos.iter().copied().collect();
    let mut out = Vec::new();
    let mut run = String::new();
    let mut matched = false;
    for (i, c) in text.chars().enumerate() {
        let is_match = set.contains(&i);
        if is_match != matched {
            flush_run(&mut out, &mut run, matched);
            matched = is_match;
        }
        run.push(c);
    }
    flush_run(&mut out, &mut run, matched);
    out
}

fn flush_run(out: &mut Vec<Piece>, run: &mut String, matched: bool) {
    if run.is_empty() {
        return;
    }
    let st = if matched {
        style::matched()
    } else {
        style::plain()
    };
    out.push(piece(std::mem::take(run), st));
}

/// "1 line", "3 lines": `n` and the word, pluralized.
fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// "5" for one line, "5-8" for a range.
fn line_range(lo: i64, hi: i64) -> String {
    if lo == hi {
        lo.to_string()
    } else {
        format!("{lo}-{hi}")
    }
}

/// Lines of a detail page for a width, computed on a copy so drawing needs
/// no mutable model.
fn detail_page(d: &Detail, w: usize) -> Vec<Vec<Piece>> {
    let mut d = d.clone();
    d.layout(w).clone()
}

/// A border segment `w` wide carrying a title: "─ files ─────".
fn pane_title(t: &str, w: usize, focused: bool) -> Vec<Piece> {
    let (t, _) = truncate_left(t, num(w) - 4);
    let st = if focused {
        style::title()
    } else {
        style::dim()
    };
    let rest = num(w) - 3 - num(t.width());
    vec![
        piece("─ ", style::border()),
        piece(t.clone(), st),
        piece(
            format!(" {}", "─".repeat(usize::try_from(rest).unwrap_or(0))),
            style::border(),
        ),
    ]
}

/// The comments covering the cursor line, the text wrapped under each header,
/// at most [`MAX_BAR`] rows.
fn comment_bar(cs: &[&Comment], w: usize) -> Vec<Vec<Piece>> {
    let mut rows: Vec<Vec<Piece>> = Vec::new();
    for c in cs {
        let mut head = vec![
            piece(c.id.clone(), style::id()),
            piece(
                format!(" · {}", line_range(c.start_line, c.end_line)),
                style::dim(),
            ),
        ];
        if let Some(a) = c.author.as_deref().filter(|a| !a.is_empty()) {
            head.push(piece(
                format!(" · @{}", style::expand_tabs(a)),
                style::dim(),
            ));
        }
        if c.status == Status::Pulled {
            head.push(piece(" (pulled)", style::dim()));
        }
        head.push(piece("   ", style::plain()));
        let hw = width(&head);
        let text_w = w.saturating_sub(hw).max(10);
        let body = safe(c.comment.trim());
        for (i, t) in wrap(&body, text_w).iter().enumerate() {
            let mut row = if i == 0 {
                head.clone()
            } else {
                vec![spaces(hw, style::plain())]
            };
            row.push(piece(style::expand_tabs(t), style::plain()));
            rows.push(row);
        }
    }
    if rows.len() > MAX_BAR {
        rows.truncate(MAX_BAR);
        let last = rows.pop().unwrap_or_default();
        let mut cut = fit_pieces(last, w.saturating_sub(2));
        cut.push(piece(" …", style::dim()));
        rows.push(cut);
    }
    rows.into_iter()
        .map(|r| render(fit_pieces(r, w), None))
        .collect()
}

/// Draws `boxed` over the dimmed rows of `bg`, centered horizontally and in
/// the upper third.
fn overlay(bg: &[Vec<Piece>], boxed: &[Vec<Piece>], w: usize) -> Vec<Vec<Piece>> {
    let bw = width(&boxed[0]);
    let x = w.saturating_sub(bw) / 2;
    let y = bg.len().saturating_sub(boxed.len()) / 3;
    bg.iter()
        .enumerate()
        .map(|(i, l)| {
            let plain: String = l.iter().map(|p| p.text.as_str()).collect();
            if i < y || i >= y + boxed.len() {
                return vec![piece(plain, style::faint())];
            }
            let left = fit(&truncate(&plain, x, ""), x);
            let right = drop_left(&plain, x + bw, "");
            let pad = w.saturating_sub(x + bw + right.width());
            let mut row = vec![piece(left, style::faint())];
            row.extend(boxed[i - y].iter().cloned());
            row.push(piece(format!("{}{right}", " ".repeat(pad)), style::faint()));
            row
        })
        .collect()
}

/// Boxes rows of pieces, each padded to the inner width `iw`, with a title
/// and an optional footer below a rule. A `None` row is a rule.
fn frame(
    title: &str,
    iw: usize,
    body: Vec<Option<Vec<Piece>>>,
    footer: Option<Vec<Piece>>,
    bg: &HashMap<usize, Color>,
) -> Vec<Vec<Piece>> {
    let edge = || piece("│", style::border());
    let rule = |l: &str, r: &str| vec![piece(format!("{l}{}{r}", "─".repeat(iw)), style::border())];
    let mut out = vec![vec![
        piece("┌─ ", style::border()),
        piece(title, style::title()),
        piece(
            format!(" {}┐", "─".repeat(iw.saturating_sub(3 + title.width()))),
            style::border(),
        ),
    ]];
    for (i, r) in body.into_iter().enumerate() {
        match r {
            None => out.push(rule("├", "┤")),
            Some(r) => {
                let mut row = vec![edge()];
                row.extend(render(fit_pieces(r, iw), bg.get(&i).copied()));
                row.push(edge());
                out.push(row);
            }
        }
    }
    if let Some(f) = footer {
        out.push(rule("├", "┤"));
        let mut row = vec![edge()];
        row.extend(render(fit_pieces(f, iw), None));
        row.push(edge());
        out.push(row);
    }
    out.push(rule("└", "┘"));
    out
}

/// The label of a picker's title.
fn picker_title(kind: PickerKind) -> &'static str {
    match kind {
        PickerKind::Comments => "Open comments",
        PickerKind::Files => "Go to file",
    }
}

/// One picker match: the marker, the id for a comment, the label with its
/// matched characters highlighted, and a file's comment count on the right.
fn picker_row(p: &Picker, r: &Ranked, selected: bool, iw: usize) -> Vec<Piece> {
    let marker = if selected { " ▸ " } else { "   " };
    let mut out = vec![piece(marker, style::title())];
    let c = &p.cands[r.index];
    let files = p.kind == PickerKind::Files;
    let count_badge = if files && c.comments > 0 {
        format!("💬{}", c.comments)
    } else {
        String::new()
    };
    let mut text_w = iw.saturating_sub(4);
    if files {
        text_w = text_w.saturating_sub(5);
    } else {
        let id = p.ids.get(r.index).map_or("", String::as_str);
        out.push(piece(format!("{id:<5} "), style::id()));
        text_w = text_w.saturating_sub(6);
    }
    let mut label = c.label.clone();
    let mut pos = r.positions.clone();
    if files {
        let (cut_label, cut) = truncate_left(&label, num(text_w));
        label = cut_label;
        if cut > 0 {
            pos = r
                .positions
                .iter()
                .filter(|&&q| q >= cut)
                .map(|&q| q - cut + 1)
                .collect();
        }
    }
    out.extend(fit_pieces(highlight_matches(&label, &pos), text_w));
    if files {
        out.push(spaces(5, style::plain()));
        if !count_badge.is_empty() {
            let last = out.len() - 1;
            let pad = 5usize.saturating_sub(count_badge.width());
            out[last] = piece(format!("{}{count_badge}", " ".repeat(pad)), style::badge());
        }
    }
    out
}

impl Model {
    /// The file tree pane's width: `clamp(width/4, 16, 36)`.
    #[must_use]
    pub fn tree_width(&self) -> usize {
        (self.width / 4).clamp(16, 36)
    }

    /// The viewer's width: `width - tree_width - 3`.
    #[must_use]
    pub fn view_width(&self) -> usize {
        self.width.saturating_sub(self.tree_width() + 3)
    }

    /// Rows of both panes: `max(1, height - 3 - bar().len())`.
    #[must_use]
    pub fn body_height(&self) -> usize {
        self.height.saturating_sub(3 + self.bar().len()).max(1)
    }

    /// The bottom bar's lines (flash, status or the comments on the cursor
    /// line); at most 5.
    #[must_use]
    pub fn bar(&self) -> Vec<Vec<Piece>> {
        let w = self.width.saturating_sub(4).max(1);
        let one = |ps: Vec<Piece>| vec![render(fit_pieces(ps, w), None)];
        if self.width == 0 {
            return vec![Vec::new()];
        }
        if !self.flash.is_empty() {
            let st = if self.flash_err {
                style::error()
            } else {
                style::notice()
            };
            return one(vec![piece(self.flash.clone(), st)]);
        }
        if let Some(g) = &self.go_line {
            return one(vec![piece(format!(":{g}▏"), style::plain())]);
        }
        if self.leader {
            return one(vec![
                piece("<leader>", style::mode()),
                piece(
                    "  p go to file · l open comments · esc cancel",
                    style::dim(),
                ),
            ]);
        }
        if self.visual && self.file.is_some() {
            let (lo, hi) = self.selection();
            return one(vec![
                piece("-- VISUAL --", style::mode()),
                piece(
                    format!(
                        "  {} ({}) · c comment · esc cancel",
                        line_range(num(lo), num(hi)),
                        plural((hi + 1).saturating_sub(lo), "line")
                    ),
                    style::dim(),
                ),
            ]);
        }
        if let Some(f) = self
            .file
            .as_ref()
            .filter(|f| !f.binary && self.focus == Pane::Viewer && self.detail.is_none())
        {
            let cs = covering(&f.comments, num(f.cursor + 1));
            if !cs.is_empty() {
                return comment_bar(&cs, w);
            }
        }
        vec![render(self.status_line(w), None)]
    }

    /// The whole terminal, one row of pieces per screen line. Too small a
    /// terminal draws a single `rvw: the terminal is too small` row.
    #[must_use]
    pub fn screen(&self) -> Vec<Vec<Piece>> {
        if self.width < 40 || self.height < 10 {
            return vec![vec![piece(
                "rvw: the terminal is too small",
                style::plain(),
            )]];
        }
        let mut lines = self.main_rows();
        let boxed = if let Some(p) = &self.picker {
            Some(self.picker_box(p))
        } else if let Some(s) = &self.submit {
            Some(self.submit_box(s.cursor))
        } else if let Some(c) = &self.compare {
            Some(self.compare_box(c.cursor))
        } else if self.help {
            Some(self.help_box())
        } else {
            None
        };
        if let Some(b) = boxed {
            lines = overlay(&lines, &b, self.width);
        }
        lines
    }

    /// The main layout: the two bordered panes, the bottom bar, and the frame.
    fn main_rows(&self) -> Vec<Vec<Piece>> {
        let tw = self.tree_width();
        let vw = self.view_width();
        let body = self.body_height();
        let title = match (&self.detail, &self.file) {
            (Some(d), _) => d.title(),
            (None, Some(f)) => f.rel.clone(),
            (None, None) => "no file".to_owned(),
        };
        let page = self.detail.as_ref().map(|d| detail_page(d, vw));
        let mut top = vec![piece("┌", style::border())];
        top.extend(pane_title(&self.side_title(), tw, self.focus == Pane::Tree));
        top.push(piece("┬", style::border()));
        top.extend(pane_title(&title, vw, self.focus == Pane::Viewer));
        top.push(piece("┐", style::border()));
        let mut out = vec![top];
        for i in 0..body {
            let mut row = vec![piece("│", style::border())];
            row.extend(self.tree_line(i, tw));
            row.push(piece("│", style::border()));
            row.extend(self.view_line(i, vw, page.as_deref()));
            row.push(piece("│", style::border()));
            out.push(row);
        }
        out.push(vec![piece(
            format!("├{}┴{}┤", "─".repeat(tw), "─".repeat(vw)),
            style::border(),
        )]);
        for b in self.bar() {
            let mut row = vec![piece("│ ", style::border())];
            row.extend(b);
            row.push(piece(" │", style::border()));
            out.push(row);
        }
        out.push(vec![piece(
            format!("└{}┘", "─".repeat(self.width.saturating_sub(2))),
            style::border(),
        )]);
        out
    }

    /// What the left pane is titled: its name, and for the changes, what they
    /// are against, last, so a narrow pane keeps it.
    fn side_title(&self) -> String {
        match self.side {
            Sidebar::Files => "files".to_owned(),
            Sidebar::Reviews => "reviews".to_owned(),
            Sidebar::Changes => {
                let against = if self.base_name.is_empty() {
                    self.cmp.label()
                } else {
                    self.base_name.as_str()
                };
                format!("changes vs {against}")
            }
        }
    }

    /// One row of the left pane: `i` rows below the top of the visible tree.
    fn tree_line(&self, i: usize, w: usize) -> Vec<Piece> {
        let idx = self.tree_off + i;
        let Some(row) = self.rows.get(idx).copied() else {
            if self.rows.is_empty() && i == 0 {
                let text = self.empty_tree_text();
                return render(
                    fit_pieces(vec![piece(format!(" {text}"), style::dim())], w),
                    None,
                );
            }
            return vec![spaces(w, style::plain())];
        };
        let node = self.tree().node(row.node);
        let bg = (idx == self.tree_cur && self.focus == Pane::Tree).then_some(self.theme.cursor);
        if self.side == Sidebar::Reviews {
            return self.review_tree_line(row, w, bg);
        }
        self.file_tree_line(row, node, w, bg)
    }

    /// What the left pane says when it has no rows.
    fn empty_tree_text(&self) -> String {
        match self.side {
            Sidebar::Reviews => "no reviews or comments yet".to_owned(),
            Sidebar::Changes if !self.chg_err.is_empty() => self.chg_err.clone(),
            Sidebar::Changes => "no changes".to_owned(),
            Sidebar::Files => "empty workspace".to_owned(),
        }
    }

    /// A row of the reviews side: the name, then the state mark at the right.
    fn review_tree_line(&self, row: Row, w: usize, bg: Option<Color>) -> Vec<Piece> {
        let (mut name, mark) = self.review_row(row.node);
        let path = &self.tree().node(row.node).path;
        if self.detail.as_ref().is_some_and(|d| d.id == *path) {
            for p in &mut name {
                p.style = p.style.add_modifier(Modifier::BOLD);
            }
        }
        let mut text = vec![piece(" ", style::plain())];
        text.extend(name);
        let mut row = fit_pieces(text, w.saturating_sub(3));
        row.push(piece(" ", style::plain()));
        row.push(mark);
        row.push(piece(" ", style::plain()));
        render(fit_pieces(row, w), bg)
    }

    /// A row of the files or changes side: the name, a comment count and the
    /// git status letter.
    fn file_tree_line(&self, row: Row, node: &Node, w: usize, bg: Option<Color>) -> Vec<Piece> {
        let indent = "  ".repeat(row.depth);
        let is_current = self.file.as_ref().is_some_and(|f| f.rel == node.path);
        let mut name = if node.dir {
            let arrow = if node.expanded { "▾" } else { "▸" };
            piece(format!("{arrow} {}/", node.name), style::dir())
        } else if is_current {
            piece(format!("  {}", node.name), style::current())
        } else {
            piece(format!("  {}", node.name), style::plain())
        };
        let count = self.counts.get(&node.path).copied().unwrap_or(0);
        let badge = if count > 0 && !node.dir {
            format!("💬{count}")
        } else {
            String::new()
        };
        let status = if self.side == Sidebar::Changes && !node.dir {
            self.status.get(&node.path).copied().unwrap_or(0)
        } else {
            0
        };
        if status == b'D' {
            name.style = style::dim();
        }
        let mut avail = num(w) - 2;
        if !badge.is_empty() {
            avail -= num(badge.width()) + 1;
        }
        if status != 0 {
            avail -= 2;
        }
        let name_w = usize::try_from(avail + 1).unwrap_or(0);
        let mut out = fit_pieces(
            vec![piece(format!(" {indent}"), style::plain()), name],
            name_w,
        );
        out.push(piece(" ", style::plain()));
        if !badge.is_empty() {
            out.push(piece(badge, style::badge()));
            out.push(piece(" ", style::plain()));
        }
        if status != 0 {
            out.push(piece(
                char::from(status).to_string(),
                style::status_style(status),
            ));
            out.push(piece(" ", style::plain()));
        }
        render(fit_pieces(out, w), bg)
    }

    /// One row of the viewer: `i` rows below the top of the shown page or
    /// file, with its line number, rail and change sign.
    fn view_line(&self, i: usize, w: usize, page: Option<&[Vec<Piece>]>) -> Vec<Piece> {
        if let (Some(d), Some(lines)) = (&self.detail, page) {
            return match lines.get(d.offset + i) {
                Some(l) => render(fit_pieces(l.clone(), w), None),
                None => vec![spaces(w, style::plain())],
            };
        }
        let Some(f) = &self.file else {
            return if i == 0 {
                render(
                    fit_pieces(
                        vec![piece(" No file open. <leader>p to find one.", style::dim())],
                        w,
                    ),
                    None,
                )
            } else {
                vec![spaces(w, style::plain())]
            };
        };
        if f.binary {
            return if i == 0 {
                render(
                    fit_pieces(
                        vec![piece(" binary file: nothing to show", style::dim())],
                        w,
                    ),
                    None,
                )
            } else {
                vec![spaces(w, style::plain())]
            };
        }
        let idx = f.offset + i;
        let Some(src) = f.lines.get(idx) else {
            return if f.lines.is_empty() && i == 0 {
                render(
                    fit_pieces(vec![piece(" empty file", style::dim())], w),
                    None,
                )
            } else {
                vec![spaces(w, style::plain())]
            };
        };
        self.file_line(f, idx, src, w)
    }

    /// A line of the open file, with its gutter and text.
    fn file_line(
        &self,
        f: &FileView,
        idx: usize,
        src: &[crate::highlight::Segment],
        w: usize,
    ) -> Vec<Piece> {
        let line = idx + 1;
        let num_width = f.lines.len().to_string().len().max(3);
        let cursor = f.cursor + 1;
        let num_st = if line == cursor {
            style::line_nr_current()
        } else {
            style::line_nr()
        };
        let (mut glyph, mut rail_st) = (" ", style::plain());
        if let Some((c, g)) = rail_at(&f.comments, num(line)) {
            glyph = g;
            rail_st = if covers(c, num(cursor)) {
                style::rail_on()
            } else if c.status == Status::Pulled {
                style::rail_away()
            } else {
                style::rail_off()
            };
        }
        let sign = f.signs.get(idx).copied().unwrap_or(Sign::None);
        let mut row = vec![
            piece(sign.glyph(), style::sign_style(sign)),
            piece(format!("{line:>num_width$} "), num_st),
            piece(glyph, rail_st),
            piece(" ", style::plain()),
        ];
        row.extend(src.iter().map(|seg| piece(seg.text.clone(), seg.style)));
        let (lo, hi) = self.selection();
        let bg = if self.visual && (lo..=hi).contains(&line) {
            Some(self.theme.selection)
        } else if line == cursor && self.focus == Pane::Viewer {
            Some(self.theme.cursor)
        } else {
            None
        };
        render(fit_pieces(row, w), bg)
    }

    /// The status line: the branch and the uncommitted changes on the left,
    /// the way to the key list on the right; a long branch name is what gets
    /// cut.
    fn status_line(&self, w: usize) -> Vec<Piece> {
        let d = &self.wd;
        let counts = if !d.err.is_empty() {
            vec![piece(d.err.clone(), style::dim())]
        } else if !d.dirty {
            vec![piece("clean", style::dim())]
        } else {
            // A zero count says nothing: +3 alone, not +3 -0.
            let mut v = vec![piece("uncommitted", style::dim())];
            if d.added > 0 {
                v.push(piece(
                    format!(" +{}", d.added),
                    style::sign_style(Sign::Added),
                ));
            }
            if d.deleted > 0 {
                v.push(piece(
                    format!(" -{}", d.deleted),
                    style::sign_style(Sign::Deleted),
                ));
            }
            v
        };
        let help = piece("? help", style::dim());
        let help_w = help.text.width();
        let mut left = Vec::new();
        if !self.branch.is_empty() {
            // " · ", and a space before help.
            let branch_w = w
                .saturating_sub(width(&counts))
                .saturating_sub(help_w)
                .saturating_sub(4)
                .max(1);
            left = fit_pieces(
                vec![piece(format!("⎇ {}", self.branch), style::title())],
                branch_w,
            );
            left.pop(); // fit_pieces' padding
            left.push(piece(" · ", style::dim()));
        }
        left.extend(counts);
        let gap = w.saturating_sub(width(&left)).saturating_sub(help_w).max(1);
        left.push(spaces(gap, style::plain()));
        left.push(help);
        fit_pieces(left, w)
    }

    /// The text of the leader key, as the help overlay names it.
    fn leader_label(&self) -> &str {
        if self.opts.leader.is_empty() {
            "space"
        } else {
            &self.opts.leader
        }
    }

    fn picker_box(&self, p: &Picker) -> Vec<Vec<Piece>> {
        let bw = self.width.saturating_sub(8).clamp(30, 90);
        let iw = bw - 2;
        let mut body: Vec<Option<Vec<Piece>>> = vec![
            Some(vec![
                piece(" > ", style::title()),
                piece(p.query.clone(), style::plain()),
                piece("▏", style::dim()),
            ]),
            None,
        ];
        let mut bg = HashMap::new();
        for i in 0..self.picker_rows() {
            let idx = p.offset + i;
            let Some(m) = p.matches.get(idx) else {
                body.push(Some(Vec::new()));
                continue;
            };
            if idx == p.cursor {
                bg.insert(body.len(), self.theme.cursor);
            }
            body.push(Some(picker_row(p, m, idx == p.cursor, iw)));
        }
        let hint = "↵ open · C-n/C-p · esc ";
        let count = format!(" {}/{}", p.matches.len(), p.cands.len());
        let gap = iw
            .saturating_sub(count.width())
            .saturating_sub(hint.width())
            .max(1);
        let footer = vec![
            piece(count, style::dim()),
            spaces(gap, style::plain()),
            piece(hint, style::dim()),
        ];
        frame(picker_title(p.kind), iw, body, Some(footer), &bg)
    }

    fn submit_box(&self, cursor: usize) -> Vec<Vec<Piece>> {
        let mut body = Vec::new();
        let mut bg = HashMap::new();
        for (i, l) in SUBMIT_LABELS.iter().enumerate() {
            let marker = if i == cursor { " ▸ " } else { "   " };
            if i == cursor {
                bg.insert(i, self.theme.cursor);
            }
            body.push(Some(vec![
                piece(marker, style::title()),
                piece((i + 1).to_string(), style::id()),
                piece(format!("  {l}"), style::plain()),
            ]));
        }
        let n = self.my_pending().len();
        let footer = vec![piece(
            format!(" links {} · ↵ · esc ", plural(n, "pending comment")),
            style::dim(),
        )];
        frame("Submit review", 36, body, Some(footer), &bg)
    }

    fn compare_box(&self, cursor: usize) -> Vec<Vec<Piece>> {
        let mut body = Vec::new();
        let mut bg = HashMap::new();
        for (i, c) in [Compare::Head, Compare::Default, Compare::Prev]
            .into_iter()
            .enumerate()
        {
            let marker = if i == cursor { " ▸ " } else { "   " };
            if i == cursor {
                bg.insert(i, self.theme.cursor);
            }
            body.push(Some(vec![
                piece(marker, style::title()),
                piece((i + 1).to_string(), style::id()),
                piece(format!("  {}", c.label()), style::plain()),
            ]));
        }
        let footer = vec![piece(
            " lists changes, marks the gutter · ↵ · esc ",
            style::dim(),
        )];
        frame("Compare against", 44, body, Some(footer), &bg)
    }

    fn help_box(&self) -> Vec<Vec<Piece>> {
        let mut body: Vec<Option<Vec<Piece>>> = Vec::new();
        for &(k, v) in HELP_KEYS {
            if k.is_empty() {
                let mut head = vec![piece(format!(" {v}"), style::title())];
                if v == "Global" {
                    head.push(piece(
                        format!("  <leader> is {}", self.leader_label()),
                        style::dim(),
                    ));
                }
                body.push(Some(head));
            } else {
                body.push(Some(vec![
                    piece(format!("   {k:<14}"), style::id()),
                    piece(v, style::plain()),
                ]));
            }
        }
        body.truncate(self.height.saturating_sub(5));
        frame(
            "Keys",
            50,
            body,
            Some(vec![piece(" any key closes", style::dim())]),
            &HashMap::new(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Event;
    use crate::testing::{Fixture, PARSE_PY, keys, screen_text, setup};
    use rvw_core::operations::{PullInput, ResolveInput, SubmitInput};

    fn text_of(rows: &[Vec<Piece>]) -> Vec<String> {
        rows.iter()
            .map(|r| r.iter().map(|p| p.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn fit_pads_and_truncates_by_columns() {
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("中文字", 4), "中… ");
        assert_eq!(fit("x", 0), "");
    }

    #[test]
    fn fit_pieces_cuts_the_row_and_pads_it() {
        let row = vec![piece("ab", style::dim()), piece("cdef", style::plain())];
        let got = fit_pieces(row.clone(), 4);
        assert_eq!(text_of(&[got]), ["abc…"]);
        assert_eq!(width(&fit_pieces(row, 9)), 9);
    }

    #[test]
    fn truncate_left_keeps_the_file_name_at_a_directory_boundary() {
        assert_eq!(
            truncate_left("handlers/v2/x.go", 12),
            ("…/v2/x.go".to_owned(), 8)
        );
        assert_eq!(truncate_left("a.go", 10), ("a.go".to_owned(), 0));
        assert_eq!(truncate_left("abc", 0), (String::new(), 3));
    }

    #[test]
    fn highlight_matches_styles_only_the_matched_runs() {
        let got = highlight_matches("parse.py", &[0, 1, 5]);
        let texts: Vec<&str> = got.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["pa", "rse", ".", "py"]);
        assert_eq!(got[0].style, style::matched());
        assert_eq!(got[1].style, style::plain());
        assert_eq!(got[2].style, style::matched());
    }

    #[test]
    fn overlay_centers_the_box_and_dims_the_rest() {
        let bg = vec![vec![piece("0123456789", style::plain())]; 4];
        let boxed = vec![vec![piece("[ab]", style::plain())]];
        let got = overlay(&bg, &boxed, 10);
        assert_eq!(
            text_of(&got),
            ["0123456789", "012[ab]789", "0123456789", "0123456789"]
        );
    }

    #[test]
    fn frame_boxes_rows_with_title_rule_and_footer() {
        let body = vec![Some(vec![piece("hi", style::plain())]), None];
        let got = frame(
            "T",
            6,
            body,
            Some(vec![piece("f", style::plain())]),
            &HashMap::new(),
        );
        assert_eq!(
            text_of(&got),
            [
                "┌─ T ──┐",
                "│hi    │",
                "├──────┤",
                "├──────┤",
                "│f     │",
                "└──────┘",
            ]
        );
    }

    #[test]
    fn a_narrow_terminal_draws_one_notice_row() {
        let f = setup();
        let mut m = f.model();
        m.update(Event::Resize {
            width: 39,
            height: 30,
        });
        assert_eq!(screen_text(&m), "rvw: the terminal is too small");
    }

    /// `commented` in the Go golden tests: two comments on parse.py.
    fn commented() -> Fixture {
        let f = setup();
        f.add(
            "src/api/parse.py",
            5,
            8,
            "extract this branch into parse_x()",
            "gustavo",
        );
        f.add("src/api/parse.py", 12, 12, "typo", "reviewer");
        f
    }

    /// `gitFixture` in `changes_test.go`: parse.py changed against HEAD, and a
    /// new file.
    fn git_fixture() -> Fixture {
        let f = setup();
        f.write("src/api/parse.py", &format!("# header\n{PARSE_PY}"));
        f.git(&["commit", "-qam", "header"]);
        f.write(
            "src/api/parse.py",
            "import parse\n\n\ndef handle(req):\n    if req.kind == \"y\":\n        body = req.body\n        log(body)\n        return parse(body)\n\n\ndef default(req):\n\treturn None\n",
        );
        f.write("src/new.py", "a = 1\nb = 2\n");
        f
    }

    /// `reviewed` in `reviews_test.go`: two submitted reviews, pulled, with one
    /// comment resolved as done and one as rejected.
    fn reviewed() -> Fixture {
        let f = setup();
        let ws = f.ws.clone();
        f.add(
            "src/api/parse.py",
            5,
            8,
            "extract this branch into parse_x()",
            "reviewer",
        );
        f.add("src/util.py", 2, 2, "typo", "reviewer");
        let submit = |decision: &str, summary: &str, no_comments: bool| SubmitInput {
            workspace: ws.clone(),
            decision: decision.to_owned(),
            summary: summary.to_owned(),
            no_comments,
            author: "reviewer".to_owned(),
            ..SubmitInput::default()
        };
        f.svc
            .submit(submit("request-changes", "Fix both before merging.", false))
            .expect("submit");
        f.svc
            .submit(submit("approve", "Docs look good.", true))
            .expect("submit");
        f.add("README.md", 1, 1, "title case", "gustavo");
        f.svc
            .pull(PullInput {
                workspace: ws.clone(),
                ..PullInput::default()
            })
            .expect("pull");
        f.add("src/util.py", 1, 1, "name it better", "gustavo");
        f.write(
            "src/api/parse.py",
            &PARSE_PY.replacen(
                "        body = req.body\n        return parse(body)\n",
                "        return parse_x(req)\n",
                1,
            ),
        );
        let resolve = |id: &str, outcome: &str, note: &str| ResolveInput {
            workspace: ws.clone(),
            id: id.to_owned(),
            outcome: outcome.to_owned(),
            note: note.to_owned(),
            author: "claude".to_owned(),
        };
        f.svc
            .resolve(resolve("r1", "done", "extracted parse_x()"))
            .expect("resolve");
        f.svc
            .resolve(resolve("r2", "rejected", "not a typo: pass is intended"))
            .expect("resolve");
        f
    }

    #[test]
    fn golden_main_view() {
        let f = commented();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "apipar", "enter", "6G"]);
        assert_eq!(
            screen_text(&m),
            include_str!("../testdata/TestGoldenMainView.golden")
        );
    }

    #[test]
    fn golden_visual_mode() {
        let f = commented();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "apipar", "enter", "2G", "V", "2j"]);
        assert_eq!(
            screen_text(&m),
            include_str!("../testdata/TestGoldenVisualMode.golden")
        );
    }

    #[test]
    fn golden_quick_open() {
        let f = commented();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "apipar"]);
        assert_eq!(
            screen_text(&m),
            include_str!("../testdata/TestGoldenQuickOpen.golden")
        );
    }

    #[test]
    fn golden_comment_picker() {
        let f = commented();
        let mut m = f.model();
        keys(&mut m, &["leader", "l"]);
        assert_eq!(
            screen_text(&m),
            include_str!("../testdata/TestGoldenCommentPicker.golden")
        );
    }

    #[test]
    fn golden_changes_view() {
        let f = git_fixture();
        let mut m = f.model();
        keys(&mut m, &["t", "tab", "b"]);
        assert_eq!(
            screen_text(&m),
            include_str!("../testdata/TestGoldenChangesView.golden")
        );
    }

    #[test]
    fn golden_git_changes() {
        let f = git_fixture();
        let mut m = f.model();
        keys(&mut m, &["leader", "p", "apipar", "enter"]);
        assert_eq!(
            screen_text(&m),
            include_str!("../testdata/TestGoldenGitChanges.golden")
        );
    }

    #[test]
    fn golden_review_page() {
        let f = reviewed();
        let mut m = f.model();
        keys(&mut m, &["t", "t", "tab", "j", "j", "l"]);
        assert_eq!(
            screen_text(&m),
            include_str!("../testdata/TestGoldenReviewPage.golden")
        );
    }
}
