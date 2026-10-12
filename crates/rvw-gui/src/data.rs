//! What the queue screen shows, read once from the service. Filters re-run the
//! view model over these stored comments and sheets, never the service.

use std::collections::HashSet;
use std::path::Path;

use rvw_core::operations::{PullInput, QueryInput, ReviewSheet, SheetsInput};
use rvw_core::types::Comment;
use rvw_core::Service;
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

/// Reads the queue for `workspace`, the branch, and what a pull pinned to
/// `lane` would take. An empty `lane` means every lane.
///
/// # Errors
///
/// Fails with the service's message when any read fails.
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

    Ok(Snapshot {
        workspace: workspace.to_owned(),
        branch: rvw_git::gitx::branch(Path::new(workspace)),
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
    })
}
