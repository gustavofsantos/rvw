//! One terminal: a child on a PTY, the emulation of its output, and the
//! threads between them.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use alacritty_terminal::event::{Event as TermEvent, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config;
use alacritty_terminal::tty::{self, Options as PtyOptions, Shell};
use alacritty_terminal::Term;

use crate::pump::{self, Pump, PumpError};
use crate::scroll::{self, Scroll};
use crate::search::Search;
use crate::select::{self, GridPoint, SelectionKind, Side};
use crate::snapshot::{capture, Snapshot};

/// The most scrollback a terminal may be given, in lines.
pub const MAX_SCROLLBACK: usize = 100_000;
/// The most bytes one `write` accepts. A larger paste is the caller's to
/// split, so a runaway producer cannot grow the input queue without bound.
pub const MAX_WRITE: usize = 1 << 20;
/// How long `kill` waits, after SIGKILL, for the child to be reaped.
const REAP_WAIT: Duration = Duration::from_secs(5);
const DEFAULT_GRACE: Duration = Duration::from_secs(2);
const DEFAULT_SCROLLBACK: usize = 10_000;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// A terminal's identity: stable for the life of the process, never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TerminalId(pub u64);

impl fmt::Display for TerminalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t{}", self.0)
    }
}

#[derive(Debug)]
pub enum Error {
    /// The PTY or the child could not be created, or the OS refused.
    Io(io::Error),
    /// Columns or rows were zero or above `u16::MAX`.
    InvalidSize,
    /// The terminal's loop has ended (the child is gone).
    Closed,
    /// A write above [`MAX_WRITE`] bytes.
    TooLarge,
    /// The child is not reading and its input queue is full.
    Busy,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(err) => write!(f, "{err}"),
            Error::InvalidSize => f.write_str("terminal size must be 1 to 65535 columns and rows"),
            Error::Closed => f.write_str("terminal is closed"),
            Error::TooLarge => write!(f, "write is larger than {MAX_WRITE} bytes"),
            Error::Busy => f.write_str("terminal input queue is full"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

/// How the child ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    /// The exit code, when it exited by itself.
    pub code: Option<i32>,
    /// The signal that ended it, on Unix.
    pub signal: Option<i32>,
}

impl Exit {
    pub(crate) fn from_status(status: ExitStatus) -> Self {
        #[cfg(unix)]
        let signal = std::os::unix::process::ExitStatusExt::signal(&status);
        #[cfg(not(unix))]
        let signal = None;
        Self {
            code: status.code(),
            signal,
        }
    }
}

/// What the terminal tells its owner. At most one `Changed` is outstanding at
/// a time: it is not queued again until `snapshot` has been called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The screen, title or bell changed: take a snapshot.
    Changed,
    /// The child exited. Sent once; `exit_status` and `Snapshot::exit` agree.
    Exited(Exit),
}

/// What to run and how big.
#[derive(Clone, Debug)]
pub struct SpawnOptions {
    /// Program and arguments. `None` runs the user's shell, from the
    /// account database.
    pub command: Option<Vec<String>>,
    /// Starting directory. An invalid path is ignored by the OS call, and the
    /// child starts in the current directory.
    pub working_directory: Option<PathBuf>,
    /// Extra environment for the child only. `TERM` and `COLORTERM` default
    /// to `xterm-256color` and `truecolor` unless set here.
    pub env: HashMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    /// Lines of history kept, at most [`MAX_SCROLLBACK`].
    pub scrollback: usize,
    /// How long `kill` waits after SIGHUP before SIGKILL.
    pub kill_grace: Duration,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            command: None,
            working_directory: None,
            env: HashMap::new(),
            cols: 80,
            rows: 24,
            scrollback: DEFAULT_SCROLLBACK,
            kill_grace: DEFAULT_GRACE,
        }
    }
}

/// State shared between the owner and the loop thread.
pub(crate) struct Shared {
    /// A `Changed` is queued and no snapshot has been taken since.
    pub(crate) dirty: AtomicBool,
    bell: AtomicBool,
    title: Mutex<Option<String>>,
    exit: Mutex<Option<Exit>>,
    exit_cv: Condvar,
    /// Columns in the high 16 bits, rows in the low 16.
    size: AtomicU32,
    /// The child's process group, 0 when unknown.
    pgid: AtomicI32,
    input: Mutex<Option<Pump>>,
    events: Sender<Event>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    pub(crate) fn new(events: Sender<Event>, cols: u16, rows: u16) -> Self {
        Self {
            dirty: AtomicBool::new(false),
            bell: AtomicBool::new(false),
            title: Mutex::new(None),
            exit: Mutex::new(None),
            exit_cv: Condvar::new(),
            size: AtomicU32::new(u32::from(cols) << 16 | u32::from(rows)),
            pgid: AtomicI32::new(0),
            input: Mutex::new(None),
            events,
        }
    }

    /// Queue a `Changed` unless one is already waiting for its snapshot.
    pub(crate) fn mark_changed(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            // A dropped receiver means nobody is listening.
            let _ = self.events.send(Event::Changed);
        }
    }

    fn window_size(&self) -> WindowSize {
        let packed = self.size.load(Ordering::Relaxed);
        WindowSize {
            num_cols: (packed >> 16) as u16,
            num_lines: (packed & 0xffff) as u16,
            cell_width: 0,
            cell_height: 0,
        }
    }

    fn reply(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let pump = lock(&self.input).clone();
        if let Some(pump) = pump {
            // A full queue drops the reply: the program asked, nobody read.
            let _ = pump.input(bytes);
        }
    }

    pub(crate) fn record_exit(&self, exit: Exit) {
        *lock(&self.exit) = Some(exit);
        self.exit_cv.notify_all();
        let _ = self.events.send(Event::Exited(exit));
    }

    /// True once the child's exit has been recorded, or within `timeout`.
    fn wait_exit(&self, timeout: Duration) -> bool {
        let guard = lock(&self.exit);
        let (guard, _) = self
            .exit_cv
            .wait_timeout_while(guard, timeout, |exit| exit.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        guard.is_some()
    }

    fn signal_group(&self, signal: i32) {
        #[cfg(unix)]
        {
            let pgid = self.pgid.load(Ordering::Acquire);
            if pgid > 1 {
                // SAFETY: killpg takes plain integers; a stale group is ESRCH.
                unsafe {
                    libc::killpg(pgid, signal);
                }
            }
        }
        #[cfg(not(unix))]
        let _ = signal;
    }
}

/// The emulation's event sink. Runs on the loop thread, so it only flips
/// flags and sends small values.
#[derive(Clone)]
pub(crate) struct Listener(Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let shared = &self.0;
        match event {
            TermEvent::Wakeup => shared.mark_changed(),
            TermEvent::Bell => {
                shared.bell.store(true, Ordering::Release);
                shared.mark_changed();
            }
            TermEvent::Title(title) => {
                *lock(&shared.title) = Some(title);
                shared.mark_changed();
            }
            TermEvent::ResetTitle => {
                *lock(&shared.title) = None;
                shared.mark_changed();
            }
            TermEvent::PtyWrite(text) => shared.reply(text.into_bytes()),
            TermEvent::TextAreaSizeRequest(format) => {
                shared.reply(format(shared.window_size()).into_bytes());
            }
            // Clipboard and color queries are not answered: a program that
            // asks gets no reply, as on a terminal that lacks them.
            TermEvent::ClipboardStore(..)
            | TermEvent::ClipboardLoad(..)
            | TermEvent::ColorRequest(..)
            | TermEvent::MouseCursorDirty
            | TermEvent::CursorBlinkingChange
            | TermEvent::ChildExit(_)
            | TermEvent::Exit => {}
        }
    }
}

#[cfg(unix)]
fn libc_sighup() -> i32 {
    libc::SIGHUP
}

#[cfg(not(unix))]
fn libc_sighup() -> i32 {
    1
}

/// The grid size handed to the emulation.
pub(crate) struct Size {
    pub(crate) cols: usize,
    pub(crate) rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

type LoopHandle = JoinHandle<()>;

/// A running terminal. Dropping it closes it like [`Terminal::kill`].
pub struct Terminal {
    id: TerminalId,
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
    events: Receiver<Event>,
    search: Mutex<Option<Search>>,
    thread: Option<LoopHandle>,
    grace: Duration,
}

impl Terminal {
    /// Start a child on a new PTY, in its own session and process group.
    pub fn spawn(options: SpawnOptions) -> Result<Self, Error> {
        if options.cols == 0 || options.rows == 0 {
            return Err(Error::InvalidSize);
        }
        let id = TerminalId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let shell = match options.command {
            Some(mut argv) if !argv.is_empty() => {
                let program = argv.remove(0);
                Some(Shell::new(program, argv))
            }
            _ => None,
        };
        let mut env = options.env;
        env.entry("TERM".to_owned())
            .or_insert_with(|| "xterm-256color".to_owned());
        env.entry("COLORTERM".to_owned())
            .or_insert_with(|| "truecolor".to_owned());
        let pty_options = PtyOptions {
            shell,
            working_directory: options.working_directory,
            // Output written just before the exit is still shown.
            drain_on_exit: true,
            env,
            #[cfg(windows)]
            escape_args: true,
        };
        let window = WindowSize {
            num_lines: options.rows,
            num_cols: options.cols,
            cell_width: 0,
            cell_height: 0,
        };
        let pty = tty::new(&pty_options, window, id.0)?;

        let (events_tx, events_rx) = mpsc::channel();
        let shared = Arc::new(Shared::new(events_tx, options.cols, options.rows));
        #[cfg(unix)]
        {
            // The child called setsid, so its pid is its group id.
            let pid = i32::try_from(pty.child().id()).unwrap_or(0);
            shared.pgid.store(pid, Ordering::Release);
        }

        let config = Config {
            scrolling_history: options.scrollback.min(MAX_SCROLLBACK),
            ..Config::default()
        };
        let size = Size {
            cols: usize::from(options.cols),
            rows: usize::from(options.rows),
        };
        let listener = Listener(Arc::clone(&shared));
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        drop(listener);
        let (pump, thread) = pump::spawn(pty, Arc::clone(&term), Arc::clone(&shared))?;
        *lock(&shared.input) = Some(pump);

        Ok(Self {
            id,
            term,
            shared,
            events: events_rx,
            search: Mutex::new(None),
            thread: Some(thread),
            grace: options.kill_grace,
        })
    }

    #[must_use]
    pub fn id(&self) -> TerminalId {
        self.id
    }

    /// A handle on the loop's input. Cloned out so no lock is held while
    /// waiting on the emulation, which the loop thread may hold while it
    /// takes that lock to reply.
    fn input(&self) -> Result<Pump, Error> {
        lock(&self.shared.input).clone().ok_or(Error::Closed)
    }

    /// Queue typed or pasted bytes for the child's input. Typing returns the
    /// viewport to the live screen and clears the selection. Returns once
    /// queued, not written. Mouse and focus reports use [`write_report`].
    ///
    /// [`write_report`]: Terminal::write_report
    pub fn write(&self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > MAX_WRITE {
            return Err(Error::TooLarge);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        {
            let mut term = self.term.lock();
            term.selection = None;
            if term.grid().display_offset() > 0 {
                scroll::apply(&mut term, Scroll::Bottom);
            }
        }
        self.shared.mark_changed();
        self.write_report(bytes)
    }

    /// Queue bytes for the child's input without touching the viewport or
    /// the selection: mouse reports, focus reports.
    pub fn write_report(&self, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > MAX_WRITE {
            return Err(Error::TooLarge);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.input()?
            .input(bytes.to_vec())
            .map_err(|err| match err {
                PumpError::Full => Error::Busy,
                PumpError::Gone => Error::Closed,
            })
    }

    /// Change the grid and the PTY's window size.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), Error> {
        if cols == 0 || rows == 0 {
            return Err(Error::InvalidSize);
        }
        let pump = self.input()?;
        self.shared
            .size
            .store(u32::from(cols) << 16 | u32::from(rows), Ordering::Relaxed);
        self.term.lock().resize(Size {
            cols: usize::from(cols),
            rows: usize::from(rows),
        });
        pump.resize(self.shared.window_size())
            .map_err(|_| Error::Closed)?;
        self.shared.mark_changed();
        Ok(())
    }

    /// An owned copy of the screen. Clears the pending `Changed`, so the
    /// next change queues a new one. Holds the emulation's lock only for the
    /// copy.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        // Before the copy: a change during it queues a fresh event.
        self.shared.dirty.store(false, Ordering::Release);
        let bell = self.shared.bell.swap(false, Ordering::AcqRel);
        let mut snapshot = {
            let term = self.term.lock();
            capture(&term, lock(&self.search).as_mut())
        };
        snapshot.bell = bell;
        snapshot.title.clone_from(&lock(&self.shared.title));
        snapshot.exit = self.exit_status();
        snapshot
    }

    /// Move the viewport over the scrollback. The alternate screen has none,
    /// so this does nothing there. New output while scrolled up keeps the
    /// same content in view until the history is full.
    pub fn scroll(&self, scroll: Scroll) {
        scroll::apply(&mut self.term.lock(), scroll);
        self.shared.mark_changed();
    }

    /// Return to the live screen.
    pub fn scroll_to_bottom(&self) {
        self.scroll(Scroll::Bottom);
    }

    /// Search scrollback and screen for `query` (plain text; case-sensitive
    /// only when it has an uppercase letter). An empty or invalid query
    /// clears the search and returns false. Nothing is focused until
    /// [`search_next`](Terminal::search_next) or `search_prev`.
    pub fn set_search(&self, query: &str) -> bool {
        let term = self.term.lock();
        let search = Search::new(query).map(|mut search| {
            search.recount(&term);
            search
        });
        let set = search.is_some();
        *lock(&self.search) = search;
        drop(term);
        self.shared.mark_changed();
        set
    }

    pub fn clear_search(&self) {
        *lock(&self.search) = None;
        self.shared.mark_changed();
    }

    /// Focus the next match below, wrapping, and scroll it into view. False
    /// when there is no search or no match.
    pub fn search_next(&self) -> bool {
        self.search_step(true)
    }

    /// Focus the previous match above, wrapping.
    pub fn search_prev(&self) -> bool {
        self.search_step(false)
    }

    fn search_step(&self, forward: bool) -> bool {
        let mut term = self.term.lock();
        let found = lock(&self.search)
            .as_mut()
            .is_some_and(|search| search.step(&mut term, forward));
        drop(term);
        self.shared.mark_changed();
        found
    }

    /// Start a selection at a viewport cell, replacing any other.
    pub fn start_selection(&self, kind: SelectionKind, at: GridPoint, side: Side) {
        select::start(&mut self.term.lock(), kind, at, side);
        self.shared.mark_changed();
    }

    /// Move the selection's end. Does nothing without a selection.
    pub fn update_selection(&self, at: GridPoint, side: Side) {
        select::update(&mut self.term.lock(), at, side);
        self.shared.mark_changed();
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
        self.shared.mark_changed();
    }

    /// The selected text, `None` when nothing (or an empty range) is selected.
    #[must_use]
    pub fn selected_text(&self) -> Option<String> {
        select::text(&self.term.lock())
    }

    /// The channel of [`Event`]s, for the owner to wait on.
    #[must_use]
    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }

    /// How the child ended, once it has.
    #[must_use]
    pub fn exit_status(&self) -> Option<Exit> {
        *lock(&self.shared.exit)
    }

    /// Close the terminal: SIGHUP to the whole process group, SIGKILL to it
    /// after the grace period if the child has not exited, then the child is
    /// reaped and the loop thread joined. Returns how the child ended, or
    /// `None` if the OS never reported it. Safe to call again.
    ///
    /// When the child had already exited, nothing is signalled: its process
    /// group id may have been reused by then, and a straggler that ignored
    /// the hangup the kernel sent it is left alone.
    pub fn kill(&mut self) -> Option<Exit> {
        let Some(thread) = self.thread.take() else {
            return self.exit_status();
        };
        if self.exit_status().is_none() {
            self.shared.signal_group(libc_sighup());
            if !self.shared.wait_exit(self.grace) {
                #[cfg(unix)]
                self.shared.signal_group(libc::SIGKILL);
                self.shared.wait_exit(REAP_WAIT);
            }
        }
        if let Some(pump) = lock(&self.shared.input).take() {
            pump.shutdown();
        }
        if self.exit_status().is_some() {
            // The loop ends on the exit; joining drops the `Pty`. Joined only
            // when the child is dead: that drop waits for it.
            let _ = thread.join();
        }
        self.exit_status()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.kill();
    }
}
