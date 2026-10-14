//! Real PTYs. Children are `/bin/sh -c` with explicit commands, never the
//! user's shell, and every wait is bounded by a channel timeout.

use std::ffi::CString;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::terminal::Shared;
use crate::{CellFlags, Color, Event, Exit, Snapshot, SpawnOptions, Terminal};

const WAIT: Duration = Duration::from_secs(20);

fn sh(script: &str) -> Terminal {
    sh_with(script, SpawnOptions::default())
}

fn sh_with(script: &str, options: SpawnOptions) -> Terminal {
    let command = vec!["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()];
    Terminal::spawn(SpawnOptions {
        command: Some(command),
        ..options
    })
    .expect("spawn")
}

/// Wait, event by event, until `done` holds for a snapshot. Panics with the
/// last screen on timeout.
fn wait_for(term: &Terminal, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + WAIT;
    loop {
        let snapshot = term.snapshot();
        if done(&snapshot) {
            return snapshot;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match term.events().recv_timeout(left) {
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {
                panic!("timed out; screen was:\n{}", term.snapshot().text())
            }
            Err(RecvTimeoutError::Disconnected) => panic!("event channel closed"),
        }
    }
}

fn wait_exit(term: &Terminal) -> Exit {
    wait_for(term, |s| s.exit.is_some()).exit.expect("exit")
}

#[test]
fn echo_output_reaches_the_snapshot() {
    let term = sh("echo hello-rvw");
    let snapshot = wait_for(&term, |s| s.text().contains("hello-rvw"));
    assert_eq!(snapshot.rows, 24);
    assert_eq!(snapshot.cols, 80);
    assert_eq!(
        wait_exit(&term),
        Exit {
            code: Some(0),
            signal: None
        }
    );
}

#[test]
fn output_just_before_exit_survives_snapshots_taken_meanwhile() {
    // The emulation's lock is contended the whole time: the last chunk must
    // still be parsed when the child hangs up right after writing it.
    for _ in 0..20 {
        let term = sh("seq 1 3000; echo end-marker");
        let deadline = Instant::now() + WAIT;
        let last = loop {
            let snapshot = term.snapshot();
            if snapshot.exit.is_some() {
                break term.snapshot();
            }
            assert!(Instant::now() < deadline, "child never exited");
        };
        assert!(
            last.text().contains("end-marker"),
            "lost output:\n{}",
            last.text()
        );
    }
}

#[test]
fn child_gets_term_and_colorterm() {
    let term = sh("echo \"[$TERM][$COLORTERM]\"");
    wait_for(&term, |s| s.text().contains("[xterm-256color][truecolor]"));
}

#[test]
fn caller_env_wins_and_directory_is_set() {
    let mut options = SpawnOptions {
        working_directory: Some("/".into()),
        ..Default::default()
    };
    options.env.insert("TERM".to_owned(), "dumb".to_owned());
    options.env.insert("RVW_T".to_owned(), "ok".to_owned());
    let term = sh_with("echo \"[$TERM][$RVW_T][$(pwd)]\"", options);
    wait_for(&term, |s| s.text().contains("[dumb][ok][/]"));
}

#[test]
fn exit_code_is_reported_by_event_and_snapshot() {
    let term = sh("exit 7");
    let deadline = Instant::now() + WAIT;
    let exit = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match term.events().recv_timeout(left).expect("event") {
            Event::Exited(exit) => break exit,
            Event::Changed | Event::Notified => {}
        }
    };
    assert_eq!(
        exit,
        Exit {
            code: Some(7),
            signal: None
        }
    );
    assert_eq!(term.exit_status(), Some(exit));
    assert_eq!(term.snapshot().exit, Some(exit));
}

#[test]
fn input_reaches_the_child() {
    let term = sh("read line; echo got:$line");
    term.write(b"abc\n").expect("write");
    wait_for(&term, |s| s.text().contains("got:abc"));
}

#[test]
fn resize_changes_the_grid_and_the_pty() {
    let term = sh("read x; stty size");
    term.resize(100, 30).expect("resize");
    let snapshot = term.snapshot();
    assert_eq!((snapshot.cols, snapshot.rows), (100, 30));
    term.write(b"\n").expect("write");
    wait_for(&term, |s| s.text().contains("30 100"));
}

#[test]
fn zero_size_is_refused() {
    let term = sh("exit 0");
    assert!(matches!(term.resize(0, 10), Err(crate::Error::InvalidSize)));
    let options = SpawnOptions {
        cols: 0,
        ..Default::default()
    };
    assert!(matches!(
        Terminal::spawn(options),
        Err(crate::Error::InvalidSize)
    ));
}

#[test]
fn colors_and_flags_are_carried() {
    let term = sh("printf '\\033[1;31mR\\033[0m\\033[38;2;1;2;3mT\\033[0m'");
    let snapshot = wait_for(&term, |s| s.row_text(0).starts_with("RT"));
    let row = snapshot.row(0);
    assert_eq!(row[0].fg, Color::Indexed(1));
    assert!(row[0].flags.contains(CellFlags::BOLD));
    assert_eq!(row[1].fg, Color::Rgb(1, 2, 3));
    assert_eq!(row[2].fg, Color::Foreground);
}

#[test]
fn cursor_and_modes_are_reported() {
    // Application cursor and bracketed paste on, then park the cursor.
    let term = sh("printf '\\033[?1h\\033[?2004h\\033[3;5Hx'");
    let snapshot = wait_for(&term, |s| s.mode.app_cursor && s.mode.bracketed_paste);
    assert!(!snapshot.mode.alt_screen);
    let snapshot = wait_for(&term, |s| s.row_text(2).contains('x'));
    assert_eq!((snapshot.cursor.row, snapshot.cursor.col), (2, 5));
    assert!(snapshot.cursor.visible);
}

#[test]
fn title_and_scrollback_are_bounded() {
    let options = SpawnOptions {
        rows: 5,
        scrollback: 10,
        ..Default::default()
    };
    let term = sh_with("printf '\\033]0;mytitle\\007'; seq 1 100", options);
    let snapshot = wait_for(&term, |s| s.exit.is_some() && s.text().contains("100"));
    assert_eq!(snapshot.title.as_deref(), Some("mytitle"));
    assert_eq!(snapshot.history_lines, 10);
}

#[test]
fn wakeups_coalesce_until_a_snapshot_is_taken() {
    let (tx, rx) = mpsc::channel();
    let shared = Shared::new(tx, 80, 24);
    shared.mark_changed();
    shared.mark_changed();
    shared.mark_changed();
    assert_eq!(rx.try_recv(), Ok(Event::Changed));
    assert!(rx.try_recv().is_err());
    // What a snapshot does, then the next change queues again.
    shared
        .dirty
        .store(false, std::sync::atomic::Ordering::Release);
    shared.mark_changed();
    assert_eq!(rx.try_recv(), Ok(Event::Changed));
}

#[test]
fn kill_ends_a_plain_child_by_hangup() {
    let mut term = sh("echo up; sleep 1000");
    wait_for(&term, |s| s.text().contains("up"));
    let exit = term.kill().expect("exit");
    assert_eq!(exit.signal, Some(libc::SIGHUP));
    assert!(term.write(b"x").is_err());
}

/// A FIFO the test reads; every process that inherited the write side keeps
/// it open, so end of file means the whole group is gone.
struct Fifo {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    fd: libc::c_int,
}

impl Fifo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("alive");
        let c_path = CString::new(path.to_str().expect("utf8")).expect("nul");
        // SAFETY: valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        // A reader first, so the child's open for writing does not block.
        // SAFETY: valid path, plain flags.
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK) };
        assert!(fd >= 0);
        Self {
            _dir: dir,
            path,
            fd,
        }
    }

    /// Wait for every writer to close. Call after a writer connected.
    fn wait_all_closed(&self) {
        let mut pfd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ms = i32::try_from(WAIT.as_millis()).expect("fits");
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&raw mut pfd, 1, ms) };
        assert_eq!(ready, 1, "a process in the group is still alive");
        assert!(pfd.revents & libc::POLLHUP != 0, "revents {}", pfd.revents);
    }
}

impl Drop for Fifo {
    fn drop(&mut self) {
        // SAFETY: fd is ours.
        unsafe { libc::close(self.fd) };
    }
}

fn trapping_group() -> (Terminal, Fifo) {
    let fifo = Fifo::new();
    // The leader and a background child both ignore HUP and hold the fifo.
    let script = format!(
        "trap '' HUP; exec 3>{}; (exec sleep 1000) & echo ready; wait",
        fifo.path.display()
    );
    let options = SpawnOptions {
        kill_grace: Duration::from_millis(300),
        ..Default::default()
    };
    let term = sh_with(&script, options);
    wait_for(&term, |s| s.text().contains("ready"));
    (term, fifo)
}

#[test]
fn kill_takes_a_whole_group_including_a_child_that_traps_hup() {
    let (mut term, fifo) = trapping_group();
    let exit = term.kill().expect("exit");
    // HUP was ignored, so only the SIGKILL after the grace period ended it.
    assert_eq!(exit.signal, Some(libc::SIGKILL));
    fifo.wait_all_closed();
}

#[test]
fn dropping_a_terminal_closes_its_group() {
    let (term, fifo) = trapping_group();
    drop(term);
    fifo.wait_all_closed();
}

#[test]
fn killing_after_exit_is_quiet_and_repeatable() {
    let mut term = sh("exit 3");
    wait_exit(&term);
    assert_eq!(
        term.kill(),
        Some(Exit {
            code: Some(3),
            signal: None
        })
    );
    assert_eq!(
        term.kill(),
        Some(Exit {
            code: Some(3),
            signal: None
        })
    );
}

#[test]
fn typing_returns_to_live_and_reports_do_not() {
    let options = SpawnOptions {
        rows: 5,
        ..Default::default()
    };
    let term = sh_with("seq 1 50; exec cat", options);
    wait_for(&term, |s| s.text().contains("50"));
    term.scroll(crate::Scroll::Up(5));
    term.start_selection(
        crate::SelectionKind::Line,
        crate::GridPoint { row: 0, col: 0 },
        crate::Side::Left,
    );
    let s = term.snapshot();
    assert_eq!(s.display_offset, 5);
    assert!(!s.selection.is_empty());
    assert!(term.selected_text().is_some());

    term.write_report(b"\x1b[I").expect("report");
    assert_eq!(term.snapshot().display_offset, 5);

    term.write(b"zz\n").expect("write");
    let s = wait_for(&term, |s| s.text().contains("zz"));
    assert_eq!(s.display_offset, 0);
    assert!(s.selection.is_empty());
}

#[test]
fn a_notification_request_reaches_the_owner() {
    let term =
        sh(r"printf '\033]9;build done\007\033]777;notify;Claude;needs input\033\\'; echo after");
    wait_for(&term, |s| s.text().contains("after"));
    let notices = term.take_notifications();
    assert_eq!(notices.len(), 2);
    assert_eq!(notices[0].body, "build done");
    assert_eq!(notices[1].title, "Claude");
    assert_eq!(notices[1].body, "needs input");
    assert!(term.take_notifications().is_empty());
    // The request is not on the screen.
    assert!(!term.snapshot().text().contains("build done"));
}

#[cfg(target_os = "linux")]
#[test]
fn the_foreground_program_is_named_while_it_runs() {
    // An interactive shell gives each job its own process group.
    let mut term = sh_with("exec sh -i", SpawnOptions::default());
    assert_eq!(term.foreground_program(), None);
    term.write(b"sleep 30\n").expect("write");
    let deadline = Instant::now() + WAIT;
    while term.foreground_program().as_deref() != Some("sleep") {
        assert!(
            Instant::now() < deadline,
            "never saw sleep in the foreground"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    term.kill();
}
