//! The ledger's matching rules: which commit a decided comment landed in.
//! Pure. Links are found by content, never by commit id alone, time or path
//! alone; anything that matches no rule is unlinked, never guessed.

use crate::operations::LinkKind;
use crate::types::{Comment, Status};

/// A commit of the range and the facts of its change that matching needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitFiles {
    /// full commit id
    pub id: String,
    /// the files the commit changed, one fact each
    pub files: Vec<FileFact>,
}

/// What a commit did to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFact {
    /// path relative to the workspace, as comments record it
    pub path: String,
    /// git blob of the file after the commit; None when the file was deleted
    pub blob_after: Option<String>,
    /// patch-id of the commit's change to this file; None when unknown
    pub patch_id: Option<String>,
}

/// Whether two commit ids name the same commit: one is a prefix of the
/// other, the shorter has at least seven hex digits, case is ignored.
#[must_use]
pub fn same_commit(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short.len() >= 7
        && is_hex(short)
        && is_hex(long)
        && long
            .to_ascii_lowercase()
            .starts_with(&short.to_ascii_lowercase())
}

fn is_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// How `comment` links to `commit`, if it does. Only a done comment links.
/// Exact when the commit's file blob equals the comment's resolved blob;
/// otherwise `SameChange` when the commit's patch-id equals the resolution's.
#[must_use]
pub fn link(
    comment: &Comment,
    resolution_patch_id: Option<&str>,
    commit: &CommitFiles,
) -> Option<LinkKind> {
    if comment.status != Status::Done {
        return None;
    }
    let fact = commit.files.iter().find(|f| f.path == comment.file)?;
    if exact_fact(comment, fact) {
        return Some(LinkKind::Exact);
    }
    match (fact.patch_id.as_deref(), resolution_patch_id) {
        (Some(mine), Some(theirs)) if mine == theirs => Some(LinkKind::SameChange),
        _ => None,
    }
}

/// Whether `commit` left the comment's file as the comment resolved it: the
/// same blob. Needs no patch-id, so the ledger checks it before reading one.
#[must_use]
pub fn exact(comment: &Comment, commit: &CommitFiles) -> bool {
    comment.status == Status::Done
        && commit
            .files
            .iter()
            .find(|f| f.path == comment.file)
            .is_some_and(|fact| exact_fact(comment, fact))
}

fn exact_fact(comment: &Comment, fact: &FileFact) -> bool {
    !comment.resolved_file_version.is_empty()
        && fact.blob_after.as_deref() == Some(comment.resolved_file_version.as_str())
}

/// Every commit of `commits` that `comment` links to, in input order.
#[must_use]
pub fn link_all<'a>(
    comment: &Comment,
    resolution_patch_id: Option<&str>,
    commits: &'a [CommitFiles],
) -> Vec<(&'a CommitFiles, LinkKind)> {
    commits
        .iter()
        .filter_map(|commit| link(comment, resolution_patch_id, commit).map(|kind| (commit, kind)))
        .collect()
}

/// Why `comment` has no link in `commits`, in the words the audit shows.
#[must_use]
pub fn unlinked_reason(
    comment: &Comment,
    resolution_patch_id: Option<&str>,
    commits: &[CommitFiles],
) -> String {
    match comment.status {
        Status::Pending | Status::Pulled => return "not resolved".into(),
        Status::Rejected => return "rejected, no change expected".into(),
        Status::Done => {}
    }
    let touched = commits
        .iter()
        .any(|c| c.files.iter().any(|f| f.path == comment.file));
    if !touched {
        return "no commit in range touches the file".into();
    }
    if comment.resolved_file_version.is_empty() && resolution_patch_id.is_none() {
        return "no snapshot of the resolved file".into();
    }
    "no content match".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Status;

    /// A done comment on app.py that resolved to blob `RESOLVED`.
    fn done(resolved: &str) -> Comment {
        Comment {
            id: "r1".into(),
            status: Status::Done,
            workspace: "/nonexistent".into(),
            lane: None,
            author: None,
            file: "app.py".into(),
            path: "/nonexistent/app.py".into(),
            start_line: 1,
            end_line: 1,
            filetype: "python".into(),
            code: String::new(),
            comment: String::new(),
            created_at: String::new(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            points_at: None,
            reopens: None,
            verdict: None,
            file_version: String::new(),
            resolved_file_version: resolved.into(),
            review_id: String::new(),
            edited_at: String::new(),
        }
    }

    fn with_status(mut c: Comment, status: Status) -> Comment {
        c.status = status;
        c
    }

    fn fact(path: &str, blob: Option<&str>, patch_id: Option<&str>) -> FileFact {
        FileFact {
            path: path.into(),
            blob_after: blob.map(Into::into),
            patch_id: patch_id.map(Into::into),
        }
    }

    fn commit_of(id: &str, files: Vec<FileFact>) -> CommitFiles {
        CommitFiles {
            id: id.into(),
            files,
        }
    }

    const FULL: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn same_commit_takes_a_prefix_of_seven_or_more_hex_digits() {
        assert!(same_commit(FULL, FULL));
        assert!(same_commit("0123456", FULL));
        assert!(same_commit(&FULL[..12], FULL));
        assert!(same_commit("0123456789ABCDEF", FULL), "case is ignored");
        assert!(!same_commit("012345", FULL), "shorter than seven");
        assert!(!same_commit("0123456x", FULL), "not hex");
        assert!(!same_commit("1123456", FULL), "not a prefix");
        assert!(!same_commit("", ""), "empty is not a commit");
    }

    #[test]
    fn a_done_comment_links_exactly_when_the_blob_matches() {
        let c = done("blobA");
        let commits = [commit_of(
            "c1",
            vec![fact("app.py", Some("blobA"), Some("p1"))],
        )];
        assert_eq!(link(&c, None, &commits[0]), Some(LinkKind::Exact));
    }

    #[test]
    fn a_done_comment_links_same_change_on_equal_patch_ids() {
        let c = done("blobA");
        let commit = commit_of("c1", vec![fact("app.py", Some("blobOther"), Some("p1"))]);
        assert_eq!(link(&c, Some("p1"), &commit), Some(LinkKind::SameChange));
    }

    #[test]
    fn exact_beats_same_change_when_both_hold() {
        let c = done("blobA");
        let commit = commit_of("c1", vec![fact("app.py", Some("blobA"), Some("p1"))]);
        assert_eq!(link(&c, Some("p1"), &commit), Some(LinkKind::Exact));
    }

    #[test]
    fn a_missing_side_never_matches() {
        // no resolved blob on the comment: exact cannot hold
        let no_blob = done("");
        let commit = commit_of("c1", vec![fact("app.py", Some(""), Some("p1"))]);
        assert_eq!(link(&no_blob, None, &commit), None);
        // no patch-id on the commit: same-change cannot hold
        let c = done("blobA");
        let commit = commit_of("c1", vec![fact("app.py", Some("blobOther"), None)]);
        assert_eq!(link(&c, Some("p1"), &commit), None);
        // no patch-id for the resolution: same-change cannot hold
        let commit = commit_with_patch("p1");
        assert_eq!(link(&c, None, &commit), None);
    }

    fn commit_with_patch(patch: &str) -> CommitFiles {
        commit_of("c1", vec![fact("app.py", Some("blobOther"), Some(patch))])
    }

    #[test]
    fn a_path_match_alone_never_links() {
        // same file, different content and a different change
        let c = done("blobA");
        let commit = commit_of("c1", vec![fact("app.py", Some("blobB"), Some("p2"))]);
        assert_eq!(link(&c, Some("p1"), &commit), None);
        assert_eq!(
            unlinked_reason(&c, Some("p1"), &[commit]),
            "no content match"
        );
    }

    #[test]
    fn a_comment_on_another_path_never_links_even_with_equal_content() {
        let c = done("blobA");
        let commit = commit_of("c1", vec![fact("other.py", Some("blobA"), Some("p1"))]);
        assert_eq!(link(&c, Some("p1"), &commit), None);
    }

    #[test]
    fn only_done_comments_link() {
        let commit = commit_of("c1", vec![fact("app.py", Some("blobA"), Some("p1"))]);
        for status in [Status::Pending, Status::Pulled, Status::Rejected] {
            let c = with_status(done("blobA"), status);
            assert_eq!(link(&c, Some("p1"), &commit), None, "{status:?}");
            assert!(link_all(&c, Some("p1"), std::slice::from_ref(&commit)).is_empty());
        }
    }

    #[test]
    fn a_rebased_commit_links_by_blob_and_by_patch_id() {
        // the rebased commit has another id but the same blob and patch-id
        let c = done("blobA");
        let original = commit_of(
            "aaaaaaa1111",
            vec![fact("app.py", Some("blobA"), Some("p1"))],
        );
        let rebased = commit_of(
            "bbbbbbb2222",
            vec![fact("app.py", Some("blobA"), Some("p1"))],
        );
        let commits = [original, rebased];
        let got = link_all(&c, Some("p1"), &commits);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0.id, "aaaaaaa1111");
        assert_eq!(got[1].0.id, "bbbbbbb2222");
        assert!(got.iter().all(|(_, kind)| *kind == LinkKind::Exact));
    }

    #[test]
    fn a_squashed_commit_is_unlinked_with_no_content_match() {
        // the squash has another blob and another patch-id than the resolution
        let c = done("blobA");
        let squash = commit_of(
            "ccccccc3333",
            vec![fact("app.py", Some("blobSquash"), Some("p9"))],
        );
        assert_eq!(link(&c, Some("p1"), &squash), None);
        assert_eq!(
            unlinked_reason(&c, Some("p1"), &[squash]),
            "no content match"
        );
    }

    #[test]
    fn link_all_keeps_input_order_and_skips_others() {
        let c = done("blobA");
        let commits = [
            commit_of("c1", vec![fact("app.py", Some("blobX"), None)]),
            commit_of("c2", vec![fact("app.py", Some("blobA"), None)]),
            commit_of("c3", vec![fact("app.py", Some("blobY"), Some("p1"))]),
        ];
        let got = link_all(&c, Some("p1"), &commits);
        let ids: Vec<(&str, LinkKind)> = got.iter().map(|(c, k)| (c.id.as_str(), *k)).collect();
        assert_eq!(ids, [("c2", LinkKind::Exact), ("c3", LinkKind::SameChange)]);
    }

    #[test]
    fn a_pending_pulled_or_rejected_comment_gives_its_reason() {
        let commits = [commit_of(
            "c1",
            vec![fact("app.py", Some("blobA"), Some("p1"))],
        )];
        let pending = with_status(done("blobA"), Status::Pending);
        let pulled = with_status(done("blobA"), Status::Pulled);
        let rejected = with_status(done("blobA"), Status::Rejected);
        assert_eq!(
            unlinked_reason(&pending, Some("p1"), &commits),
            "not resolved"
        );
        assert_eq!(
            unlinked_reason(&pulled, Some("p1"), &commits),
            "not resolved"
        );
        assert_eq!(
            unlinked_reason(&rejected, Some("p1"), &commits),
            "rejected, no change expected"
        );
    }

    #[test]
    fn no_commit_touching_the_file_is_its_own_reason() {
        let c = done("blobA");
        let commits = [commit_of(
            "c1",
            vec![fact("other.py", Some("blobA"), Some("p1"))],
        )];
        assert_eq!(
            unlinked_reason(&c, Some("p1"), &commits),
            "no commit in range touches the file"
        );
        assert_eq!(
            unlinked_reason(&c, Some("p1"), &[]),
            "no commit in range touches the file"
        );
    }

    #[test]
    fn a_comment_with_no_snapshot_and_no_patch_id_says_so() {
        let c = done("");
        let commits = [commit_of(
            "c1",
            vec![fact("app.py", Some("blobA"), Some("p1"))],
        )];
        assert_eq!(
            unlinked_reason(&c, None, &commits),
            "no snapshot of the resolved file"
        );
        // with a resolution patch-id it can still be matched, so the reason is a content one
        assert_eq!(
            unlinked_reason(&c, Some("p9"), &commits),
            "no content match"
        );
    }

    #[test]
    fn a_deleted_file_blob_never_matches_an_exact_link() {
        let c = done("blobA");
        let commits = [commit_of("c1", vec![fact("app.py", None, None)])];
        assert_eq!(link(&c, None, &commits[0]), None);
        assert_eq!(unlinked_reason(&c, None, &commits), "no content match");
    }
}
