//! Branches and tags: create, rename, delete and put back, and the upstream a branch tracks.

use std::path::Path;

use crate::gitx::{self, owned_args, run, GitError};

/// Creates local branch `name` at `start` (HEAD when None); returns the commit it points at.
pub fn create_branch(dir: &Path, name: &str, start: Option<&str>) -> Result<String, GitError> {
    let op = ["branch"];
    check_branch(dir, &op, name)?;
    let mut args = vec!["branch", name];
    if let Some(start) = start {
        check_rev(&op, start)?;
        args.push(start);
    }
    run(dir, &[], &args)?;
    ref_id(dir, &format!("refs/heads/{name}"))
}

/// Deletes local branch `name`; returns the commit it pointed at, so it can be put back.
pub fn delete_branch(dir: &Path, name: &str, force: bool) -> Result<String, GitError> {
    check_branch(dir, &["branch"], name)?;
    let id = ref_id(dir, &format!("refs/heads/{name}"))?;
    if gitx::branch(dir) == name {
        return Err(GitError {
            args: owned_args(&["branch", name]),
            stderr: "cannot delete the branch you are on".to_owned(),
        });
    }
    run(dir, &[], &["branch", if force { "-D" } else { "-d" }, name])?;
    Ok(id)
}

/// Recreates branch `name` at `commit`; fails when a branch of that name exists.
pub fn restore_branch(dir: &Path, name: &str, commit: &str) -> Result<(), GitError> {
    let op = ["update-ref"];
    check_branch(dir, &op, name)?;
    check_rev(&op, commit)?;
    let full = format!("refs/heads/{name}");
    // An empty old value makes update-ref refuse when the ref already exists.
    run(dir, &[], &["update-ref", full.as_str(), commit, ""]).map(|_| ())
}

pub fn rename_branch(dir: &Path, from: &str, to: &str) -> Result<(), GitError> {
    let op = ["branch", "-m"];
    check_branch(dir, &op, from)?;
    check_branch(dir, &op, to)?;
    run(dir, &[], &["branch", "-m", from, to]).map(|_| ())
}

/// The upstream of branch `name`, e.g. "origin/main"; None when it tracks nothing.
pub fn upstream(dir: &Path, name: &str) -> Result<Option<String>, GitError> {
    check_branch(dir, &["rev-parse"], name)?;
    let spec = format!("{name}@{{upstream}}");
    let ran = gitx::exec(
        dir,
        &[],
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            spec.as_str(),
        ],
    )?;
    if ran.code != Some(0) {
        return Ok(None);
    }
    let out = ran.stdout.trim();
    Ok((!out.is_empty()).then(|| out.to_owned()))
}

pub fn set_upstream(dir: &Path, name: &str, upstream: &str) -> Result<(), GitError> {
    let op = ["branch"];
    check_branch(dir, &op, name)?;
    check_rev(&op, upstream)?;
    let flag = format!("--set-upstream-to={upstream}");
    run(dir, &[], &["branch", flag.as_str(), name]).map(|_| ())
}

pub fn unset_upstream(dir: &Path, name: &str) -> Result<(), GitError> {
    check_branch(dir, &["branch"], name)?;
    run(dir, &[], &["branch", "--unset-upstream", name]).map(|_| ())
}

/// Commits `name` has that its upstream lacks, and the reverse; None without an upstream.
pub fn ahead_behind(dir: &Path, name: &str) -> Result<Option<(usize, usize)>, GitError> {
    if upstream(dir, name)?.is_none() {
        return Ok(None);
    }
    let spec = format!("{name}...{name}@{{upstream}}");
    let out = run(
        dir,
        &[],
        &["rev-list", "--left-right", "--count", spec.as_str()],
    )?;
    let fields: Vec<&str> = out.split_whitespace().collect();
    let counts = match fields.as_slice() {
        [left, right] => left.parse::<usize>().ok().zip(right.parse::<usize>().ok()),
        _ => None,
    };
    match counts {
        Some(counts) => Ok(Some(counts)),
        None => Err(GitError {
            args: owned_args(&["rev-list", "--left-right", "--count", spec.as_str()]),
            stderr: format!("unexpected output '{}'", out.trim()),
        }),
    }
}

/// The remotes' names, sorted.
pub fn remotes(dir: &Path) -> Result<Vec<String>, GitError> {
    let out = run(dir, &[], &["remote"])?;
    let mut names: Vec<String> = out
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    names.sort();
    Ok(names)
}

/// Tag names, newest first.
pub fn tags(dir: &Path) -> Result<Vec<String>, GitError> {
    let out = run(dir, &[], &["tag", "--list", "--sort=-creatordate"])?;
    Ok(out
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Creates tag `name` at `rev`; annotated when `message` is non-empty. Returns what
/// the tag ref points at.
pub fn create_tag(dir: &Path, name: &str, rev: &str, message: &str) -> Result<String, GitError> {
    let op = ["tag"];
    check_tag(dir, &op, name)?;
    check_rev(&op, rev)?;
    if message.trim().is_empty() {
        run(dir, &[], &["tag", name, rev])?;
    } else {
        run(
            dir,
            message.as_bytes(),
            &["tag", "-a", name, "-F", "-", rev],
        )?;
    }
    ref_id(dir, &format!("refs/tags/{name}"))
}

/// Deletes tag `name`; returns what it pointed at (the tag object for an annotated tag).
pub fn delete_tag(dir: &Path, name: &str) -> Result<String, GitError> {
    check_tag(dir, &["tag"], name)?;
    let id = ref_id(dir, &format!("refs/tags/{name}"))?;
    run(dir, &[], &["tag", "-d", name])?;
    Ok(id)
}

/// Recreates tag `name` pointing at `target` exactly (an annotated tag keeps its object).
pub fn restore_tag(dir: &Path, name: &str, target: &str) -> Result<(), GitError> {
    let op = ["update-ref"];
    check_tag(dir, &op, name)?;
    check_rev(&op, target)?;
    let full = format!("refs/tags/{name}");
    run(dir, &[], &["update-ref", full.as_str(), target, ""]).map(|_| ())
}

/// Rejects a branch name that is empty, option-like or not a valid branch
/// name, before any command changes anything.
fn check_branch(dir: &Path, op: &[&str], name: &str) -> Result<(), GitError> {
    let invalid = || invalid_name(op, "branch", name);
    if name.is_empty() || name.starts_with('-') {
        return Err(invalid());
    }
    let ran = gitx::exec(dir, &[], &["check-ref-format", "--branch", name])?;
    // `--branch` expands `@{-N}` to a branch name, so the name it prints must be the one given.
    if ran.code == Some(0) && ran.stdout.trim_end() == name {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Rejects a tag name that is empty, option-like or not a valid ref name.
fn check_tag(dir: &Path, op: &[&str], name: &str) -> Result<(), GitError> {
    let invalid = || invalid_name(op, "tag", name);
    if name.is_empty() || name.starts_with('-') {
        return Err(invalid());
    }
    let full = format!("refs/tags/{name}");
    let ran = gitx::exec(dir, &[], &["check-ref-format", full.as_str()])?;
    if ran.code == Some(0) {
        Ok(())
    } else {
        Err(invalid())
    }
}

/// Rejects a revision that git would read as an option.
fn check_rev(op: &[&str], rev: &str) -> Result<(), GitError> {
    if rev.starts_with('-') {
        return Err(GitError {
            args: with_value(op, rev),
            stderr: format!("invalid revision '{rev}'"),
        });
    }
    Ok(())
}

fn invalid_name(op: &[&str], kind: &str, name: &str) -> GitError {
    GitError {
        args: with_value(op, name),
        stderr: format!("invalid {kind} name '{name}'"),
    }
}

/// The git command `op` with the rejected `value` as its last argument.
fn with_value(op: &[&str], value: &str) -> Vec<String> {
    let mut args = owned_args(op);
    args.push(value.to_owned());
    args
}

/// The object id the full ref name `full` points at; the ref must exist.
fn ref_id(dir: &Path, full: &str) -> Result<String, GitError> {
    let ran = gitx::exec(dir, &[], &["rev-parse", "--verify", "--quiet", full])?;
    let id = ran.stdout.trim();
    if ran.code == Some(0) && !id.is_empty() {
        Ok(id.to_owned())
    } else {
        Err(GitError {
            args: owned_args(&["rev-parse", "--verify", full]),
            stderr: format!("{full} does not exist"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};
    use std::process::Command;
    use tempfile::TempDir;

    /// The object id `rev` names in the repository at `dir`.
    fn rev_of(dir: &Path, rev: &str) -> String {
        git(dir, &["rev-parse", rev]).trim().to_owned()
    }

    /// The local branches, as `git branch --list` prints them.
    fn branch_list(dir: &Path) -> String {
        git(dir, &["branch", "--list"])
    }

    /// Commits everything with the given author and committer date.
    fn commit_at(dir: &Path, msg: &str, date: &str) {
        git(dir, &["add", "-A"]);
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["commit", "-q", "-m", msg])
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "commit: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn create_branch_at_head_or_at_an_older_commit() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "first");
        let first = rev_of(r.path(), "HEAD");
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "second");
        let second = rev_of(r.path(), "HEAD");

        assert_eq!(create_branch(r.path(), "at-head", None).unwrap(), second);
        assert_eq!(rev_of(r.path(), "refs/heads/at-head"), second);

        assert_eq!(
            create_branch(r.path(), "old", Some(first.as_str())).unwrap(),
            first
        );
        assert_eq!(rev_of(r.path(), "refs/heads/old"), first);
    }

    #[test]
    fn create_branch_rejects_invalid_names_and_creates_nothing() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let before = branch_list(r.path());

        for name in ["bad..name", "-x", ""] {
            let err = create_branch(r.path(), name, None).unwrap_err();
            assert_eq!(err.stderr, format!("invalid branch name '{name}'"));
        }
        assert!(create_branch(r.path(), "fine", Some("--force")).is_err());
        assert_eq!(branch_list(r.path()), before);
    }

    #[test]
    fn delete_branch_returns_its_commit_and_restore_brings_it_back() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let head = rev_of(r.path(), "HEAD");
        create_branch(r.path(), "topic", None).unwrap();

        assert_eq!(delete_branch(r.path(), "topic", false).unwrap(), head);
        assert!(!branch_list(r.path()).contains("topic"));

        restore_branch(r.path(), "topic", &head).unwrap();
        assert_eq!(rev_of(r.path(), "refs/heads/topic"), head);

        let err = restore_branch(r.path(), "topic", &head).unwrap_err();
        assert!(err.stderr.contains("topic"), "{err}");
        assert_eq!(rev_of(r.path(), "refs/heads/topic"), head);
    }

    #[test]
    fn delete_branch_refuses_the_current_branch() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");

        let err = delete_branch(r.path(), "main", true).unwrap_err();
        assert_eq!(err.stderr, "cannot delete the branch you are on");
        assert!(branch_list(r.path()).contains("main"));
    }

    #[test]
    fn delete_branch_of_unmerged_work_needs_force() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        git(r.path(), &["switch", "-q", "-c", "wip"]);
        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "wip work");
        let wip = rev_of(r.path(), "HEAD");
        git(r.path(), &["switch", "-q", "main"]);

        assert!(delete_branch(r.path(), "wip", false).is_err());
        assert!(branch_list(r.path()).contains("wip"));
        assert_eq!(delete_branch(r.path(), "wip", true).unwrap(), wip);
        assert!(!branch_list(r.path()).contains("wip"));
    }

    #[test]
    fn rename_branch_moves_the_name_and_refuses_an_existing_one() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let head = rev_of(r.path(), "HEAD");
        create_branch(r.path(), "old-name", None).unwrap();

        rename_branch(r.path(), "old-name", "new-name").unwrap();
        assert_eq!(rev_of(r.path(), "refs/heads/new-name"), head);
        assert!(!branch_list(r.path()).contains("old-name"));

        create_branch(r.path(), "taken", None).unwrap();
        assert!(rename_branch(r.path(), "new-name", "taken").is_err());
        assert!(rename_branch(r.path(), "new-name", "bad..name").is_err());
        assert!(branch_list(r.path()).contains("new-name"));
        assert!(branch_list(r.path()).contains("taken"));
    }

    #[test]
    fn upstream_follows_the_push_and_can_be_unset_and_set_again() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");

        let remote = TempDir::new().unwrap();
        git(remote.path(), &["init", "-q", "--bare"]);
        let url = remote.path().to_str().unwrap();
        git(r.path(), &["remote", "add", "origin", url]);
        git(r.path(), &["push", "-q", "-u", "origin", "main"]);

        assert_eq!(
            upstream(r.path(), "main").unwrap(),
            Some("origin/main".to_owned())
        );
        assert_eq!(ahead_behind(r.path(), "main").unwrap(), Some((0, 0)));

        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "second");
        assert_eq!(ahead_behind(r.path(), "main").unwrap(), Some((1, 0)));

        unset_upstream(r.path(), "main").unwrap();
        assert_eq!(upstream(r.path(), "main").unwrap(), None);
        assert_eq!(ahead_behind(r.path(), "main").unwrap(), None);

        set_upstream(r.path(), "main", "origin/main").unwrap();
        assert_eq!(
            upstream(r.path(), "main").unwrap(),
            Some("origin/main".to_owned())
        );
        assert_eq!(ahead_behind(r.path(), "main").unwrap(), Some((1, 0)));

        assert_eq!(remotes(r.path()).unwrap(), ["origin"]);
    }

    #[test]
    fn remotes_are_sorted_names() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        assert_eq!(remotes(r.path()).unwrap(), Vec::<String>::new());

        let url = r.path().to_str().unwrap();
        git(r.path(), &["remote", "add", "zeta", url]);
        git(r.path(), &["remote", "add", "alpha", url]);
        assert_eq!(remotes(r.path()).unwrap(), ["alpha", "zeta"]);
    }

    #[test]
    fn create_tag_is_lightweight_or_annotated() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let head = rev_of(r.path(), "HEAD");

        assert_eq!(create_tag(r.path(), "light", "HEAD", "").unwrap(), head);
        assert_eq!(
            git(r.path(), &["cat-file", "-t", "refs/tags/light"]).trim(),
            "commit"
        );

        let annotated = create_tag(r.path(), "v1", "HEAD", "release one\n").unwrap();
        assert_ne!(annotated, head);
        assert_eq!(rev_of(r.path(), "refs/tags/v1"), annotated);
        assert_eq!(
            git(r.path(), &["cat-file", "-t", "refs/tags/v1"]).trim(),
            "tag"
        );
    }

    #[test]
    fn delete_then_restore_an_annotated_tag_keeps_it_annotated() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let annotated = create_tag(r.path(), "v1", "HEAD", "release one\n").unwrap();

        assert_eq!(delete_tag(r.path(), "v1").unwrap(), annotated);
        assert_eq!(tags(r.path()).unwrap(), Vec::<String>::new());

        restore_tag(r.path(), "v1", &annotated).unwrap();
        assert_eq!(rev_of(r.path(), "refs/tags/v1"), annotated);
        assert_eq!(
            git(r.path(), &["cat-file", "-t", "refs/tags/v1"]).trim(),
            "tag"
        );
        assert!(restore_tag(r.path(), "v1", &annotated).is_err());
    }

    #[test]
    fn tags_are_listed_newest_first() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_at(r.path(), "old", "2020-01-01T00:00:00Z");
        create_tag(r.path(), "old-tag", "HEAD", "").unwrap();
        write(r.path(), "b.txt", "b\n");
        commit_at(r.path(), "new", "2024-01-01T00:00:00Z");
        create_tag(r.path(), "new-tag", "HEAD", "").unwrap();

        assert_eq!(tags(r.path()).unwrap(), ["new-tag", "old-tag"]);
    }

    #[test]
    fn invalid_tag_names_and_revisions_change_nothing() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");

        for name in ["bad..tag", "-t", ""] {
            let err = create_tag(r.path(), name, "HEAD", "").unwrap_err();
            assert_eq!(err.stderr, format!("invalid tag name '{name}'"));
        }
        assert!(create_tag(r.path(), "t", "-HEAD", "").is_err());
        assert_eq!(tags(r.path()).unwrap(), Vec::<String>::new());
    }
}
