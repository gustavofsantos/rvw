//! The one thread per terminal: moves bytes between the PTY and the
//! emulation, and watches for the child's exit.
//!
//! This replaces `alacritty_terminal::event_loop`, which gives up its read
//! buffer when the emulation's lock is busy and the next read fails with EIO
//! (the child has hung up): the last output of a command is lost whenever a
//! snapshot is being copied at that moment. Here the lock is waited for, which
//! costs the length of one snapshot copy.

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use alacritty_terminal::event::WindowSize;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::tty::{ChildEvent, EventedPty, EventedReadWrite, Pty};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use polling::{Event as Readiness, Events, PollMode, Poller};

use crate::notify::Scanner;
use crate::terminal::{Exit, Listener, Shared};

/// Bytes read per `read` call; the only buffer between PTY and parser.
const READ_CHUNK: usize = 64 * 1024;
/// Bytes parsed in one turn before the other work gets a look in.
const TURN_LIMIT: usize = 16 * READ_CHUNK;
/// Bytes drained after the child exits, however much a straggler writes.
const DRAIN_LIMIT: usize = 64 * READ_CHUNK;
/// Input queued and not yet accepted by the PTY, at most.
pub(crate) const MAX_PENDING: usize = 4 << 20;

const TOKEN_PTY: usize = 0;
const TOKEN_CHILD: usize = 1;

enum Msg {
    Input(Vec<u8>),
    Resize(WindowSize),
    Shutdown,
}

/// The owner's end: queue input, resize, stop.
#[derive(Clone)]
pub(crate) struct Pump {
    tx: Sender<Msg>,
    poller: Arc<Poller>,
    pending: Arc<AtomicUsize>,
}

impl Pump {
    /// `Err` when the queue is full or the thread is gone.
    pub(crate) fn input(&self, bytes: Vec<u8>) -> Result<(), PumpError> {
        let len = bytes.len();
        if self.pending.fetch_add(len, Ordering::AcqRel) + len > MAX_PENDING {
            self.pending.fetch_sub(len, Ordering::AcqRel);
            return Err(PumpError::Full);
        }
        self.send(Msg::Input(bytes)).inspect_err(|_| {
            self.pending.fetch_sub(len, Ordering::AcqRel);
        })
    }

    pub(crate) fn resize(&self, size: WindowSize) -> Result<(), PumpError> {
        self.send(Msg::Resize(size))
    }

    pub(crate) fn shutdown(&self) {
        // The thread may have ended already.
        let _ = self.send(Msg::Shutdown);
    }

    fn send(&self, msg: Msg) -> Result<(), PumpError> {
        self.tx.send(msg).map_err(|_| PumpError::Gone)?;
        self.poller.notify().map_err(|_| PumpError::Gone)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PumpError {
    Full,
    Gone,
}

/// Start the thread. It owns the PTY and drops it (reaping the child) when
/// it ends.
pub(crate) fn spawn(
    pty: Pty,
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
) -> io::Result<(Pump, JoinHandle<()>)> {
    let poller = Arc::new(Poller::new()?);
    let (tx, rx) = mpsc::channel();
    let pending = Arc::new(AtomicUsize::new(0));
    let pump = Pump {
        tx,
        poller: Arc::clone(&poller),
        pending: Arc::clone(&pending),
    };
    let thread = std::thread::Builder::new()
        .name("rvw-term pty".to_owned())
        .spawn(move || {
            let mut pump_loop = PumpLoop {
                pty,
                poller,
                rx,
                term,
                shared,
                parser: Processor::<StdSyncHandler>::default(),
                scanner: Scanner::new(),
                buf: vec![0; READ_CHUNK],
                out: VecDeque::new(),
                written: 0,
                pending,
            };
            pump_loop.run();
        })?;
    Ok((pump, thread))
}

struct PumpLoop {
    pty: Pty,
    poller: Arc<Poller>,
    rx: Receiver<Msg>,
    term: Arc<FairMutex<Term<Listener>>>,
    shared: Arc<Shared>,
    parser: Processor<StdSyncHandler>,
    /// Finds notification requests in what the parser is given.
    scanner: Scanner,
    buf: Vec<u8>,
    /// Input waiting for the PTY; `written` bytes of the front are gone.
    out: VecDeque<Vec<u8>>,
    written: usize,
    pending: Arc<AtomicUsize>,
}

enum Turn {
    Continue,
    Stop,
}

impl PumpLoop {
    fn run(&mut self) {
        let mut want_write = false;
        // SAFETY: the PTY outlives its registration; it is deregistered
        // below before this struct drops it.
        let registered = unsafe {
            self.pty.register(
                &self.poller,
                Readiness::readable(TOKEN_PTY),
                PollMode::Level,
            )
        };
        if registered.is_err() {
            return;
        }
        let mut events = Events::with_capacity(NonZeroUsize::MIN.saturating_add(63));
        'turns: loop {
            let timeout = self
                .parser
                .sync_timeout()
                .sync_timeout()
                .map(|at| at.saturating_duration_since(Instant::now()));
            events.clear();
            if let Err(err) = self.poller.wait(&mut events, timeout) {
                if err.kind() == ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if !self.take_messages() {
                break;
            }
            // A synchronized update that ran past its time is shown as is.
            if let Some(at) = self.parser.sync_timeout().sync_timeout() {
                if at <= Instant::now() {
                    self.parser.stop_sync(&mut *self.term.lock());
                    self.shared.mark_changed();
                }
            }
            for event in events.iter() {
                let turn = match event.key {
                    TOKEN_CHILD => self.child_event(),
                    TOKEN_PTY if event.is_interrupt() => Turn::Continue,
                    TOKEN_PTY => self.pty_ready(event.readable, event.writable),
                    _ => Turn::Continue,
                };
                if matches!(turn, Turn::Stop) {
                    break 'turns;
                }
            }
            let writable = !self.out.is_empty();
            if writable != want_write {
                want_write = writable;
                let mut interest = Readiness::readable(TOKEN_PTY);
                interest.writable = writable;
                if self
                    .pty
                    .reregister(&self.poller, interest, PollMode::Level)
                    .is_err()
                {
                    break;
                }
            }
        }
        let _ = self.pty.deregister(&self.poller);
    }

    /// Apply queued messages. False on shutdown.
    fn take_messages(&mut self) -> bool {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Input(bytes) => self.out.push_back(bytes),
                Msg::Resize(size) => self.set_window_size(size),
                Msg::Shutdown => return false,
            }
        }
        true
    }

    #[cfg(unix)]
    fn set_window_size(&mut self, size: WindowSize) {
        use std::os::fd::AsRawFd;
        let win = libc::winsize {
            ws_row: size.num_lines,
            ws_col: size.num_cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: the fd is the open PTY master; `win` outlives the call. A
        // failure leaves the old size, which the next resize replaces.
        unsafe {
            libc::ioctl(
                self.pty.file().as_raw_fd(),
                libc::TIOCSWINSZ,
                &raw const win,
            );
        }
    }

    #[cfg(not(unix))]
    fn set_window_size(&mut self, size: WindowSize) {
        alacritty_terminal::event::OnResize::on_resize(&mut self.pty, size);
    }

    fn child_event(&mut self) -> Turn {
        let Some(ChildEvent::Exited(status)) = self.pty.next_child_event() else {
            return Turn::Continue;
        };
        // Output written just before the exit is still in the PTY.
        let _ = self.read_available(DRAIN_LIMIT);
        if let Some(status) = status {
            self.shared.record_exit(Exit::from_status(status));
        }
        self.shared.mark_changed();
        Turn::Stop
    }

    fn pty_ready(&mut self, readable: bool, writable: bool) -> Turn {
        if readable {
            match self.read_available(TURN_LIMIT) {
                Ok(parsed) => {
                    // Output held back by a synchronized update is not shown yet.
                    if parsed > 0 && self.parser.sync_bytes_count() < parsed {
                        self.shared.mark_changed();
                    }
                }
                Err(_) => return Turn::Stop,
            }
        }
        if writable && self.flush().is_err() {
            return Turn::Stop;
        }
        Turn::Continue
    }

    /// Read what the PTY has, up to `limit` bytes, into the emulation.
    /// Returns the bytes parsed. A hangup (EIO) is not an error: the exit
    /// arrives as a child event.
    fn read_available(&mut self, limit: usize) -> io::Result<usize> {
        let mut parsed = 0;
        while parsed < limit {
            match self.pty.reader().read(&mut self.buf) {
                Ok(0) => break,
                Ok(n) => {
                    let shared = &self.shared;
                    self.scanner
                        .feed(&self.buf[..n], |notice| shared.push_notification(notice));
                    let mut term = self.term.lock();
                    self.parser.advance(&mut *term, &self.buf[..n]);
                    parsed += n;
                }
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => {}
                    ErrorKind::WouldBlock => break,
                    _ if err.raw_os_error() == Some(libc_eio()) => break,
                    _ => return Err(err),
                },
            }
        }
        Ok(parsed)
    }

    /// Write queued input until the PTY would block.
    fn flush(&mut self) -> io::Result<()> {
        while let Some(front) = self.out.front() {
            match self.pty.writer().write(&front[self.written..]) {
                Ok(0) => break,
                Ok(n) => {
                    self.pending.fetch_sub(n, Ordering::AcqRel);
                    self.written += n;
                    if self.written == front.len() {
                        self.out.pop_front();
                        self.written = 0;
                    }
                }
                Err(err) => match err.kind() {
                    ErrorKind::Interrupted => {}
                    ErrorKind::WouldBlock => break,
                    _ => return Err(err),
                },
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
fn libc_eio() -> i32 {
    libc::EIO
}

#[cfg(not(unix))]
fn libc_eio() -> i32 {
    -1
}
