//! The thin shell around the pure model: raw mode and the alternate screen,
//! the crossterm event loop, flash timers, and suspending to run the editor.
//! It holds no UI logic; every key is translated and handed to
//! `Model::update`, and every frame is `Model::screen`.

use std::io::{stdout, Write};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CtEvent, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, MouseButton as CtButton, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal as ct_terminal;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::DefaultTerminal;

use crate::editor::editor_command;
use crate::model::{
    EditRequest, Effect, Event, Model, MouseButton, MouseEvent, MouseKind, FLASH_FOR_MS,
};
use crate::Options;

/// How often the loop wakes to check timers and the cancel flag.
const TICK: Duration = Duration::from_millis(50);

/// Owns the terminal's raw mode, alternate screen and mouse capture, and
/// gives them back on drop (also when a panic unwinds).
struct Screen {
    terminal: DefaultTerminal,
}

impl Screen {
    fn enter() -> Screen {
        let terminal = ratatui::init();
        let _ = execute!(stdout(), EnableMouseCapture);
        Screen { terminal }
    }

    fn leave() {
        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
    }

    fn resume(&mut self) {
        self.terminal = ratatui::init();
        let _ = execute!(stdout(), EnableMouseCapture);
    }

    fn draw(&mut self, model: &Model) {
        let lines: Vec<Line> = model
            .screen()
            .into_iter()
            .map(|row| {
                Line::from(
                    row.into_iter()
                        .map(|p| Span::styled(p.text, p.style))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let _ = self.terminal.draw(|f| {
            f.render_widget(Paragraph::new(lines), f.area());
        });
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        Screen::leave();
    }
}

/// A key press named the way Bubble Tea names keys; `None` for keys the UI
/// does not know.
fn key_name(k: KeyEvent) -> Option<String> {
    if k.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    let mut name = match k.code {
        KeyCode::Char(' ') => "space".to_owned(),
        KeyCode::Char(c) if ctrl => format!("ctrl+{}", c.to_ascii_lowercase()),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".to_owned(),
        KeyCode::Esc => "esc".to_owned(),
        KeyCode::Tab => "tab".to_owned(),
        KeyCode::BackTab => "shift+tab".to_owned(),
        KeyCode::Backspace => "backspace".to_owned(),
        KeyCode::Delete => "delete".to_owned(),
        KeyCode::Up => "up".to_owned(),
        KeyCode::Down => "down".to_owned(),
        KeyCode::Left => "left".to_owned(),
        KeyCode::Right => "right".to_owned(),
        KeyCode::PageUp => "pgup".to_owned(),
        KeyCode::PageDown => "pgdown".to_owned(),
        KeyCode::Home => "home".to_owned(),
        KeyCode::End => "end".to_owned(),
        _ => return None,
    };
    if alt {
        name = format!("alt+{name}");
    }
    Some(name)
}

fn button(b: CtButton) -> MouseButton {
    if b == CtButton::Left {
        MouseButton::Left
    } else {
        MouseButton::Other
    }
}

#[allow(clippy::needless_pass_by_value)] // events are consumed at the call site
fn translate(ev: CtEvent) -> Option<Event> {
    match ev {
        CtEvent::Resize(w, h) => Some(Event::Resize {
            width: usize::from(w),
            height: usize::from(h),
        }),
        CtEvent::Key(k) => key_name(k).map(Event::Key),
        CtEvent::Mouse(m) => {
            let kind = match m.kind {
                MouseEventKind::Down(b) => MouseKind::Click(button(b)),
                MouseEventKind::Up(_) => MouseKind::Release,
                MouseEventKind::Drag(b) => MouseKind::Motion(button(b)),
                MouseEventKind::ScrollUp => MouseKind::WheelUp,
                MouseEventKind::ScrollDown => MouseKind::WheelDown,
                _ => return None,
            };
            Some(Event::Mouse(MouseEvent {
                kind,
                x: usize::from(m.column),
                y: usize::from(m.row),
            }))
        }
        _ => None,
    }
}

/// Runs the editor on `path` with the terminal handed over to it.
fn run_editor(screen: &mut Screen, editor: &str, path: &std::path::Path) -> Option<String> {
    Screen::leave();
    let status = editor_command(editor, path).status();
    screen.resume();
    match status {
        Ok(s) if s.success() => None,
        Ok(s) => Some(format!("{s}")),
        Err(e) => Some(e.to_string()),
    }
}

/// Shows the UI until the user quits or a signal cancels it.
pub fn run(opts: Options) -> Result<(), String> {
    let editor = opts.editor.clone();
    let mut model = Model::new(opts)?;
    let mut screen = Screen::enter();
    let cancel = rvw_git::cancel_flag();

    let mut timers: Vec<(Instant, u64)> = Vec::new();
    let mut pending: Vec<Event> = Vec::new();
    if let Ok((w, h)) = ct_terminal::size() {
        pending.push(Event::Resize {
            width: usize::from(w),
            height: usize::from(h),
        });
    }

    'outer: loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        // Fire due timers.
        let now = Instant::now();
        let (due, rest): (Vec<_>, Vec<_>) = timers.drain(..).partition(|(at, _)| *at <= now);
        timers = rest;
        pending.extend(due.into_iter().map(|(_, seq)| Event::FlashDone(seq)));

        while !pending.is_empty() {
            let ev = pending.remove(0);
            for effect in model.update(ev) {
                match effect {
                    Effect::Quit => break 'outer,
                    Effect::FlashTimer { seq } => {
                        timers.push((Instant::now() + Duration::from_millis(FLASH_FOR_MS), seq));
                    }
                    Effect::RunEditor { req, path } => {
                        pending.push(editor_done(&mut screen, &editor, req, path));
                    }
                }
            }
        }

        screen.draw(&model);

        if event::poll(TICK).unwrap_or(false) {
            // Drain everything that is ready, then draw once.
            loop {
                match event::read() {
                    Ok(ev) => pending.extend(translate(ev)),
                    Err(_) => break 'outer,
                }
                if !event::poll(Duration::ZERO).unwrap_or(false) {
                    break;
                }
            }
        }
    }
    let _ = stdout().flush();
    Ok(())
}

fn editor_done(
    screen: &mut Screen,
    editor: &str,
    req: EditRequest,
    path: std::path::PathBuf,
) -> Event {
    let error = run_editor(screen, editor, &path);
    Event::EditorDone { req, path, error }
}
