//! What the screens show, read once from the service and git. Filters re-run
//! the view model over these stored comments and sheets, never the service.

use std::collections::HashSet;
use std::path::Path;

use rvw_core::operations::{PullInput, QueryInput, ReviewSheet, SheetsInput};
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

/// The diff of one file against the base commit `base_id`.
///
/// # Errors
///
/// Fails with git's message when the diff cannot be read.
pub fn load_diff(workspace: &str, base_id: &str, path: &str) -> Result<FileDiff, String> {
    let text = gitx::unified_diff(Path::new(workspace), path, base_id, 3).map_err(|e| e.stderr)?;
    Ok(diff::parse(&text))
}
