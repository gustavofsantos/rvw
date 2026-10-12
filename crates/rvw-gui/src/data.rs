//! What the shell shows: the workspace, its branch and the three counters.

use std::path::Path;

use rvw_core::operations::QueryInput;
use rvw_core::types::Status;
use rvw_core::Service;

pub struct ShellData {
    pub workspace: String,
    pub branch: String,
    pub waiting_on_you: usize,
    pub with_agent: usize,
    pub in_draft: usize,
}

/// Reads the queue for `workspace` and its current branch.
///
/// # Errors
///
/// Fails with the service's message when the queue cannot be read.
pub fn load(service: &Service, workspace: &str) -> Result<ShellData, String> {
    let list = service
        .list(QueryInput {
            workspace: workspace.to_owned(),
            status: "all".into(),
            ..QueryInput::default()
        })
        .map_err(|e| e.message)?;

    // TODO: duplicates the counting that will live in rvw-view::queue; use that
    // once it is shared with this crate.
    let (mut waiting_on_you, mut with_agent, mut in_draft) = (0, 0, 0);
    for comment in &list.comments {
        match comment.status {
            Status::Pulled => with_agent += 1,
            Status::Pending if comment.review_id.is_empty() => in_draft += 1,
            Status::Done => waiting_on_you += 1,
            Status::Pending | Status::Rejected => {}
        }
    }

    Ok(ShellData {
        workspace: workspace.to_owned(),
        branch: rvw_git::gitx::branch(Path::new(workspace)),
        waiting_on_you,
        with_agent,
        in_draft,
    })
}
