//! What the screens show, read once from the service and git. Filters re-run
//! the view model over these stored comments and sheets, never the service.

use std::collections::HashSet;
use std::path::Path;

use rvw_core::operations::{
    AddInput, PullInput, QueryInput, ReviewSheet, SheetsInput, SubmitInput,
};
use rvw_core::types::Comment;
use rvw_core::Service;
use rvw_git::gitx;
use rvw_view::changes::{self, ChangedFile};
use rvw_view::diff::{self, FileDiff};
use rvw_view::queue::{self, Facets, Filter, Queue};

pub struct Snapshot {
    pub workspace: String,
    pub branch: String,
    pub comments: Vec<Comment>,
    pub sheets: Vec<ReviewSheet>,
    pub acknowledged: HashSet<String>,
    /// The queue under the default filter: the counters and the empty check.
    pub queue: Queue,
    pub facets: Facets,
    pub next_pull: NextPull,
    /// The files that differ from the base, in tree order.
    pub changes: Vec<ChangedFile>,
    /// The commit id the changes are taken against; empty when there is none.
    pub base_id: String,
    /// What the changes are compared with, for display: `main` or `HEAD`.
    /// Empty when there is no comparison.
    pub base_name: String,
    /// Why the changes cannot be shown, when they cannot.
    pub changes_note: Option<String>,
}

/// What a pull pinned to the lane would take, read without taking it.
pub struct NextPull {
    pub lane: String,
    pub count: usize,
    /// handoffs waiting in other lanes; zero when no lane is pinned
    pub elsewhere: usize,
    pub elsewhere_lanes: Vec<String>,
}

impl Snapshot {
    /// The queue under `filter`, built from the stored comments and sheets.
    pub fn view(&self, filter: &Filter) -> Queue {
        queue::build(&self.comments, &self.sheets, &self.acknowledged, filter)
    }
}

/// Reads the queue for `workspace`, the branch, what a pull pinned to `lane`
/// would take, and the files that differ from the base. An empty `lane` means
/// every lane.
///
/// A missing comparison is not an error: the changes stay empty and the note
/// says why, so the queue still opens.
///
/// # Errors
///
/// Fails with the service's message when any queue read fails.
pub fn load(service: &Service, workspace: &str, lane: &str) -> Result<Snapshot, String> {
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
    let peek = service
        .pull(PullInput {
            workspace: workspace.to_owned(),
            lane: lane.to_owned(),
            peek: true,
            ..PullInput::default()
        })
        .map_err(|e| e.message)?;

    let acknowledged = HashSet::new();
    let queue = queue::build(
        &list.comments,
        &sheets.sheets,
        &acknowledged,
        &Filter::default(),
    );
    let facets = queue::facets(&list.comments);
    let count = peek.count();
    let (elsewhere, elsewhere_lanes) = match peek.elsewhere {
        Some(found) => (usize::try_from(found.count).unwrap_or(0), found.lanes),
        None => (0, Vec::new()),
    };
    let (changes, base_id, base_name, changes_note) =
        read_changes(Path::new(workspace), &list.comments);

    Ok(Snapshot {
        workspace: workspace.to_owned(),
        branch: gitx::branch(Path::new(workspace)),
        comments: list.comments,
        sheets: sheets.sheets,
        acknowledged,
        queue,
        facets,
        next_pull: NextPull {
            lane: lane.to_owned(),
            count,
            elsewhere,
            elsewhere_lanes,
        },
        changes,
        base_id,
        base_name,
        changes_note,
    })
}

/// The changed files against the resolved base, with the base's id and name.
/// The last element is the note explaining why there are no changes to show.
fn read_changes(
    dir: &Path,
    comments: &[Comment],
) -> (Vec<ChangedFile>, String, String, Option<String>) {
    let base = match changes::resolve_base(dir) {
        Ok(base) => base,
        Err(message) => return (Vec::new(), String::new(), String::new(), Some(message)),
    };
    match gitx::changed_files(dir, &base.id) {
        Ok(found) => (
            changes::changed_files(&found, comments),
            base.id,
            base.name,
            None,
        ),
        Err(error) => (Vec::new(), base.id, base.name, Some(error.stderr)),
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
/// relative to the workspace, as the changed files list it.
///
/// # Errors
///
/// Fails with the service's message when the comment is refused.
pub fn add_comment(
    service: &Service,
    identity: &Identity,
    file: &str,
    lines: (i64, i64),
    text: &str,
) -> Result<(), String> {
    service
        .add(AddInput {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    #[test]
    fn add_comment_enqueues_on_the_range_for_the_author_and_lane() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("proj");
        fs::create_dir(&root).expect("mkdir");
        fs::write(root.join("app.py"), "one\ntwo\nthree\n").expect("write");
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-q"])
            .status()
            .expect("git init")
            .success());
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
        add_comment(&service, &identity, "app.py", (1, 2), "name this better")
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
    fn add_comment_reports_the_service_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = rvw_store::Store::open(&dir.path().join("q.db")).expect("open store");
        let service = Service::new(Box::new(store));
        let identity = Identity {
            workspace: dir.path().to_string_lossy().into_owned(),
            ..Identity::default()
        };
        let error =
            add_comment(&service, &identity, "missing.py", (1, 1), "hi").expect_err("no such file");
        assert!(!error.is_empty());
    }

    /// A workspace with `app.py` and a store; returns the tempdir (kept alive),
    /// the service and the identity.
    fn workspace_with_store() -> (tempfile::TempDir, Service, Identity) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("proj");
        fs::create_dir(&root).expect("mkdir");
        fs::write(root.join("app.py"), "one\ntwo\nthree\n").expect("write");
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-q"])
            .status()
            .expect("git init")
            .success());
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
    fn submit_review_links_two_draft_comments_as_request_changes() {
        let (_dir, service, identity) = workspace_with_store();
        add_comment(&service, &identity, "app.py", (1, 1), "first").expect("add");
        add_comment(&service, &identity, "app.py", (2, 2), "second").expect("add");
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
}
