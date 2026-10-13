//! The network: fetch, pull (fast-forward only) and push (never forced).

use std::path::Path;

use crate::gitx::{self, owned_args, run, GitError};

/// HEAD before and after a pull; equal when it was already up to date. `before` is
/// empty when the branch had no commit yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    pub before: String,
    pub after: String,
}

/// The remote a command uses by default: the current branch's upstream remote, else
/// `origin` when it exists, else the only remote; None otherwise.
pub fn default_remote(dir: &Path) -> Option<String> {
    if let Some(name) = configured_remote(dir) {
        return Some(name);
    }
    let remotes = run(dir, &[], &["remote"]).ok()?;
    let names: Vec<&str> = remotes.lines().collect();
    if names.contains(&"origin") {
        return Some("origin".to_owned());
    }
    match names.as_slice() {
        [only] => Some((*only).to_owned()),
        _ => None,
    }
}

/// Fetches `remote` (every remote when None), pruning deleted branches.
pub fn fetch(dir: &Path, remote: Option<&str>) -> Result<(), GitError> {
    match remote {
        Some(remote) => {
            let args = ["fetch", "-q", "--prune", remote];
            check_arg(&args, remote)?;
            run(dir, &[], &args).map(|_| ())
        }
        None => run(dir, &[], &["fetch", "-q", "--prune", "--all"]).map(|_| ()),
    }
}

/// Fast-forwards the current branch to its upstream.
pub fn pull(dir: &Path) -> Result<Pulled, GitError> {
    const PULL: &[&str] = &["pull", "-q", "--ff-only", "--no-rebase"];
    checked_out_branch(dir, PULL)?;
    require_upstream(dir)?;
    let before = gitx::commit(dir, "HEAD").unwrap_or_default();
    run(dir, &[], PULL)?;
    let after = gitx::commit(dir, "HEAD").ok_or_else(|| GitError {
        args: owned_args(PULL),
        stderr: "no commit after git pull".to_owned(),
    })?;
    Ok(Pulled { before, after })
}

/// Pushes the current branch to its upstream.
pub fn push(dir: &Path) -> Result<(), GitError> {
    const PUSH: &[&str] = &["push", "-q"];
    let branch = checked_out_branch(dir, PUSH)?;
    require_upstream(dir)?;
    // The refspec is spelled out, so a `remote.<name>.push` from the user's
    // config, which may force with a '+', never applies.
    let config = |key: &str| {
        let key = format!("branch.{branch}.{key}");
        run(dir, &[], &["config", "--get", &key]).map(|out| out.trim().to_owned())
    };
    let remote = config("remote")?;
    let merge = config("merge")?;
    if remote == "." || remote.starts_with('-') || merge.starts_with('+') {
        return Err(GitError {
            args: owned_args(PUSH),
            stderr: "the branch tracks a local branch; there is nothing to push to".to_owned(),
        });
    }
    let refspec = format!("refs/heads/{branch}:{merge}");
    run(dir, &[], &["push", "-q", &remote, &refspec]).map(|_| ())
}

/// Pushes the current branch to `remote` under the same name and tracks it.
pub fn publish(dir: &Path, remote: &str) -> Result<(), GitError> {
    let args = ["push", "-q", "-u", remote, "HEAD"];
    check_arg(&args, remote)?;
    let branch = checked_out_branch(dir, &args)?;
    let refspec = format!("HEAD:refs/heads/{branch}");
    let args = ["push", "-q", "-u", remote, refspec.as_str()];
    run(dir, &[], &args).map(|_| ())
}

/// Pushes tag `name` to `remote`.
pub fn push_tag(dir: &Path, remote: &str, name: &str) -> Result<(), GitError> {
    let args = ["push", "-q", remote, "refs/tags/"];
    check_arg(&args, remote)?;
    check_arg(&args, name)?;
    if name.contains(':') {
        return Err(GitError {
            args: owned_args(&args),
            stderr: format!("invalid tag name '{name}'"),
        });
    }
    let refspec = format!("refs/tags/{name}");
    let args = ["push", "-q", remote, refspec.as_str()];
    run(dir, &[], &args).map(|_| ())
}

/// The remote the current branch tracks in its config, if it names a real remote.
fn configured_remote(dir: &Path) -> Option<String> {
    let branch = gitx::branch(dir);
    if is_detached(&branch) {
        return None;
    }
    let key = format!("branch.{branch}.remote");
    let args = ["config", "--get", key.as_str()];
    let ran = gitx::exec(dir, &[], &args).ok()?;
    let name = ran.stdout.trim();
    // A branch that tracks another local branch names "." as its remote.
    if ran.code != Some(0) || name.is_empty() || name == "." {
        return None;
    }
    Some(name.to_owned())
}

/// The current branch's name. Detached HEAD and an unknown HEAD are both "not on a branch".
fn checked_out_branch(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    let branch = gitx::branch(dir);
    if branch.is_empty() || is_detached(&branch) {
        return Err(GitError {
            args: owned_args(args),
            stderr: "not on a branch".to_owned(),
        });
    }
    Ok(branch)
}

/// `gitx::branch` reports a detached HEAD as "detached <id>"; no branch name can contain a space.
fn is_detached(branch: &str) -> bool {
    branch.starts_with("detached ")
}

/// Fails unless the current branch has an upstream.
fn require_upstream(dir: &Path) -> Result<(), GitError> {
    const UPSTREAM: &[&str] = &[
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{upstream}",
    ];
    let ran = gitx::exec(dir, &[], UPSTREAM)?;
    if ran.code == Some(0) {
        return Ok(());
    }
    Err(GitError {
        args: owned_args(UPSTREAM),
        stderr: "the branch tracks no remote branch; publish it first".to_owned(),
    })
}

/// Refuses an empty name or one git would read as an option, before git runs.
fn check_arg(args: &[&str], name: &str) -> Result<(), GitError> {
    if name.is_empty() || name.starts_with('-') {
        return Err(GitError {
            args: owned_args(args),
            stderr: format!("invalid name '{name}'"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testrepo::{commit_all, git, repo, write};
    use tempfile::TempDir;

    /// An empty bare repository on branch main, playing the remote.
    fn bare_remote() -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q", "--bare", "-b", "main"]);
        dir
    }

    fn url(bare: &TempDir) -> &str {
        bare.path().to_str().unwrap()
    }

    /// A repository with one commit on main, published to a new bare `origin`.
    fn published() -> (TempDir, TempDir) {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        let bare = bare_remote();
        git(r.path(), &["remote", "add", "origin", url(&bare)]);
        publish(r.path(), "origin").unwrap();
        (r, bare)
    }

    /// A second clone of `bare`, playing someone else.
    fn clone_of(bare: &TempDir) -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["clone", "-q", url(bare), "."]);
        git(dir.path(), &["config", "user.email", "o@example.com"]);
        git(dir.path(), &["config", "user.name", "Other"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        dir
    }

    fn head_id(dir: &Path) -> String {
        gitx::commit(dir, "HEAD").unwrap()
    }

    #[test]
    fn default_remote_is_none_in_a_fresh_repo() {
        let r = repo();
        assert_eq!(default_remote(r.path()), None);
    }

    #[test]
    fn default_remote_is_origin_when_it_exists() {
        let r = repo();
        git(r.path(), &["remote", "add", "origin", "/unused"]);
        assert_eq!(default_remote(r.path()), Some("origin".to_owned()));
    }

    #[test]
    fn default_remote_is_the_only_remote() {
        let r = repo();
        git(r.path(), &["remote", "add", "up", "/unused"]);
        assert_eq!(default_remote(r.path()), Some("up".to_owned()));
    }

    #[test]
    fn default_remote_prefers_the_upstream_remote() {
        let r = repo();
        git(r.path(), &["remote", "add", "origin", "/unused"]);
        git(r.path(), &["remote", "add", "up", "/unused"]);
        git(r.path(), &["config", "branch.main.remote", "up"]);
        assert_eq!(default_remote(r.path()), Some("up".to_owned()));
    }

    #[test]
    fn publish_sets_the_upstream_and_push_sends_later_commits() {
        let (r, bare) = published();
        let upstream = git(r.path(), &["rev-parse", "--abbrev-ref", "@{upstream}"]);
        assert_eq!(upstream.trim(), "origin/main");
        assert_eq!(
            gitx::commit(bare.path(), "main"),
            gitx::commit(r.path(), "HEAD")
        );

        write(r.path(), "b.txt", "b\n");
        commit_all(r.path(), "second");
        push(r.path()).unwrap();
        assert_eq!(gitx::commit(bare.path(), "main"), Some(head_id(r.path())));
    }

    #[test]
    fn push_without_an_upstream_asks_to_publish_first() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        git(r.path(), &["remote", "add", "origin", "/unused"]);

        let want = "the branch tracks no remote branch; publish it first";
        assert_eq!(push(r.path()).unwrap_err().stderr, want);
        assert_eq!(pull(r.path()).unwrap_err().stderr, want);
    }

    #[test]
    fn push_and_pull_on_detached_head_say_not_on_a_branch() {
        let r = repo();
        write(r.path(), "a.txt", "a\n");
        commit_all(r.path(), "init");
        git(r.path(), &["checkout", "-q", "--detach"]);

        assert_eq!(push(r.path()).unwrap_err().stderr, "not on a branch");
        assert_eq!(pull(r.path()).unwrap_err().stderr, "not on a branch");
        assert_eq!(
            publish(r.path(), "origin").unwrap_err().stderr,
            "not on a branch"
        );
    }

    #[test]
    fn pull_fast_forwards_to_the_other_clones_commit_then_is_a_no_op() {
        let (r, bare) = published();
        let other = clone_of(&bare);
        write(other.path(), "b.txt", "b\n");
        commit_all(other.path(), "from other");
        push(other.path()).unwrap();
        let theirs = head_id(other.path());

        fetch(r.path(), None).unwrap();
        let pulled = pull(r.path()).unwrap();
        assert_ne!(pulled.before, pulled.after);
        assert_eq!(pulled.after, theirs);

        let again = pull(r.path()).unwrap();
        assert_eq!(again.before, again.after);
        assert_eq!(again.after, theirs);
    }

    #[test]
    fn pull_of_diverged_branches_fails_and_leaves_head() {
        let (r, bare) = published();
        let other = clone_of(&bare);
        write(other.path(), "b.txt", "b\n");
        commit_all(other.path(), "from other");
        push(other.path()).unwrap();

        write(r.path(), "c.txt", "c\n");
        commit_all(r.path(), "mine");
        fetch(r.path(), Some("origin")).unwrap();
        let before = head_id(r.path());

        let err = pull(r.path()).unwrap_err();
        assert!(err.stderr.contains("fast-forward"), "{err}");
        assert_eq!(head_id(r.path()), before);
    }

    #[test]
    fn fetch_prunes_a_branch_the_remote_deleted() {
        let (r, bare) = published();
        let other = clone_of(&bare);
        git(other.path(), &["push", "-q", "origin", "main:feature"]);
        fetch(r.path(), Some("origin")).unwrap();
        assert!(gitx::commit(r.path(), "refs/remotes/origin/feature").is_some());

        git(
            other.path(),
            &["push", "-q", "origin", "--delete", "feature"],
        );
        fetch(r.path(), Some("origin")).unwrap();
        assert_eq!(gitx::commit(r.path(), "refs/remotes/origin/feature"), None);
    }

    #[test]
    fn push_tag_sends_the_tag_to_the_remote() {
        let (r, bare) = published();
        git(r.path(), &["config", "tag.gpgSign", "false"]);
        git(r.path(), &["tag", "v1"]);

        push_tag(r.path(), "origin", "v1").unwrap();
        assert_eq!(
            gitx::commit(bare.path(), "refs/tags/v1"),
            gitx::commit(r.path(), "v1")
        );
    }

    #[test]
    fn empty_or_option_like_names_are_refused() {
        let (r, _bare) = published();
        assert!(fetch(r.path(), Some("")).is_err());
        assert!(fetch(r.path(), Some("--all")).is_err());
        assert!(publish(r.path(), "-u").is_err());
        assert!(push_tag(r.path(), "", "v1").is_err());
        assert!(push_tag(r.path(), "origin", "-v1").is_err());
        assert!(push_tag(r.path(), "origin", "a:refs/heads/main").is_err());
    }

    #[test]
    fn the_non_test_code_has_no_forced_or_mirror_push() {
        let source = include_str!("remote.rs");
        let code = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(code, _)| code);
        for banned in ["\"--force", "\"--mirror", "\"-f\"", "\"+"] {
            assert!(!code.contains(banned), "remote.rs contains {banned}");
        }
    }
}
