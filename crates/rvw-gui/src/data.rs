//! What the screens show, read once from the service and git. Filters re-run
//! the view model over these stored comments and sheets, never the service.

use std::path::Path;

use rvw_core::Service;
use rvw_core::operations::{
    AddInput, EditInput, LedgerInput, LinkedComment, PullInput, QueryInput, RemoveInput,
    ReviewSheet, SessionsInput, SheetsInput, SubmitInput, UnlinkedComment,
};
use rvw_core::types::{Comment, Location, Session};
use rvw_git::gitx;
use rvw_view::changes::{self, ChangedFile, Compare};
use rvw_view::diff::{self, FileDiff};
use rvw_view::handoff::{self, Preview};
use rvw_view::queue::{self, Facets, Filter, Queue};

pub struct Snapshot {
    pub workspace: String,
    pub branch: String,
    pub comments: Vec<Comment>,
    pub sheets: Vec<ReviewSheet>,
    /// The queue under the default filter: the counters and the empty check.
    pub queue: Queue,
    pub facets: Facets,
    /// What a pull would take, per lane, for the lane the window is pinned to.
    pub handoff: Preview,
    /// The files that differ from the base, in tree order.
    pub changes: Vec<ChangedFile>,
    /// The commit id the changes are taken against; empty when there is none.
    pub base_id: String,
    /// What the changes are compared with, for display: `main` or `HEAD`.
    /// Empty when there is no comparison.
    pub base_name: String,
    /// The comparison the changes were read with; `None` when none resolved.
    pub compare: Option<Compare>,
    /// Why the changes cannot be shown, when they cannot.
    pub changes_note: Option<String>,
}

impl Snapshot {
    /// The queue under `filter`, built from the stored comments and sheets.
    pub fn view(&self, filter: &Filter) -> Queue {
        queue::build(&self.comments, &self.sheets, filter)
    }
}

/// Reads the queue for `workspace`, the branch, what a pull pinned to `lane`
/// would take, and the files that differ from the base. An empty `lane` means
/// every lane. A decided comment waits on the reviewer until a verdict closes it.
///
/// `compare` is the comparison to use; `None` takes the default branch and
/// falls back to the uncommitted changes.
///
/// A missing comparison is not an error: the changes stay empty and the note
/// says why, so the queue still opens.
///
/// # Errors
///
/// Fails with the service's message when any queue read fails.
pub fn load(
    service: &Service,
    workspace: &str,
    lane: &str,
    compare: Option<Compare>,
) -> Result<Snapshot, String> {
    let list = service
        .list(QueryInput {
            workspace: workspace.to_owned(),
            status: "all".into(),
            ..QueryInput::default()
        })
        .map_err(|e| e.message)?;
    let sheets = service
        .sheets(SheetsInput {
            workspace: workspace.to_owned(),
            status: "all".into(),
            ..SheetsInput::default()
        })
        .map_err(|e| e.message)?;
    // One peek with no lane sees every handoff; the preview groups them by
    // lane and works out what a pull pinned to `lane` would take.
    let peek = service
        .pull(PullInput {
            workspace: workspace.to_owned(),
            lane: String::new(),
            peek: true,
            ..PullInput::default()
        })
        .map_err(|e| e.message)?;

    let queue = queue::build(&list.comments, &sheets.sheets, &Filter::default());
    let facets = queue::facets(&list.comments);
    let handoff = handoff::preview(&peek.reviews, &peek.comments, lane);
    let read = read_changes(Path::new(workspace), &list.comments, compare);

    Ok(Snapshot {
        workspace: workspace.to_owned(),
        branch: gitx::branch(Path::new(workspace)),
        comments: list.comments,
        sheets: sheets.sheets,
        queue,
        facets,
        handoff,
        changes: read.changes,
        base_id: read.base_id,
        base_name: read.base_name,
        compare: read.compare,
        changes_note: read.note,
    })
}

/// What reading the changes found.
struct ReadChanges {
    changes: Vec<ChangedFile>,
    base_id: String,
    base_name: String,
    compare: Option<Compare>,
    /// Why there are no changes to show, when there are none to show.
    note: Option<String>,
}

/// The changed files against the base `wanted` resolves to.
fn read_changes(dir: &Path, comments: &[Comment], wanted: Option<Compare>) -> ReadChanges {
    let (compare, base) = match changes::resolve(dir, wanted) {
        Ok(found) => found,
        Err(message) => {
            return ReadChanges {
                changes: Vec::new(),
                base_id: String::new(),
                base_name: String::new(),
                compare: None,
                note: Some(message),
            };
        }
    };
    let (changes, note) = match gitx::changed_files(dir, &base.id) {
        Ok(found) => (changes::changed_files(&found, comments), None),
        Err(error) => (Vec::new(), Some(error.stderr)),
    };
    ReadChanges {
        changes,
        base_id: base.id,
        base_name: base.name,
        compare: Some(compare),
        note,
    }
}

/// Who the window writes as: the workspace, the author and the lane the
/// `--author` and `--lane` flags give. Empty author or lane means none.
#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub workspace: String,
    pub author: String,
    pub lane: String,
}

/// Enqueues one comment on the lines `start..=end` of `file`, which is a path
/// relative to the workspace, as the changed files list it. `points_at` is the
/// changed location the comment is about, when it is about one.
///
/// # Errors
///
/// Fails with the service's message when the comment is refused, for example
/// when `points_at` names a file that is not in the workspace.
pub fn add_comment(
    service: &Service,
    identity: &Identity,
    file: &str,
    lines: (i64, i64),
    text: &str,
    points_at: Option<Location>,
) -> Result<(), String> {
    service
        .add(AddInput {
            points_at,
            workspace: identity.workspace.clone(),
            file: file.to_owned(),
            start_line: lines.0,
            end_line: lines.1,
            comment: text.to_owned(),
            author: identity.author.clone(),
            lane: identity.lane.clone(),
            ..AddInput::default()
        })
        .map(|_| ())
        .map_err(|e| e.message)
}

/// Replaces the text of the pending comment `id`.
///
/// # Errors
///
/// Fails with a message to write something when `text` is blank, and
/// otherwise with the service's message, as when the comment was pulled.
pub fn edit_comment(
    service: &Service,
    identity: &Identity,
    id: &str,
    text: &str,
) -> Result<Comment, String> {
    if text.trim().is_empty() {
        return Err(BLANK_EDIT.into());
    }
    service
        .edit(EditInput {
            workspace: identity.workspace.clone(),
            id: id.to_owned(),
            comment: text.to_owned(),
        })
        .map_err(|e| e.message)
}

/// What a blank edit is refused with.
pub const BLANK_EDIT: &str = "Write something, or delete the comment.";

/// Deletes the draft comment `id` and returns it as it was.
///
/// # Errors
///
/// Fails with the service's message when the comment is not a draft: it was
/// pulled, or it belongs to a submitted review.
pub fn remove_comment(service: &Service, identity: &Identity, id: &str) -> Result<Comment, String> {
    service
        .remove(RemoveInput {
            workspace: identity.workspace.clone(),
            id: id.to_owned(),
        })
        .map_err(|e| e.message)
}

/// Submits `comment_ids` as one review with `decision` and `summary`, and
/// returns the new review's id. An empty `comment_ids` makes a summary-only
/// review, so the service never takes the author's other pending comments.
///
/// # Errors
///
/// Fails with "Nothing to submit." when there are no comments and no summary,
/// and otherwise with the service's message.
pub fn submit_review(
    service: &Service,
    identity: &Identity,
    decision: &str,
    summary: &str,
    comment_ids: Vec<String>,
) -> Result<String, String> {
    let summary = summary.trim();
    if comment_ids.is_empty() && summary.is_empty() {
        return Err("Nothing to submit.".into());
    }
    let no_comments = comment_ids.is_empty();
    service
        .submit(SubmitInput {
            workspace: identity.workspace.clone(),
            decision: decision.to_owned(),
            summary: summary.to_owned(),
            comment_ids,
            no_comments,
            lane: identity.lane.clone(),
            author: identity.author.clone(),
        })
        .map(|review| review.id)
        .map_err(|e| e.message)
}

/// The diff of one file against the base commit `base_id`.
///
/// # Errors
///
/// Fails with git's message when the diff cannot be read.
pub fn load_diff(workspace: &str, base_id: &str, path: &str) -> Result<FileDiff, String> {
    let text = gitx::unified_diff(Path::new(workspace), path, base_id, 3).map_err(|e| e.stderr)?;
    Ok(diff::parse(&text))
}

/// The stored workspace with the most handoffs waiting, for a launch outside
/// any repository. Workspaces whose directory is gone or no longer a
/// repository are skipped.
pub fn busiest_workspace(service: &Service) -> Option<String> {
    let all = service
        .workspaces(rvw_core::operations::WorkspacesInput { all: true })
        .ok()?;
    all.workspaces
        .into_iter()
        .filter(|row| rvw_git::workspace::resolve(std::path::Path::new(&row.workspace)).is_ok())
        .max_by_key(|row| (row.pending, row.pulled))
        .map(|row| row.workspace)
}

/// The sessions agents reported in `workspace`, newest first.
///
/// # Errors
///
/// Fails with the service's message when the sessions cannot be read.
pub fn sessions(service: &Service, workspace: &str) -> Result<Vec<Session>, String> {
    service
        .sessions(SessionsInput {
            workspace: workspace.to_owned(),
        })
        .map(|read| read.sessions)
        .map_err(|e| e.message)
}

/// The reviews that led to one commit: the decided comments linked to it, and
/// the comments on its files that matched no commit.
#[derive(Debug, Clone, Default)]
pub struct CommitReview {
    pub linked: Vec<LinkedComment>,
    pub unlinked: Vec<UnlinkedComment>,
}

/// The review of `commit_id`, read from the ledger of that one commit (`<id>^!`,
/// which also works for a root commit). Lanes are not scoped, so a comment of
/// any lane that landed in the commit shows. `Ok(None)` when the ledger lists
/// no such commit.
///
/// # Errors
///
/// Fails with the service's message when the ledger cannot be read.
pub fn ledger_for_commit(
    service: &Service,
    workspace: &str,
    commit_id: &str,
) -> Result<Option<CommitReview>, String> {
    let read = service
        .ledger(LedgerInput {
            workspace: workspace.to_owned(),
            range: format!("{commit_id}^!"),
            since: String::new(),
            until: String::new(),
            lane: String::new(),
        })
        .map_err(|e| e.message)?;
    Ok(read.commits.into_iter().next().map(|commit| CommitReview {
        linked: commit.comments,
        unlinked: read.unlinked,
    }))
}

#[cfg(test)]
#[path = "data_draft_tests.rs"]
mod draft_tests;

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    #[test]
    fn add_comment_enqueues_on_the_range_for_the_author_and_lane() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("proj");
        fs::create_dir(&root).expect("mkdir");
        fs::write(root.join("app.py"), "one\ntwo\nthree\n").expect("write");
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status()
                .expect("git init")
                .success()
        );
        let workspace = fs::canonicalize(&root)
            .expect("canonicalize")
            .to_string_lossy()
            .into_owned();

        let store = rvw_store::Store::open(&dir.path().join("q.db")).expect("open store");
        let service = Service::new(Box::new(store));
        let identity = Identity {
            workspace: workspace.clone(),
            author: "gus".into(),
            lane: "feat".into(),
        };
        add_comment(
            &service,
            &identity,
            "app.py",
            (1, 2),
            "name this better",
            None,
        )
        .expect("add comment");

        let list = service
            .list(QueryInput {
                workspace,
                status: "all".into(),
                ..QueryInput::default()
            })
            .expect("list");
        assert_eq!(list.comments.len(), 1);
        let comment = &list.comments[0];
        assert_eq!(comment.file, "app.py");
        assert_eq!((comment.start_line, comment.end_line), (1, 2));
        assert_eq!(comment.comment, "name this better");
        assert_eq!(comment.author.as_deref(), Some("gus"));
        assert_eq!(comment.lane.as_deref(), Some("feat"));
    }

    #[test]
    fn sessions_reads_back_a_reported_session() {
        let (_dir, service, identity) = workspace_with_store();
        assert!(
            sessions(&service, &identity.workspace)
                .expect("read")
                .is_empty()
        );
        service
            .report_session(rvw_core::operations::ReportSessionInput {
                workspace: identity.workspace.clone(),
                session_id: "s1".into(),
                tool: "claude-code".into(),
                model: "opus".into(),
                summary: "fixed the parser".into(),
                commits: vec!["abcdef1".into()],
            })
            .expect("report");

        let read = sessions(&service, &identity.workspace).expect("read");
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].session_id, "s1");
        assert_eq!(read[0].tool, "claude-code");
        assert_eq!(read[0].summary.as_deref(), Some("fixed the parser"));
        assert_eq!(read[0].commits, ["abcdef1"]);
    }

    #[test]
    fn ledger_for_commit_reads_a_root_commit_with_no_reviews() {
        let (_dir, service, identity) = workspace_with_store();
        let root = std::path::Path::new(&identity.workspace);
        for args in [
            vec!["add", "app.py"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "root",
            ],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(&args)
                    .status()
                    .expect("git")
                    .success()
            );
        }
        let id = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git");
        let id = String::from_utf8_lossy(&id.stdout).trim().to_owned();

        let review = ledger_for_commit(&service, &identity.workspace, &id)
            .expect("read")
            .expect("the commit is listed");
        assert!(review.linked.is_empty());
        assert!(review.unlinked.is_empty());
    }

    #[test]
    fn ledger_for_commit_refuses_a_commit_git_does_not_know() {
        let (_dir, service, identity) = workspace_with_store();
        assert!(ledger_for_commit(&service, &identity.workspace, "0123456789abcdef").is_err());
    }

    #[test]
    fn add_comment_reports_the_service_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = rvw_store::Store::open(&dir.path().join("q.db")).expect("open store");
        let service = Service::new(Box::new(store));
        let identity = Identity {
            workspace: dir.path().to_string_lossy().into_owned(),
            ..Identity::default()
        };
        let error = add_comment(&service, &identity, "missing.py", (1, 1), "hi", None)
            .expect_err("no such file");
        assert!(!error.is_empty());
    }

    /// A workspace with `app.py` and a store; returns the tempdir (kept alive),
    /// the service and the identity.
    pub(crate) fn workspace_with_store() -> (tempfile::TempDir, Service, Identity) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("proj");
        fs::create_dir(&root).expect("mkdir");
        fs::write(root.join("app.py"), "one\ntwo\nthree\n").expect("write");
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status()
                .expect("git init")
                .success()
        );
        let workspace = fs::canonicalize(&root)
            .expect("canonicalize")
            .to_string_lossy()
            .into_owned();
        let store = rvw_store::Store::open(&dir.path().join("q.db")).expect("open store");
        let service = Service::new(Box::new(store));
        let identity = Identity {
            workspace,
            author: "gus".into(),
            lane: "feat".into(),
        };
        (dir, service, identity)
    }

    #[test]
    fn add_comment_stores_the_points_at_reference() {
        let (_dir, service, identity) = workspace_with_store();
        let at = Location {
            file: "app.py".into(),
            start_line: 2,
            end_line: 3,
        };
        add_comment(
            &service,
            &identity,
            "app.py",
            (1, 1),
            "see the range",
            Some(at.clone()),
        )
        .expect("add");
        let comments = service
            .list(QueryInput {
                workspace: identity.workspace.clone(),
                status: "all".into(),
                ..QueryInput::default()
            })
            .expect("list")
            .comments;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].points_at, Some(at));
    }

    #[test]
    fn add_comment_refuses_a_points_at_the_service_cannot_resolve() {
        let (_dir, service, identity) = workspace_with_store();
        let missing = Location {
            file: "nope.rs".into(),
            start_line: 1,
            end_line: 1,
        };
        let past_end = Location {
            file: "app.py".into(),
            start_line: 9,
            end_line: 9,
        };
        for at in [missing, past_end] {
            let error = add_comment(
                &service,
                &identity,
                "app.py",
                (1, 1),
                "see this",
                Some(at.clone()),
            )
            .expect_err("refused");
            assert!(!error.is_empty(), "{at:?}");
        }
        let queued = service
            .list(QueryInput {
                workspace: identity.workspace.clone(),
                status: "all".into(),
                ..QueryInput::default()
            })
            .expect("list")
            .comments;
        assert!(queued.is_empty());
    }

    #[test]
    fn busiest_workspace_is_a_stored_workspace_that_still_exists() {
        let (dir, service, identity) = workspace_with_store();
        assert_eq!(busiest_workspace(&service), None);
        add_comment(&service, &identity, "app.py", (1, 1), "first", None).expect("add");
        assert_eq!(busiest_workspace(&service), Some(identity.workspace));
        fs::remove_dir_all(dir.path().join("proj")).expect("remove workspace");
        assert_eq!(busiest_workspace(&service), None);
    }

    #[test]
    fn submit_review_links_two_draft_comments_as_request_changes() {
        let (_dir, service, identity) = workspace_with_store();
        add_comment(&service, &identity, "app.py", (1, 1), "first", None).expect("add");
        add_comment(&service, &identity, "app.py", (2, 2), "second", None).expect("add");
        let ids: Vec<String> = service
            .list(QueryInput {
                workspace: identity.workspace.clone(),
                status: "all".into(),
                ..QueryInput::default()
            })
            .expect("list")
            .comments
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids.len(), 2);

        let review = submit_review(
            &service,
            &identity,
            "request-changes",
            "  Fix both.  ",
            ids.clone(),
        )
        .expect("submit");

        let sheets = service
            .sheets(SheetsInput {
                workspace: identity.workspace.clone(),
                status: "all".into(),
                ..SheetsInput::default()
            })
            .expect("sheets")
            .sheets;
        assert_eq!(sheets.len(), 1);
        let sheet = &sheets[0];
        assert_eq!(sheet.review.id, review);
        assert_eq!(sheet.review.decision, "request-changes");
        assert_eq!(sheet.review.summary, "Fix both.");
        let linked: Vec<&str> = sheet.comments.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(linked, ids.iter().map(String::as_str).collect::<Vec<_>>());
    }

    #[test]
    fn submit_review_with_no_comments_needs_a_summary() {
        let (_dir, service, identity) = workspace_with_store();
        let error = submit_review(&service, &identity, "comment", "  ", Vec::new())
            .expect_err("nothing to submit");
        assert_eq!(error, "Nothing to submit.");
    }

    #[test]
    fn submit_review_reports_a_refused_decision() {
        let (_dir, service, identity) = workspace_with_store();
        let error = submit_review(&service, &identity, "maybe", "ok", Vec::new())
            .expect_err("bad decision");
        assert!(!error.is_empty());
    }

    #[test]
    fn a_decided_comment_waits_on_the_reviewer_until_judged() {
        let (_dir, service, identity) = workspace_with_store();
        add_comment(&service, &identity, "app.py", (1, 1), "first", None).expect("add");
        let workspace = identity.workspace.clone();
        service
            .pull(PullInput {
                workspace: workspace.clone(),
                ..PullInput::default()
            })
            .expect("pull");
        service
            .resolve(rvw_core::operations::ResolveInput {
                workspace: workspace.clone(),
                id: "r1".into(),
                outcome: rvw_core::types::OUTCOME_DONE.into(),
                ..rvw_core::operations::ResolveInput::default()
            })
            .expect("resolve");

        let snapshot = load(&service, &workspace, "", None).expect("load");
        assert_eq!(snapshot.queue.counters.waiting_on_you, 1);
        assert_eq!(snapshot.queue.counters.closed, 0);
    }

    #[test]
    fn the_preview_takes_the_pinned_lane_and_counts_what_waits_elsewhere() {
        let (_dir, service, feat) = workspace_with_store();
        let other = Identity {
            lane: "other".into(),
            ..feat.clone()
        };
        add_comment(&service, &feat, "app.py", (1, 1), "in feat", None).expect("add");
        add_comment(&service, &other, "app.py", (2, 2), "in other", None).expect("add");

        let snapshot = load(&service, &feat.workspace, "feat", None).expect("load");
        assert_eq!(snapshot.handoff.will_take(), 1);
        assert_eq!(snapshot.handoff.elsewhere(), 1);
        assert_eq!(snapshot.handoff.elsewhere_lanes(), vec!["other"]);
        // a GUI comment is a pending standalone comment, so the pull takes it
        // as a draft of the reviewer's own
        assert_eq!(snapshot.handoff.drafts(), 1);
    }
}
