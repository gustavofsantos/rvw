//! The user's shell environment, for git processes started from a window
//! that was not itself started by a shell. A window opened from the Finder or
//! the Dock has the system's minimal `PATH`; a commit hook that needs a
//! program the shell's startup files add (Homebrew's, say) fails there and
//! works in a terminal. Asking the shell once, as VS Code does, closes the gap.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::gitx::{self, GitLog};

/// Printed around the environment so whatever the startup files print is
/// ignored.
const MARK: &str = "__RVW_SHELL_ENV__";

/// Reads the environment of a login, interactive `shell` and gives it to
/// every git process. The attempt is reported to the git logger either way.
/// Fails with a message when the shell does not answer within `wait` or
/// prints no environment; git then keeps the environment the window has.
pub fn adopt(shell: &str, wait: Duration) -> Result<(), String> {
    let started = Instant::now();
    let args: Vec<String> = [
        "-l",
        "-i",
        "-c",
        &format!("printf %s {MARK}; env; printf %s {MARK}"),
    ]
    .map(str::to_owned)
    .into();
    let result = run(shell, &args, wait);
    let (code, stdout_bytes, problem) = match &result {
        Ok(out) => (Some(0), out.len(), String::new()),
        Err(message) => (Some(1), 0, message.clone()),
    };
    gitx::report(&GitLog {
        program: shell.to_owned(),
        dir: PathBuf::from("."),
        args: vec!["(reads the shell environment for git)".to_owned()],
        code,
        stdout_bytes,
        stdout: String::new(),
        stderr: problem,
        took: started.elapsed(),
    });
    let env = parse(&result?);
    if env.is_empty() {
        return Err("the shell printed no environment".to_owned());
    }
    gitx::set_shell_env(env);
    Ok(())
}

/// Runs `shell` with `args` and returns its stdout, killing it after `wait`.
fn run(shell: &str, args: &[String], wait: Duration) -> Result<String, String> {
    let mut child = Command::new(shell)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start {shell}: {e}"))?;
    let mut pipe = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= wait => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{shell} took longer than {} s", wait.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = reader.join().map_err(|_| "reader panicked".to_owned())?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The variables between the two marks of `output`, one `NAME=value` per
/// line. A line that is not `NAME=...` continues the previous value, which is
/// how `env` prints a value holding a newline. Empty when the marks are
/// missing.
#[must_use]
pub fn parse(output: &str) -> Vec<(String, String)> {
    let Some(start) = output.find(MARK) else {
        return Vec::new();
    };
    let rest = &output[start + MARK.len()..];
    let Some(end) = rest.find(MARK) else {
        return Vec::new();
    };
    let mut env: Vec<(String, String)> = Vec::new();
    for line in rest[..end].lines() {
        match line.split_once('=') {
            Some((key, value)) if is_name(key) => env.push((key.to_owned(), value.to_owned())),
            _ => {
                if let Some((_, value)) = env.last_mut() {
                    value.push('\n');
                    value.push_str(line);
                }
            }
        }
    }
    env
}

fn is_name(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_is_what_sits_between_the_marks() {
        let out = format!(
            "motd noise\n{MARK}PATH=/opt/homebrew/bin:/usr/bin\nHOME=/Users/g\nMULTI=a\nb c\n  d\nEMPTY=\n{MARK}prompt"
        );
        assert_eq!(
            parse(&out),
            [
                ("PATH".to_owned(), "/opt/homebrew/bin:/usr/bin".to_owned()),
                ("HOME".to_owned(), "/Users/g".to_owned()),
                ("MULTI".to_owned(), "a\nb c\n  d".to_owned()),
                ("EMPTY".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn without_both_marks_there_is_no_environment() {
        assert!(parse("PATH=/usr/bin\n").is_empty());
        assert!(parse(&format!("{MARK}PATH=/usr/bin\n")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_that_prints_the_marked_environment_is_read() {
        let script = format!("printf %s {MARK}; echo RVW_TEST=1; printf %s {MARK}");
        let out = run(
            "/bin/sh",
            &["-c".to_owned(), script],
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(parse(&out), [("RVW_TEST".to_owned(), "1".to_owned())]);
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_that_hangs_is_killed_after_the_wait() {
        let err = run(
            "/bin/sh",
            &["-c".to_owned(), "sleep 5".to_owned()],
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(err.contains("took longer"));
    }
}
