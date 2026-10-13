//! End-to-end tests of the daily commit loop: stage chosen lines, commit,
//! restore a checkpoint, restore the "Before restoring" one. Each test drives the
//! library calls the Changes screen makes, in a real git repository in a temp
//! directory, and checks what git and the files on disk show afterwards. No GUI.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use rvw_git::checkpoint;
use rvw_git::commits;
use rvw_git::index;
use rvw_view::diff::{self, LineKind};
use rvw_view::patch::{lines_patch, Direction};

/// The one file most tests change.
const FILE: &str = "notes.txt";

/// Runs git in `dir` and returns its stdout; a git failure fails the test.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("git output is UTF-8")
}

/// A new repository on `main`, with an identity and no commit signing.
fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    dir
}

/// Writes `text` to `file` under `dir`, replacing its content.
fn write(dir: &Path, file: &str, text: &str) {
    fs::write(dir.join(file), text).expect("write file");
}

/// The text of `file` in the worktree.
fn read(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).expect("read file")
}

/// Makes the first commit of `dir` hold `file` with `text`.
fn first_commit(dir: &Path, file: &str, text: &str) {
    write(dir, file, text);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "first"]);
}

/// The commit HEAD names.
fn head(dir: &Path) -> String {
    commits::head(dir).expect("HEAD names a commit")
}

/// The positions of the first line matching each `(kind, text)` in `raw`, a one-file diff.
fn pick(raw: &str, wanted: &[(LineKind, &str)]) -> BTreeSet<(usize, usize)> {
    let file = diff::parse(raw);
    wanted
        .iter()
        .map(|(kind, text)| {
            file.hunks
                .iter()
                .enumerate()
                .find_map(|(h, hunk)| {
                    hunk.lines
                        .iter()
                        .position(|line| line.kind == *kind && line.text == *text)
                        .map(|l| (h, l))
                })
                .unwrap_or_else(|| panic!("no {kind:?} line '{text}' in:\n{raw}"))
        })
        .collect()
}

/// The added and removed lines of a one-file diff, as "+text" and "-text", in order.
fn changes(raw: &str) -> Vec<String> {
    diff::parse(raw)
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter_map(|line| match line.kind {
            LineKind::Added => Some(format!("+{}", line.text)),
            LineKind::Removed => Some(format!("-{}", line.text)),
            LineKind::Context => None,
        })
        .collect()
}

/// Stages the chosen lines of the unstaged diff of `file`, as the Changes screen does.
fn stage(dir: &Path, file: &str, wanted: &[(LineKind, &str)]) {
    let raw = index::unstaged_diff(dir, file).expect("unstaged diff");
    let patch = lines_patch(&raw, &pick(&raw, wanted), Direction::Stage).expect("stage patch");
    index::apply_to_index(dir, &patch, false).expect("apply to index");
}

/// Unstages the chosen lines of the staged diff of `file`, as the Changes screen does.
fn unstage(dir: &Path, file: &str, wanted: &[(LineKind, &str)]) {
    let raw = index::staged_diff(dir, file).expect("staged diff");
    let patch = lines_patch(&raw, &pick(&raw, wanted), Direction::Unstage).expect("unstage patch");
    index::apply_to_index(dir, &patch, true).expect("apply to index");
}

/// What a person can see of the repository: git's status, the staged diff, and the named files.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    status: String,
    staged: String,
    files: Vec<String>,
}

/// The snapshot of `dir` for the given worktree files.
fn snapshot(dir: &Path, files: &[&str]) -> Snapshot {
    Snapshot {
        status: git(dir, &["status", "--porcelain=v1", "--untracked-files=all"]),
        staged: git(dir, &["diff", "--cached"]),
        files: files.iter().map(|file| read(dir, file)).collect(),
    }
}

#[test]
fn committing_one_staged_added_line_of_a_file_without_a_final_newline() {
    let dir = repo();
    first_commit(dir.path(), FILE, "alpha\nbeta\ngamma");
    let worktree = "alpha\nNEW1\nbeta\nNEW2\ngamma";
    write(dir.path(), FILE, worktree);

    stage(dir.path(), FILE, &[(LineKind::Added, "NEW1")]);
    commits::commit(dir.path(), "add NEW1").expect("commit");

    assert_eq!(
        git(dir.path(), &["show", "HEAD:notes.txt"]),
        "alpha\nNEW1\nbeta\ngamma"
    );
    assert_eq!(git(dir.path(), &["diff", "--cached"]), "");
    let unstaged = index::unstaged_diff(dir.path(), FILE).expect("unstaged diff");
    assert_eq!(changes(&unstaged), vec!["+NEW2"]);
    assert_eq!(read(dir.path(), FILE), worktree);
}

#[test]
fn unstaging_a_staged_line_puts_the_index_back_to_head_and_keeps_the_worktree() {
    let dir = repo();
    first_commit(dir.path(), FILE, "alpha\nbeta\ngamma");
    let worktree = "alpha\nNEW1\nbeta\nNEW2\ngamma";
    write(dir.path(), FILE, worktree);
    stage(dir.path(), FILE, &[(LineKind::Added, "NEW1")]);
    let staged = index::staged_diff(dir.path(), FILE).expect("staged diff");
    assert_eq!(changes(&staged), vec!["+NEW1"]);

    unstage(dir.path(), FILE, &[(LineKind::Added, "NEW1")]);

    assert_eq!(git(dir.path(), &["diff", "--cached"]), "");
    assert_eq!(read(dir.path(), FILE), worktree);
    let unstaged = index::unstaged_diff(dir.path(), FILE).expect("unstaged diff");
    assert_eq!(changes(&unstaged), vec!["+NEW1", "+NEW2"]);
}

#[test]
fn restoring_a_checkpoint_from_before_a_commit_brings_back_the_staged_and_unstaged_work() {
    let dir = repo();
    first_commit(dir.path(), FILE, "one\ntwo\nthree\n");
    let old_head = head(dir.path());
    write(dir.path(), FILE, "one\nTWO\nthree\nfour\n");
    write(dir.path(), "new.txt", "new\n");
    stage(
        dir.path(),
        FILE,
        &[(LineKind::Removed, "two"), (LineKind::Added, "TWO")],
    );
    let files = [FILE, "new.txt"];
    let before = snapshot(dir.path(), &files);
    let saved = checkpoint::create(dir.path(), "before the commit", &[]).expect("checkpoint");

    commits::commit(dir.path(), "capitalise two").expect("commit");
    let committed_head = head(dir.path());
    let after_commit = snapshot(dir.path(), &files);
    assert_ne!(committed_head, old_head);

    let safety = checkpoint::restore(dir.path(), &saved).expect("restore the checkpoint");
    assert_eq!(head(dir.path()), old_head);
    assert_eq!(snapshot(dir.path(), &files), before);

    assert!(
        safety.label.starts_with("Before restoring"),
        "label: {}",
        safety.label
    );
    assert_eq!(safety.head.as_deref(), Some(committed_head.as_str()));
    checkpoint::restore(dir.path(), &safety).expect("restore the safety checkpoint");
    assert_eq!(head(dir.path()), committed_head);
    assert_eq!(snapshot(dir.path(), &files), after_commit);
}

#[test]
fn committing_an_earlier_change_keeps_the_old_last_line_without_a_newline() {
    let dir = repo();
    first_commit(dir.path(), FILE, "one\ntwo\nthree");
    write(dir.path(), FILE, "ONE\ntwo\nthree\nfour");
    stage(
        dir.path(),
        FILE,
        &[(LineKind::Removed, "one"), (LineKind::Added, "ONE")],
    );

    commits::commit(dir.path(), "capitalise one").expect("commit");

    assert_eq!(
        git(dir.path(), &["show", "HEAD:notes.txt"]),
        "ONE\ntwo\nthree"
    );
    assert_eq!(read(dir.path(), FILE), "ONE\ntwo\nthree\nfour");
    let unstaged = index::unstaged_diff(dir.path(), FILE).expect("unstaged diff");
    assert_eq!(changes(&unstaged), vec!["-three", "+three", "+four"]);
    assert!(
        unstaged.contains(
            "-three\n\\ No newline at end of file\n+three\n+four\n\\ No newline at end of file\n"
        ),
        "{unstaged}"
    );
}
