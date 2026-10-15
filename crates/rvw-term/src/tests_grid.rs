//! Scrollback, search, selection, mouse encoding and URLs, on grids built by
//! feeding bytes to the emulation. No PTY.

use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

use crate::scroll::{self, Scroll};
use crate::search::Search;
use crate::select::{self, GridPoint, SelectionKind, Side};
use crate::snapshot::capture;
use crate::terminal::Size;
use crate::{
    MAX_MATCHES, Mode, Modifiers, MouseButton, MouseEvent, MouseEventKind, Snapshot, Wheel,
    encode_alternate_scroll, encode_mouse,
};

struct Grid {
    term: Term<VoidListener>,
    parser: Processor<StdSyncHandler>,
}

impl Grid {
    fn new(cols: usize, rows: usize, history: usize) -> Self {
        let config = Config {
            scrolling_history: history,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &Size { cols, rows }, VoidListener),
            parser: Processor::default(),
        }
    }

    fn feed(&mut self, text: &str) {
        self.parser.advance(&mut self.term, text.as_bytes());
    }

    fn lines(&mut self, from: usize, to: usize) {
        for n in from..=to {
            self.feed(&format!("line{n}\r\n"));
        }
    }

    fn snap(&self, search: Option<&mut Search>) -> Snapshot {
        capture(&self.term, search)
    }

    fn scroll(&mut self, scroll: Scroll) {
        scroll::apply(&mut self.term, scroll);
    }
}

fn pt(row: usize, col: usize) -> GridPoint {
    GridPoint { row, col }
}

#[test]
fn viewport_scrolls_by_lines_pages_and_ends() {
    let mut g = Grid::new(10, 4, 100);
    g.lines(1, 20);
    let live = g.snap(None);
    assert_eq!(live.display_offset, 0);
    assert_eq!(live.history_lines, 17);
    assert!(live.row_text(2).starts_with("line20") || live.text().contains("line20"));

    g.scroll(Scroll::Up(2));
    let s = g.snap(None);
    assert_eq!(s.display_offset, 2);
    assert_eq!(s.row_text(0), "line16");
    assert!(!s.text().contains("line20"));

    g.scroll(Scroll::PageUp);
    assert_eq!(g.snap(None).display_offset, 6);
    g.scroll(Scroll::PageDown);
    assert_eq!(g.snap(None).display_offset, 2);
    g.scroll(Scroll::Down(1));
    assert_eq!(g.snap(None).display_offset, 1);
    g.scroll(Scroll::Top);
    let top = g.snap(None);
    assert_eq!(top.display_offset, top.history_lines);
    assert_eq!(top.row_text(0), "line1");
    g.scroll(Scroll::Up(1000));
    assert_eq!(g.snap(None).display_offset, 17);
    g.scroll(Scroll::Bottom);
    assert_eq!(g.snap(None).display_offset, 0);
}

#[test]
fn output_while_scrolled_keeps_the_content_in_view() {
    let mut g = Grid::new(10, 4, 100);
    g.lines(1, 20);
    g.scroll(Scroll::Up(3));
    let before = g.snap(None);
    g.lines(21, 25);
    let after = g.snap(None);
    assert_eq!(after.display_offset, 3 + 5);
    assert_eq!(after.row_text(0), before.row_text(0));
}

#[test]
fn scrolled_past_the_cap_the_offset_stays_in_range() {
    let mut g = Grid::new(10, 4, 5);
    g.lines(1, 20);
    g.scroll(Scroll::Top);
    g.lines(21, 40);
    let s = g.snap(None);
    assert_eq!(s.history_lines, 5);
    assert!(s.display_offset <= 5);
}

#[test]
fn alt_screen_has_no_scrollback() {
    let mut g = Grid::new(10, 4, 100);
    g.lines(1, 20);
    g.feed("\x1b[?1049h");
    g.lines(1, 10);
    g.scroll(Scroll::Up(5));
    let s = g.snap(None);
    assert!(s.mode.alt_screen);
    assert_eq!(s.display_offset, 0);
    assert_eq!(s.history_lines, 0);
    g.feed("\x1b[?1049l");
    assert_eq!(g.snap(None).history_lines, 17);
}

#[test]
fn scrolled_cursor_is_hidden_when_below_the_viewport() {
    let mut g = Grid::new(10, 4, 100);
    g.lines(1, 20);
    assert!(g.snap(None).cursor.visible);
    g.scroll(Scroll::Up(10));
    assert!(!g.snap(None).cursor.visible);
}

fn search(g: &mut Grid, query: &str) -> Search {
    let mut s = Search::new(query).expect("query");
    s.recount(&g.term);
    s
}

#[test]
fn search_counts_across_history_and_is_case_smart() {
    let mut g = Grid::new(20, 4, 100);
    g.feed("Foo foo\r\nbar\r\nFOO\r\n");
    g.lines(1, 10);
    let lower = search(&mut g, "foo");
    assert_eq!(lower.status().count, 3);
    let upper = search(&mut g, "Foo");
    assert_eq!(upper.status().count, 1);
}

#[test]
fn search_text_is_literal() {
    let mut g = Grid::new(20, 4, 100);
    g.feed("a.c abc a+c\r\n");
    assert_eq!(search(&mut g, "a.c").status().count, 1);
    assert_eq!(search(&mut g, "a+c").status().count, 1);
    assert!(Search::new("").is_none());
    assert!(Search::new(&"x".repeat(2000)).is_none());
}

#[test]
fn search_steps_scroll_the_match_into_view_and_wrap() {
    let mut g = Grid::new(20, 4, 100);
    g.feed("needle one\r\n");
    g.lines(1, 20);
    g.feed("needle two\r\n");
    g.lines(21, 25);
    let mut s = search(&mut g, "needle");
    assert_eq!(s.status().count, 2);
    assert_eq!(s.status().current, None);

    assert!(s.step(&mut g.term, true));
    let snap = g.snap(Some(&mut s));
    assert_eq!(snap.search.as_ref().unwrap().current, Some(0));
    assert!(snap.display_offset > 0);
    let current: Vec<_> = snap.matches.iter().filter(|m| m.current).collect();
    assert_eq!(current.len(), 1);
    assert_eq!(snap.row_text(current[0].row), "needle one");
    assert_eq!((current[0].start, current[0].end), (0, 5));

    assert!(s.step(&mut g.term, true));
    assert_eq!(s.status().current, Some(1));
    assert!(s.step(&mut g.term, true));
    assert_eq!(s.status().current, Some(0));
    assert!(s.step(&mut g.term, false));
    assert_eq!(s.status().current, Some(1));
}

#[test]
fn search_without_a_match_steps_nowhere() {
    let mut g = Grid::new(20, 4, 100);
    g.lines(1, 5);
    let mut s = search(&mut g, "zzz");
    assert!(!s.step(&mut g.term, true));
    assert_eq!(s.status().count, 0);
}

#[test]
fn focus_follows_history_growth() {
    let mut g = Grid::new(20, 4, 100);
    g.feed("needle\r\n");
    g.lines(1, 6);
    let mut s = search(&mut g, "needle");
    assert!(s.step(&mut g.term, true));
    g.lines(7, 12);
    s.recount(&g.term);
    assert_eq!(s.status().current, Some(0));
}

#[test]
fn match_count_is_capped() {
    let mut g = Grid::new(40, 4, MAX_MATCHES * 2);
    for _ in 0..(MAX_MATCHES + 50) {
        g.feed("zz\r\n");
    }
    let s = search(&mut g, "zz");
    assert_eq!(s.status().count, MAX_MATCHES);
    assert!(s.status().capped);
}

#[test]
fn visible_matches_cover_wrapped_rows() {
    let mut g = Grid::new(5, 4, 10);
    g.feed("abcdefgh");
    let mut s = search(&mut g, "cdef");
    let spans = s.visible(&g.term);
    assert_eq!(spans.len(), 2);
    assert_eq!((spans[0].row, spans[0].start, spans[0].end), (0, 2, 4));
    assert_eq!((spans[1].row, spans[1].start, spans[1].end), (1, 0, 0));
}

#[test]
fn simple_selection_text_and_spans() {
    let mut g = Grid::new(20, 4, 10);
    g.feed("hello world\r\nsecond line\r\n");
    select::start(&mut g.term, SelectionKind::Simple, pt(0, 6), Side::Left);
    select::update(&mut g.term, pt(1, 5), Side::Right);
    assert_eq!(select::text(&g.term).as_deref(), Some("world\nsecond"));
    let s = g.snap(None);
    assert_eq!(s.selection.len(), 2);
    assert_eq!((s.selection[0].row, s.selection[0].start), (0, 6));
    assert_eq!((s.selection[0].end, s.selection[1].end), (19, 5));
    g.term.selection = None;
    assert!(g.snap(None).selection.is_empty());
    assert_eq!(select::text(&g.term), None);
}

#[test]
fn word_and_line_selection() {
    let mut g = Grid::new(20, 4, 10);
    g.feed("hello world\r\nnext\r\n");
    select::start(&mut g.term, SelectionKind::Word, pt(0, 8), Side::Left);
    assert_eq!(select::text(&g.term).as_deref(), Some("world"));
    select::start(&mut g.term, SelectionKind::Line, pt(0, 3), Side::Left);
    assert_eq!(select::text(&g.term).as_deref(), Some("hello world\n"));
    select::update(&mut g.term, pt(1, 1), Side::Left);
    assert_eq!(
        select::text(&g.term).as_deref(),
        Some("hello world\nnext\n")
    );
}

#[test]
fn block_selection_takes_a_rectangle_and_trims_blanks() {
    let mut g = Grid::new(20, 4, 10);
    g.feed("abcdef\r\nghijkl\r\nmn\r\n");
    select::start(&mut g.term, SelectionKind::Block, pt(0, 1), Side::Left);
    select::update(&mut g.term, pt(2, 3), Side::Right);
    assert_eq!(select::text(&g.term).as_deref(), Some("bcd\nhij\nn"));
    let spans = g.snap(None).selection;
    assert!(spans.iter().all(|s| (s.start, s.end) == (1, 3)));
}

#[test]
fn selection_joins_wrapped_lines_and_keeps_wide_chars() {
    let mut g = Grid::new(5, 4, 10);
    g.feed("abcdefgh\r\n");
    select::start(&mut g.term, SelectionKind::Simple, pt(0, 0), Side::Left);
    select::update(&mut g.term, pt(1, 2), Side::Right);
    assert_eq!(select::text(&g.term).as_deref(), Some("abcdefgh"));

    let mut w = Grid::new(10, 3, 10);
    w.feed("a\u{4e16}\u{754c}b\r\n");
    select::start(&mut w.term, SelectionKind::Simple, pt(0, 0), Side::Left);
    select::update(&mut w.term, pt(0, 5), Side::Right);
    assert_eq!(select::text(&w.term).as_deref(), Some("a\u{4e16}\u{754c}b"));
    // Ending on the spacer half still takes the whole character.
    select::start(&mut w.term, SelectionKind::Simple, pt(0, 1), Side::Left);
    select::update(&mut w.term, pt(0, 2), Side::Right);
    assert_eq!(select::text(&w.term).as_deref(), Some("\u{4e16}"));
}

#[test]
fn selection_follows_scrolling_and_is_viewport_relative() {
    let mut g = Grid::new(10, 4, 100);
    g.lines(1, 6);
    // Rows now hold line4, line5, line6 and a blank.
    select::start(&mut g.term, SelectionKind::Line, pt(0, 0), Side::Left);
    assert_eq!(select::text(&g.term).as_deref(), Some("line4\n"));
    g.lines(7, 8);
    // It scrolled up into history with the content: not in the live view.
    assert!(g.snap(None).selection.is_empty());
    g.scroll(Scroll::Top);
    let s = g.snap(None);
    assert_eq!(s.selection.len(), 1);
    assert_eq!(s.row_text(s.selection[0].row), "line4");
    // A new selection by point is relative to the scrolled viewport.
    select::start(&mut g.term, SelectionKind::Line, pt(0, 0), Side::Left);
    assert_eq!(select::text(&g.term).as_deref(), Some("line1\n"));
}

#[test]
fn selection_is_cleared_when_it_scrolls_out_of_history() {
    let mut g = Grid::new(10, 4, 3);
    g.lines(1, 3);
    select::start(&mut g.term, SelectionKind::Line, pt(0, 0), Side::Left);
    g.lines(4, 30);
    assert!(g.snap(None).selection.is_empty());
}

fn event(kind: MouseEventKind, col: usize, row: usize) -> MouseEvent {
    MouseEvent {
        kind,
        col,
        row,
        mods: Modifiers::NONE,
    }
}

fn mode(drag: bool, motion: bool, sgr: bool) -> Mode {
    Mode {
        mouse_reporting: true,
        mouse_drag: drag,
        mouse_motion: motion,
        sgr_mouse: sgr,
        ..Mode::default()
    }
}

#[test]
fn legacy_mouse_encoding() {
    let m = mode(false, false, false);
    let press = event(MouseEventKind::Press(MouseButton::Left), 0, 0);
    assert_eq!(encode_mouse(press, &m), Some(b"\x1b[M !!".to_vec()));
    let release = event(MouseEventKind::Release(MouseButton::Right), 9, 4);
    assert_eq!(
        encode_mouse(release, &m),
        Some(vec![0x1b, b'[', b'M', 35, 42, 37])
    );
    let wheel = event(MouseEventKind::Wheel(Wheel::Down), 1, 1);
    assert_eq!(
        encode_mouse(wheel, &m),
        Some(vec![0x1b, b'[', b'M', 32 + 65, 34, 34])
    );
    let far = event(MouseEventKind::Press(MouseButton::Left), 223, 0);
    assert_eq!(encode_mouse(far, &m), None);
    let edge = event(MouseEventKind::Press(MouseButton::Left), 222, 0);
    assert!(encode_mouse(edge, &m).is_some());
}

#[test]
fn mouse_modes_gate_motion() {
    let drag_motion = event(MouseEventKind::Motion(Some(MouseButton::Left)), 2, 3);
    let bare_motion = event(MouseEventKind::Motion(None), 2, 3);
    let click = mode(false, false, false);
    let drag = mode(true, false, false);
    let any = mode(true, true, false);
    assert_eq!(encode_mouse(drag_motion, &click), None);
    assert_eq!(
        encode_mouse(drag_motion, &drag),
        Some(vec![0x1b, b'[', b'M', 32 + 32, 35, 36])
    );
    assert_eq!(encode_mouse(bare_motion, &drag), None);
    assert_eq!(
        encode_mouse(bare_motion, &any),
        Some(vec![0x1b, b'[', b'M', 32 + 35, 35, 36])
    );
    assert_eq!(encode_mouse(drag_motion, &Mode::default()), None);
}

#[test]
fn sgr_mouse_encoding_keeps_the_released_button_and_modifiers() {
    let m = mode(true, false, true);
    let press = event(MouseEventKind::Press(MouseButton::Middle), 299, 0);
    assert_eq!(encode_mouse(press, &m), Some(b"\x1b[<1;300;1M".to_vec()));
    let release = MouseEvent {
        mods: Modifiers {
            shift: true,
            ctrl: true,
            alt: false,
        },
        ..event(MouseEventKind::Release(MouseButton::Left), 4, 5)
    };
    assert_eq!(encode_mouse(release, &m), Some(b"\x1b[<20;5;6m".to_vec()));
    let drag = event(MouseEventKind::Motion(Some(MouseButton::Right)), 0, 0);
    assert_eq!(encode_mouse(drag, &m), Some(b"\x1b[<34;1;1M".to_vec()));
    let up = event(MouseEventKind::Wheel(Wheel::Up), 0, 0);
    assert_eq!(encode_mouse(up, &m), Some(b"\x1b[<64;1;1M".to_vec()));
}

#[test]
fn alternate_scroll_turns_the_wheel_into_arrows() {
    let alt = Mode {
        alt_screen: true,
        alternate_scroll: true,
        ..Mode::default()
    };
    assert_eq!(
        encode_alternate_scroll(Wheel::Up, 2, &alt),
        Some(b"\x1b[A\x1b[A".to_vec())
    );
    let app = Mode {
        app_cursor: true,
        ..alt
    };
    assert_eq!(
        encode_alternate_scroll(Wheel::Down, 1, &app),
        Some(b"\x1bOB".to_vec())
    );
    assert_eq!(encode_alternate_scroll(Wheel::Left, 1, &alt), None);
    assert_eq!(encode_alternate_scroll(Wheel::Up, 0, &alt), None);
    let primary = Mode {
        alt_screen: false,
        ..alt
    };
    assert_eq!(encode_alternate_scroll(Wheel::Up, 1, &primary), None);
    let mouse = Mode {
        mouse_reporting: true,
        ..alt
    };
    assert_eq!(encode_alternate_scroll(Wheel::Up, 1, &mouse), None);
    let many = encode_alternate_scroll(Wheel::Up, 10_000, &alt).unwrap();
    assert_eq!(many.len(), 3 * crate::MAX_WHEEL_LINES);
}

fn url_of(text: &str, row: usize, col: usize) -> Option<String> {
    let mut g = Grid::new(60, 5, 10);
    g.feed(text);
    g.snap(None).url_at(row, col).map(|m| m.url)
}

#[test]
fn urls_are_found_under_the_point_and_trimmed() {
    let text =
        "see https://example.com/a?b=1, ok\r\n(http://x.org/p_(q)) and file:///tmp/a.txt.\r\n";
    assert_eq!(
        url_of(text, 0, 4).as_deref(),
        Some("https://example.com/a?b=1")
    );
    assert_eq!(
        url_of(text, 0, 20).as_deref(),
        Some("https://example.com/a?b=1")
    );
    assert_eq!(url_of(text, 0, 0), None);
    // The trailing comma and the word after it are not the URL.
    assert_eq!(url_of(text, 0, 29), None);
    assert_eq!(url_of(text, 1, 5).as_deref(), Some("http://x.org/p_(q)"));
    assert_eq!(url_of(text, 1, 25).as_deref(), Some("file:///tmp/a.txt"));
    assert_eq!(url_of("ftp://example.com", 0, 3), None);
    assert_eq!(url_of("http:// nothing", 0, 3), None);
    assert_eq!(url_of("xhttp://example.com", 0, 8), None);
}

#[test]
fn urls_follow_wrapped_rows() {
    let mut g = Grid::new(10, 4, 10);
    g.feed("go http://example.com/long\r\n");
    let snap = g.snap(None);
    let m = snap.url_at(1, 2).expect("url");
    assert_eq!(m.url, "http://example.com/long");
    assert_eq!(
        (m.start_row, m.start_col, m.end_row, m.end_col),
        (0, 3, 2, 5)
    );
    assert_eq!(snap.url_at(0, 3), Some(m));
}
