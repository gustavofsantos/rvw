//! The service: every review operation against a [`Repository`].
//! Port of internal/review/service.go. Synchronous; no context.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use time::{OffsetDateTime, UtcOffset};

use crate::error::{Error, Result};
use crate::evidence::{filetype_of, snapshot_diff};
use crate::operations::{
    AddInput, CountOutput, EditInput, Elsewhere, Evidence, GetInput, ListOutput, PullInput,
    PullOutput, QueryInput, RemoveInput, ResolveInput, ReviewSheet, SHEET_FILTERS, SheetState,
    SheetsInput, SheetsOutput, SubmitInput, WorkspaceSummary, WorkspacesInput, WorkspacesOutput,
};
use crate::repository::{Repository, Tx};
use crate::types::{
    Comment, Handoff, LineRange, Location, OUTCOME_DONE, OUTCOME_REJECTED, Review, ReviewStatus,
    Status, comment_id, review_id, statuses_of, validate_decision,
};

/// The largest file, or buffer, a comment snapshots.
pub const MAX_SOURCE: usize = 4 << 20;

/// Runs every review operation against a [`Repository`].
pub struct Service {
    repo: Box<dyn Repository>,
    now: Box<dyn Fn() -> OffsetDateTime + Send + Sync>,
}

// Operations take their input by value, as the adapters build it per call.
// Errors are documented per operation in the port's module docs.
#[allow(clippy::needless_pass_by_value, clippy::missing_errors_doc)]
impl Service {
    /// A service over `repo`, stamping times with the wall clock.
    pub fn new(repo: Box<dyn Repository>) -> Self {
        Self::with_clock(repo, Box::new(OffsetDateTime::now_utc))
    }

    /// A service over `repo` that stamps times with `now`.
    pub fn with_clock(
        repo: Box<dyn Repository>,
        now: Box<dyn Fn() -> OffsetDateTime + Send + Sync>,
    ) -> Self {
        Self { repo, now }
    }

    /// Where the service keeps its data.
    pub fn location(&self) -> String {
        self.repo.location()
    }

    pub(crate) fn stamp(&self) -> String {
        format_time((self.now)())
    }

    /// Runs `f` in one read-only transaction on the workspace.
    pub(crate) fn read<T>(&self, ws: &str, f: impl FnOnce(&mut dyn Tx) -> Result<T>) -> Result<T> {
        check_workspace(ws)?;
        run(|body| self.repo.view(ws, body), f)
    }

    /// Runs `f` in one write transaction on the workspace; an error rolls back.
    pub(crate) fn write<T>(&self, ws: &str, f: impl FnOnce(&mut dyn Tx) -> Result<T>) -> Result<T> {
        check_workspace(ws)?;
        run(|body| self.repo.update(ws, body), f)
    }

    // ── add / submit ─────────────────────────────────────────────────────────

    /// Enqueues one comment, snapshotting the reviewed lines, and, for a file
    /// tracked by git, the whole reviewed version as a blob.
    pub fn add(&self, input: AddInput) -> Result<Comment> {
        check_workspace(&input.workspace)?;
        if input.comment.trim().is_empty() {
            return Err(Error::invalid("a review comment needs text"));
        }
        let end_line = if input.end_line == 0 {
            input.start_line
        } else {
            input.end_line
        };
        let lines = LineRange {
            start_line: input.start_line,
            end_line,
        }
        .normalized()?;
        let path = Self::file_path(&input.workspace, &input.file)?;
        let rel = rvw_git::workspace::in_workspace(Path::new(&input.workspace), Path::new(&path))
            .ok_or_else(|| {
            Error::invalid(format!(
                "{path} is outside the workspace {}",
                input.workspace
            ))
        })?;

        let source = read_source(&path, input.source.as_deref())?;
        let source_lines = split_source(&source);
        let total = to_i64(source_lines.len());
        if lines.start_line > total {
            return Err(Error::invalid(format!(
                "lines {} is past the end of {path} ({total} lines)",
                lines.label()
            )));
        }
        let end = lines.end_line.min(total);
        let code = source_lines[to_index(lines.start_line - 1)..to_index(end)].join("\n");
        // Checked before anything is written: the git blob and the queue.
        let points_at = input
            .points_at
            .as_ref()
            .map(|loc| Self::points_at_location(&input.workspace, loc))
            .transpose()?;

        let file_version = snapshot_reviewed(&input.workspace, &rel, &source)?;

        let filetype = if input.filetype.is_empty() {
            filetype_of(&path)
        } else {
            input.filetype.clone()
        };
        let c = Comment {
            points_at,
            reopens: None,
            verdict: None,
            id: String::new(),
            status: Status::Pending,
            workspace: input.workspace.clone(),
            lane: str_opt(&input.lane),
            author: str_opt(&input.author),
            file: rel,
            path,
            start_line: lines.start_line,
            end_line: end,
            filetype,
            code,
            comment: trim_newlines(&input.comment).to_string(),
            created_at: String::new(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version,
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        };
        self.write(&input.workspace, |tx| {
            let seq = tx.next_comment_seq()?;
            let mut c = c;
            c.id = comment_id(seq);
            c.created_at = self.stamp();
            tx.insert_comment(&c)?;
            Ok(c)
        })
    }

    /// Records one review over an explicit, all-or-nothing set of comments.
    pub fn submit(&self, input: SubmitInput) -> Result<Review> {
        let summary = input.summary.trim().to_string();
        if summary.is_empty() {
            return Err(Error::invalid("a submitted review needs a summary"));
        }
        validate_decision(&input.decision)?;
        if input.no_comments && !input.comment_ids.is_empty() {
            return Err(Error::invalid("pass comment ids or no_comments, not both"));
        }
        let lane = str_opt(&input.lane);
        let author = str_opt(&input.author);

        self.write(&input.workspace, |tx| {
            let ids: Vec<String> = if input.no_comments {
                Vec::new()
            } else if !input.comment_ids.is_empty() {
                unique(&input.comment_ids)
            } else {
                tx.comments(&[Status::Pending])?
                    .into_iter()
                    .filter(|c| c.lane == lane && c.author == author && c.review_id.is_empty())
                    .map(|c| c.id)
                    .collect()
            };
            for id in &ids {
                let c = find(tx, id)?;
                if c.status != Status::Pending {
                    return Err(Error::conflict(format!("{} is not pending", c.id)));
                }
                if !c.review_id.is_empty() {
                    return Err(Error::conflict(format!(
                        "{} already belongs to {}",
                        c.id, c.review_id
                    )));
                }
                if c.lane != lane {
                    return Err(Error::conflict(format!(
                        "{} belongs to a different lane",
                        c.id
                    )));
                }
                if c.author != author {
                    return Err(Error::conflict(format!(
                        "{} belongs to a different author",
                        c.id
                    )));
                }
            }
            let seq = tx.next_review_seq()?;
            let r = Review {
                id: review_id(seq),
                status: ReviewStatus::Pending,
                workspace: input.workspace.clone(),
                lane: lane.clone(),
                author: author.clone(),
                decision: input.decision.clone(),
                summary: summary.clone(),
                comment_ids: ids,
                created_at: self.stamp(),
                pulled_at: None,
            };
            tx.insert_review(&r)?;
            Ok(r)
        })
    }

    // ── reads ────────────────────────────────────────────────────────────────

    /// The comments a query scopes to, oldest first. Nothing moves.
    pub fn list(&self, input: QueryInput) -> Result<ListOutput> {
        let statuses = statuses_of(&input.status)?;
        let scope = Self::scope(&input.workspace, &input.file, &input.lane)?;
        let comments = self.read(&input.workspace, |tx| {
            Ok(scope.comments(tx.comments(&statuses)?))
        })?;
        Ok(ListOutput {
            workspace: input.workspace,
            comments,
        })
    }

    /// Counts what a query scopes to. For pending it counts handoffs: a
    /// submitted review once, with its linked comments.
    pub fn count(&self, input: QueryInput) -> Result<CountOutput> {
        let statuses = statuses_of(&input.status)?;
        let scope = Self::scope(&input.workspace, &input.file, &input.lane)?;
        let counts_comments = !input.status.is_empty() && input.status != "pending";
        let count = self.read(&input.workspace, |tx| {
            let comments = scope.comments(tx.comments(&statuses)?);
            if counts_comments {
                return Ok(to_i64(comments.len()));
            }
            let reviews = scope.reviews(tx.reviews(&[ReviewStatus::Pending])?, &comments);
            let grouped = linked_ids(&reviews);
            let mut count = to_i64(reviews.len());
            for c in &comments {
                if !grouped.contains(&c.id) {
                    count += 1;
                }
            }
            Ok(count)
        })?;
        Ok(CountOutput {
            workspace: input.workspace,
            count,
        })
    }

    /// Returns one comment.
    pub fn get(&self, input: GetInput) -> Result<Comment> {
        self.read(&input.workspace, |tx| find(tx, &input.id))
    }

    /// Returns one comment and, once it is done, the diff between the file as
    /// reviewed and as resolved.
    pub fn evidence(&self, input: GetInput) -> Result<Evidence> {
        let comment = self.get(input.clone())?;
        let (diff, diff_available) = if comment.status == Status::Done {
            snapshot_diff(&comment)
        } else {
            (Vec::new(), false)
        };
        Ok(Evidence {
            comment,
            diff,
            diff_available,
        })
    }

    /// Returns a submitted review with its linked comments.
    pub fn sheet(&self, input: GetInput) -> Result<ReviewSheet> {
        self.read(&input.workspace, |tx| {
            let r = tx.review(&input.id)?.ok_or_else(|| {
                Error::not_found(format!(
                    "no submitted review '{}' in this workspace",
                    input.id
                ))
            })?;
            sheet_of(tx, r)
        })
    }

    /// Returns the submitted reviews a query scopes to, oldest first, each with
    /// its linked comments. Nothing moves.
    pub fn sheets(&self, input: SheetsInput) -> Result<SheetsOutput> {
        // Validate the filter before anything else reads the store.
        sheet_matches(&input.status, SheetState::Pending)?;
        let scope = Self::scope(&input.workspace, &input.file, &input.lane)?;
        let sheets = self.read(&input.workspace, |tx| {
            let reviews = tx.reviews(&[])?;
            // A review's comments move on without it (pulled, then decided), so
            // match it against its comments in every status.
            let comments = tx.comments(&[])?;
            let mut kept = Vec::new();
            for r in scope.reviews(reviews, &scope.comments(comments)) {
                let sheet = sheet_of(tx, r)?;
                if sheet_matches(&input.status, sheet.state)? {
                    kept.push(sheet);
                }
            }
            Ok(kept)
        })?;
        Ok(SheetsOutput {
            workspace: input.workspace,
            count: to_i64(sheets.len()),
            sheets,
        })
    }

    /// Lists every workspace in the store with its pending handoffs.
    pub fn workspaces(&self, input: WorkspacesInput) -> Result<WorkspacesOutput> {
        let mut rows = Vec::new();
        for ws in self.repo.workspaces()? {
            let mut row = WorkspaceSummary {
                workspace: ws.clone(),
                pending: 0,
                pulled: 0,
            };
            self.repo.view(&ws, &mut |tx: &mut dyn Tx| {
                let reviews = tx.reviews(&[ReviewStatus::Pending])?;
                let comments = tx.comments(&[Status::Pending, Status::Pulled])?;
                let grouped = linked_ids(&reviews);
                row.pending = to_i64(reviews.len());
                for c in &comments {
                    if c.status == Status::Pulled {
                        row.pulled += 1;
                    } else if !grouped.contains(&c.id) {
                        row.pending += 1;
                    }
                }
                Ok(())
            })?;
            if input.all || row.pending > 0 {
                rows.push(row);
            }
        }
        Ok(WorkspacesOutput { workspaces: rows })
    }

    // ── pull ─────────────────────────────────────────────────────────────────

    /// Dequeues handoffs: submitted reviews (with every linked comment) and
    /// standalone comments. Pulled handoffs never come back, unless peeking.
    pub fn pull(&self, input: PullInput) -> Result<PullOutput> {
        if input.limit < 0 {
            return Err(Error::invalid("limit must be >= 1"));
        }
        let scope = Self::scope(&input.workspace, &input.file, &input.lane)?;
        self.write(&input.workspace, |tx| {
            let pending_reviews = tx.reviews(&[ReviewStatus::Pending])?;
            let pending_comments = tx.comments(&[Status::Pending])?;
            let mut reviews = scope.reviews(
                pending_reviews.clone(),
                &scope.comments(pending_comments.clone()),
            );
            let grouped = linked_ids(&reviews);
            let mut comments: Vec<Comment> = scope
                .comments(pending_comments.clone())
                .into_iter()
                .filter(|c| !grouped.contains(&c.id))
                .collect();

            if !input.ids.is_empty() {
                (reviews, comments) = select_ids(&unique(&input.ids), reviews, &comments)?;
            }
            if input.limit > 0 {
                (reviews, comments) = oldest(
                    usize::try_from(input.limit).unwrap_or(usize::MAX),
                    reviews,
                    comments,
                );
            }

            let stamp = self.stamp();
            let mut handoffs = Vec::new();
            for review in reviews {
                let mut h = Handoff {
                    review,
                    comments: Vec::new(),
                };
                for id in &h.review.comment_ids {
                    let Some(mut c) = tx.comment(id)? else {
                        continue;
                    };
                    // Resolve refuses a comment of an unpulled review, so this
                    // holds; never let a pull overwrite a recorded decision.
                    if c.status != Status::Pending {
                        return Err(Error::conflict(format!(
                            "{} of {} is already {}",
                            c.id, h.review.id, c.status
                        )));
                    }
                    if !input.peek {
                        c.status = Status::Pulled;
                        c.pulled_at = Some(stamp.clone());
                        tx.update_comment(&c)?;
                    }
                    h.comments.push(c);
                }
                if !input.peek {
                    h.review.status = ReviewStatus::Pulled;
                    h.review.pulled_at = Some(stamp.clone());
                    tx.update_review(&h.review)?;
                }
                handoffs.push(h);
            }
            let mut taken = Vec::new();
            for mut c in comments {
                if !input.peek {
                    c.status = Status::Pulled;
                    c.pulled_at = Some(stamp.clone());
                    tx.update_comment(&c)?;
                }
                taken.push(c);
            }

            let mut out = PullOutput {
                workspace: input.workspace.clone(),
                drained: !input.peek,
                reviews: handoffs,
                comments: taken,
                elsewhere: None,
            };
            if out.count() == 0 && !input.lane.is_empty() {
                out.elsewhere = elsewhere(&pending_reviews, &pending_comments, &input.lane);
            }
            Ok(out)
        })
    }

    // ── decisions and edits ──────────────────────────────────────────────────

    /// Records what became of a comment and who decided. A decision is final.
    /// Resolving as done keeps the file's current tracked version as a blob.
    pub fn resolve(&self, input: ResolveInput) -> Result<Comment> {
        if input.outcome != OUTCOME_DONE && input.outcome != OUTCOME_REJECTED {
            return Err(Error::invalid(format!(
                "outcome \"{}\" is not done or rejected",
                input.outcome
            )));
        }
        if input.outcome == OUTCOME_REJECTED && input.note.trim().is_empty() {
            return Err(Error::invalid("rejecting a comment needs a reason"));
        }
        // Git runs before the write transaction, so a slow git never holds the
        // database lock; the checks run again inside it, where they count.
        let c = self.read(&input.workspace, |tx| {
            resolvable(tx, &input.id, &input.outcome)
        })?;
        let version = if input.outcome == OUTCOME_DONE {
            snapshot_resolved(&input.workspace, &c)?
        } else {
            String::new()
        };
        self.write(&input.workspace, |tx| {
            let mut c = resolvable(tx, &input.id, &input.outcome)?;
            c.status = outcome_status(&input.outcome);
            c.resolved_file_version = version;
            c.resolved_at = Some(self.stamp());
            c.resolved_by = str_opt(&input.author);
            c.resolution_note = str_opt(trim_newlines(&input.note));
            tx.update_comment(&c)?;
            Ok(c)
        })
    }

    /// Replaces a pending comment's text. Where it points and what it
    /// snapshotted stay.
    pub fn edit(&self, input: EditInput) -> Result<Comment> {
        if input.comment.trim().is_empty() {
            return Err(Error::invalid(
                "refusing to replace a comment with empty text",
            ));
        }
        self.write(&input.workspace, |tx| {
            let mut c = find(tx, &input.id)?;
            // Once pulled, the text is what was asked; rewriting it would falsify
            // the record the decision answers.
            if c.status != Status::Pending {
                return Err(Error::conflict(format!(
                    "{} is {} — only a pending comment can be edited",
                    c.id, c.status
                )));
            }
            c.comment = trim_newlines(&input.comment).to_string();
            c.edited_at = self.stamp();
            tx.update_comment(&c)?;
            Ok(c)
        })
    }

    /// Deletes a comment that is still in the reviewer's draft: pending, not
    /// part of a submitted review, and not the follow-up of a reopen (a verdict
    /// points at that). Returns it as it was. Its id is not issued again.
    pub fn remove(&self, input: RemoveInput) -> Result<Comment> {
        self.write(&input.workspace, |tx| {
            let c = find(tx, &input.id)?;
            if c.status != Status::Pending {
                return Err(Error::conflict(format!(
                    "{} is {} — only a pending comment can be removed",
                    c.id, c.status
                )));
            }
            if !c.review_id.is_empty() {
                return Err(Error::conflict(format!(
                    "{} belongs to review {} — only a comment outside a submitted review can be removed",
                    c.id, c.review_id
                )));
            }
            if let Some(reopened) = &c.reopens {
                return Err(Error::conflict(format!(
                    "{} reopens {reopened} — withdraw it by rejecting it instead",
                    c.id
                )));
            }
            tx.delete_comment(&c.id)?;
            Ok(c)
        })
    }

    // ── scoping and helpers ──────────────────────────────────────────────────

    fn scope(ws: &str, file: &str, lane: &str) -> Result<Scope> {
        let path = if file.is_empty() {
            String::new()
        } else {
            Self::file_path(ws, file)?
        };
        Ok(Scope {
            path,
            lane: lane.to_string(),
        })
    }

    /// Canonicalizes a file named absolutely or relative to the workspace.
    fn file_path(ws: &str, file: &str) -> Result<String> {
        let joined = if Path::new(file).is_absolute() {
            PathBuf::from(file)
        } else {
            Path::new(ws).join(file)
        };
        let path =
            rvw_git::workspace::canonical(&joined).map_err(|e| Error::internal(e.to_string()))?;
        Ok(path.to_string_lossy().into_owned())
    }

    /// Resolves a points-at range: a regular file inside the workspace, stored
    /// relative to it, with the lines inside the file. A reversed range is
    /// swapped and the end clamped to the file's last line.
    fn points_at_location(ws: &str, loc: &Location) -> Result<Location> {
        let lines = LineRange {
            start_line: loc.start_line,
            end_line: loc.end_line,
        }
        .normalized()
        .map_err(|_| Error::invalid("points-at lines are 1-indexed; N must be >= 1"))?;
        let path = Self::file_path(ws, &loc.file)?;
        let rel =
            rvw_git::workspace::in_workspace(Path::new(ws), Path::new(&path)).ok_or_else(|| {
                Error::invalid(format!("points-at {path} is outside the workspace {ws}"))
            })?;
        if !Path::new(&path).exists() {
            return Err(Error::not_found(format!("points-at {path} does not exist")));
        }
        let source = read_source(&path, None)
            .map_err(|e| Error::new(e.kind, format!("points-at {}", e.message)))?;
        let total = to_i64(split_source(&source).len());
        if lines.start_line > total {
            return Err(Error::invalid(format!(
                "points-at lines {} is past the end of {path} ({total} lines)",
                lines.label()
            )));
        }
        Ok(Location {
            file: rel,
            start_line: lines.start_line,
            end_line: lines.end_line.min(total),
        })
    }
}

/// Runs `body` as one transaction and returns what `f` produced. `f` runs
/// exactly once; an error from either side is returned unchanged.
fn run<T>(
    access: impl FnOnce(&mut dyn FnMut(&mut dyn Tx) -> Result<()>) -> Result<()>,
    f: impl FnOnce(&mut dyn Tx) -> Result<T>,
) -> Result<T> {
    let mut f = Some(f);
    let mut out = None;
    access(&mut |tx: &mut dyn Tx| {
        if let Some(f) = f.take() {
            out = Some(f(tx)?);
        }
        Ok(())
    })?;
    out.ok_or_else(|| Error::internal("the transaction did not run"))
}

/// Refuses a workspace an adapter forgot to resolve: an empty or relative path
/// would silently open a separate queue.
fn check_workspace(ws: &str) -> Result<()> {
    if ws.is_empty() || !Path::new(ws).is_absolute() {
        return Err(Error::invalid(format!(
            "workspace '{ws}' is not an absolute path"
        )));
    }
    Ok(())
}

/// The reviewed content: the buffer when given, else the regular file at
/// `path`. Only a regular file is opened, so a FIFO or a device never blocks
/// or floods the read.
pub(crate) fn read_source(path: &str, buffer: Option<&str>) -> Result<String> {
    let too_large = || Error::invalid(format!("{path} is larger than {} MiB", MAX_SOURCE >> 20));
    if let Some(buffer) = buffer {
        if buffer.len() > MAX_SOURCE {
            return Err(too_large());
        }
        return Ok(buffer.to_string());
    }
    let meta = fs::metadata(path).map_err(|_| {
        Error::not_found(format!(
            "{path} does not exist — pass its content to snapshot unsaved changes"
        ))
    })?;
    if !meta.is_file() {
        return Err(Error::invalid(format!("{path} is not a regular file")));
    }
    let file =
        File::open(path).map_err(|e| Error::not_found(format!("{path} cannot be read: {e}")))?;
    let mut data = Vec::new();
    file.take(to_u64(MAX_SOURCE) + 1)
        .read_to_end(&mut data)
        .map_err(|e| Error::internal(format!("cannot read {path}: {e}")))?;
    if data.len() > MAX_SOURCE {
        return Err(too_large());
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

/// Keeps the reviewed content of a tracked file as a git blob. A git failure
/// refuses the comment rather than enqueue it without its version.
pub(crate) fn snapshot_reviewed(ws: &str, rel: &str, source: &str) -> Result<String> {
    let dir = Path::new(ws);
    let tracked = rvw_git::gitx::tracked(dir, rel).map_err(|e| {
        Error::internal(format!(
            "cannot inspect Git metadata for {rel}: {}",
            e.stderr
        ))
    })?;
    if !tracked {
        return Ok(String::new());
    }
    rvw_git::gitx::store_content(dir, source).map_err(|e| {
        Error::internal(format!(
            "cannot store the reviewed version of {rel}: {}",
            e.stderr
        ))
    })
}

/// Keeps the current version of a comment's file, when it is still tracked at
/// the recorded path inside the workspace.
pub(crate) fn snapshot_resolved(ws: &str, c: &Comment) -> Result<String> {
    let recorded = if c.path.is_empty() {
        c.file.clone()
    } else {
        c.path.clone()
    };
    if recorded.is_empty() {
        return Ok(String::new());
    }
    let joined = if Path::new(&recorded).is_absolute() {
        PathBuf::from(&recorded)
    } else {
        Path::new(ws).join(&recorded)
    };
    // A path that does not resolve has no snapshot.
    let Ok(path) = rvw_git::workspace::canonical(&joined) else {
        return Ok(String::new());
    };
    // A path outside the workspace, or not a file, has no snapshot.
    let Some(rel) = rvw_git::workspace::in_workspace(Path::new(ws), &path) else {
        return Ok(String::new());
    };
    if !is_file(&path) {
        return Ok(String::new());
    }
    let label = if c.file.is_empty() {
        rel.clone()
    } else {
        c.file.clone()
    };
    let dir = Path::new(ws);
    let tracked = rvw_git::gitx::tracked(dir, &rel).map_err(|e| {
        Error::internal(format!(
            "cannot inspect Git metadata for {label}: {}",
            e.stderr
        ))
    })?;
    if !tracked {
        return Ok(String::new());
    }
    match rvw_git::gitx::store_file(dir, &rel) {
        Ok(id) => Ok(id),
        Err(_) if !is_file(&path) => Ok(String::new()),
        Err(e) => Err(Error::internal(format!(
            "cannot store the resolved version of {label}: {}",
            e.stderr
        ))),
    }
}

/// Finds a comment that may take this outcome: not yet decided, not waiting in
/// a review nobody has pulled, and pulled before it is done. A pending comment
/// may still be rejected: that retracts it.
fn resolvable(tx: &mut dyn Tx, id: &str, outcome: &str) -> Result<Comment> {
    let c = find(tx, id)?;
    if !c.review_id.is_empty()
        && let Some(r) = tx.review(&c.review_id)?
        && r.status == ReviewStatus::Pending
    {
        return Err(Error::conflict(format!(
            "{} belongs to {} — pull {} first",
            c.id, r.id, r.id
        )));
    }
    if c.status.resolved() {
        return Err(Error::conflict(format!(
            "{} is already {} — it cannot be re-decided",
            c.id, c.status
        )));
    }
    if outcome == OUTCOME_DONE && c.status != Status::Pulled {
        return Err(Error::conflict(format!(
            "{} is {} — pull it before resolving it as done",
            c.id, c.status
        )));
    }
    Ok(c)
}

fn outcome_status(outcome: &str) -> Status {
    if outcome == OUTCOME_DONE {
        Status::Done
    } else {
        Status::Rejected
    }
}

/// The scope of a lane-pinned, file-scoped request. Lane scoping is strict:
/// a pinned request never sees an unlaned or another lane's comment.
struct Scope {
    /// canonical file path, "" for every file
    path: String,
    /// "" for every lane
    lane: String,
}

#[allow(clippy::needless_pass_by_value)] // the scope filters consume the lists they are given
impl Scope {
    fn in_lane(&self, lane: Option<&str>) -> bool {
        self.lane.is_empty() || lane == Some(self.lane.as_str())
    }

    fn comments(&self, cs: Vec<Comment>) -> Vec<Comment> {
        cs.into_iter()
            .filter(|c| {
                (self.path.is_empty() || c.path == self.path) && self.in_lane(c.lane.as_deref())
            })
            .collect()
    }

    /// Keeps the reviews in the lane and, when scoped to a file, those linking
    /// at least one of the in-scope comments.
    fn reviews(&self, rs: Vec<Review>, in_scope: &[Comment]) -> Vec<Review> {
        let matching: HashSet<&str> = in_scope.iter().map(|c| c.id.as_str()).collect();
        rs.into_iter()
            .filter(|r| {
                self.in_lane(r.lane.as_deref())
                    && (self.path.is_empty()
                        || r.comment_ids
                            .iter()
                            .any(|id| matching.contains(id.as_str())))
            })
            .collect()
    }
}

/// Gathers a review's linked comments and derives its state.
fn sheet_of(tx: &mut dyn Tx, r: Review) -> Result<ReviewSheet> {
    let mut complete = r.status == ReviewStatus::Pulled;
    let mut comments = Vec::new();
    for id in &r.comment_ids {
        let Some(c) = tx.comment(id)? else {
            complete = false;
            continue;
        };
        complete = complete && c.status.resolved();
        comments.push(c);
    }
    let state = if complete {
        SheetState::Complete
    } else if r.status == ReviewStatus::Pulled {
        SheetState::Pulled
    } else {
        SheetState::Pending
    };
    Ok(ReviewSheet {
        review: r,
        state,
        comments,
    })
}

/// Whether a sheet in `state` passes the filter; "" is pending. An unknown
/// filter is refused.
fn sheet_matches(filter: &str, state: SheetState) -> Result<bool> {
    Ok(match filter {
        "" => state == SheetState::Pending,
        "open" => state != SheetState::Complete,
        "all" => true,
        "pending" | "pulled" | "complete" => state.as_str() == filter,
        other => {
            return Err(Error::invalid(format!(
                "status \"{other}\" is not one of {}",
                SHEET_FILTERS.join(", ")
            )));
        }
    })
}

/// Finds the comments in `ids`, narrowing the pull. A linked comment brings its
/// review. Every id must name a pending review, or a standalone comment.
fn select_ids(
    ids: &[String],
    reviews: Vec<Review>,
    comments: &[Comment],
) -> Result<(Vec<Review>, Vec<Comment>)> {
    // review or linked comment id → review id
    let mut owner: HashMap<&str, &str> = HashMap::new();
    for r in &reviews {
        owner.insert(r.id.as_str(), r.id.as_str());
        for id in &r.comment_ids {
            owner.insert(id.as_str(), r.id.as_str());
        }
    }
    let standalone: HashMap<&str, &Comment> = comments.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut missing = Vec::new();
    let mut wanted: HashSet<String> = HashSet::new();
    let mut picked = Vec::new();
    for id in ids {
        if let Some(rv) = owner.get(id.as_str()) {
            wanted.insert((*rv).to_string());
        } else if let Some(c) = standalone.get(id.as_str()) {
            picked.push((*c).clone());
        } else {
            missing.push(id.clone());
        }
    }
    if !missing.is_empty() {
        return Err(Error::not_found(format!(
            "not pending in this workspace: {}",
            missing.join(", ")
        )));
    }
    let reviews = reviews
        .into_iter()
        .filter(|r| wanted.contains(&r.id))
        .collect();
    Ok((reviews, picked))
}

/// Keeps the first `n` handoffs by creation time. Ties keep reviews ahead of
/// comments, each in creation order.
#[allow(clippy::large_enum_variant)] // short-lived; boxing would only add allocations
fn oldest(n: usize, reviews: Vec<Review>, comments: Vec<Comment>) -> (Vec<Review>, Vec<Comment>) {
    enum Unit {
        Review(Review),
        Comment(Comment),
    }
    let mut units: Vec<(String, Unit)> = reviews
        .into_iter()
        .map(|r| (r.created_at.clone(), Unit::Review(r)))
        .chain(
            comments
                .into_iter()
                .map(|c| (c.created_at.clone(), Unit::Comment(c))),
        )
        .collect();
    // A stable sort, so equal timestamps keep their order (reviews first).
    units.sort_by(|a, b| a.0.cmp(&b.0));
    let mut kept_reviews = Vec::new();
    let mut kept_comments = Vec::new();
    for (_, unit) in units.into_iter().take(n) {
        match unit {
            Unit::Review(r) => kept_reviews.push(r),
            Unit::Comment(c) => kept_comments.push(c),
        }
    }
    (kept_reviews, kept_comments)
}

/// The handoffs pending outside `lane`, so a pinned pull that took nothing
/// never reads as an empty queue.
fn elsewhere(reviews: &[Review], comments: &[Comment], lane: &str) -> Option<Elsewhere> {
    let grouped = linked_ids(reviews);
    let mut lanes = BTreeSet::new();
    let mut count: i64 = 0;
    let mut note = |l: Option<&str>| {
        if l == Some(lane) {
            return;
        }
        count += 1;
        lanes.insert(l.unwrap_or("(no lane)").to_string());
    };
    for r in reviews {
        note(r.lane.as_deref());
    }
    for c in comments {
        if !grouped.contains(&c.id) {
            note(c.lane.as_deref());
        }
    }
    if count == 0 {
        return None;
    }
    Some(Elsewhere {
        count,
        lanes: lanes.into_iter().collect(),
    })
}

/// Looks up one comment, refusing an unknown id.
pub(crate) fn find(tx: &mut dyn Tx, id: &str) -> Result<Comment> {
    tx.comment(id)?
        .ok_or_else(|| Error::not_found(format!("no review '{id}' in this workspace")))
}

/// Ids of every comment linked to one of `reviews`.
fn linked_ids(reviews: &[Review]) -> HashSet<String> {
    reviews
        .iter()
        .flat_map(|r| r.comment_ids.iter().cloned())
        .collect()
}

/// Splits file content into lines; a trailing newline is not a line.
pub(crate) fn split_source(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

fn unique(ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.iter()
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect()
}

/// Formats a stamp as second-resolution UTC with an explicit `+00:00` offset.
pub(crate) fn format_time(t: OffsetDateTime) -> String {
    let u = t.to_offset(UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
        u.year(),
        u8::from(u.month()),
        u.day(),
        u.hour(),
        u.minute(),
        u.second()
    )
}

pub(crate) fn is_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// `None` for an empty string, the Go `strPtr` rule.
pub(crate) fn str_opt(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Trims only newline characters, as `strings.Trim(x, "\n")` does.
pub(crate) fn trim_newlines(s: &str) -> &str {
    s.trim_matches('\n')
}

pub(crate) fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

pub(crate) fn to_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

pub(crate) fn to_index(n: i64) -> usize {
    usize::try_from(n).unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod testing {
    //! An in-memory [`Repository`] for the service tests. Each write runs on a
    //! copy of the workspace that replaces the stored one only when it succeeds.

    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use time::{Date, Month, OffsetDateTime};

    use crate::error::{Error, Result};
    use crate::repository::{Repository, Tx};
    use crate::types::{Comment, GateOverride, Review, ReviewStatus, Status};

    #[derive(Default, Clone)]
    struct WorkspaceData {
        overrides: Vec<GateOverride>,
        comments: Vec<Comment>,
        reviews: Vec<Review>,
        comment_seq: i64,
        review_seq: i64,
    }

    #[derive(Default)]
    pub struct MemRepo {
        data: Mutex<BTreeMap<String, WorkspaceData>>,
    }

    struct MemTx<'a> {
        data: &'a mut WorkspaceData,
    }

    impl Repository for MemRepo {
        fn update(
            &self,
            workspace: &str,
            f: &mut dyn FnMut(&mut dyn Tx) -> Result<()>,
        ) -> Result<()> {
            let mut copy = self
                .data
                .lock()
                .expect("mem repo lock")
                .get(workspace)
                .cloned()
                .unwrap_or_default();
            f(&mut MemTx { data: &mut copy })?;
            self.data
                .lock()
                .expect("mem repo lock")
                .insert(workspace.to_string(), copy);
            Ok(())
        }

        fn view(
            &self,
            workspace: &str,
            f: &mut dyn FnMut(&mut dyn Tx) -> Result<()>,
        ) -> Result<()> {
            let mut copy = self
                .data
                .lock()
                .expect("mem repo lock")
                .get(workspace)
                .cloned()
                .unwrap_or_default();
            f(&mut MemTx { data: &mut copy })
        }

        fn workspaces(&self) -> Result<Vec<String>> {
            Ok(self
                .data
                .lock()
                .expect("mem repo lock")
                .keys()
                .cloned()
                .collect())
        }

        fn location(&self) -> String {
            "memory".to_string()
        }
    }

    impl Tx for MemTx<'_> {
        fn next_comment_seq(&mut self) -> Result<i64> {
            self.data.comment_seq += 1;
            Ok(self.data.comment_seq)
        }

        fn next_review_seq(&mut self) -> Result<i64> {
            self.data.review_seq += 1;
            Ok(self.data.review_seq)
        }

        fn comments(&mut self, statuses: &[Status]) -> Result<Vec<Comment>> {
            Ok(self
                .data
                .comments
                .iter()
                .filter(|c| statuses.is_empty() || statuses.contains(&c.status))
                .cloned()
                .collect())
        }

        fn comment(&mut self, id: &str) -> Result<Option<Comment>> {
            Ok(self.data.comments.iter().find(|c| c.id == id).cloned())
        }

        fn insert_comment(&mut self, c: &Comment) -> Result<()> {
            self.data.comments.push(c.clone());
            Ok(())
        }

        fn update_comment(&mut self, c: &Comment) -> Result<()> {
            match self.data.comments.iter_mut().find(|x| x.id == c.id) {
                Some(slot) => {
                    *slot = c.clone();
                    Ok(())
                }
                None => Err(Error::internal(format!("no comment {}", c.id))),
            }
        }

        fn delete_comment(&mut self, id: &str) -> Result<()> {
            self.data.comments.retain(|c| c.id != id);
            Ok(())
        }

        fn reviews(&mut self, statuses: &[ReviewStatus]) -> Result<Vec<Review>> {
            Ok(self
                .data
                .reviews
                .iter()
                .filter(|r| statuses.is_empty() || statuses.contains(&r.status))
                .cloned()
                .collect())
        }

        fn review(&mut self, id: &str) -> Result<Option<Review>> {
            Ok(self.data.reviews.iter().find(|r| r.id == id).cloned())
        }

        fn insert_review(&mut self, r: &Review) -> Result<()> {
            for id in &r.comment_ids {
                if let Some(c) = self.data.comments.iter_mut().find(|c| &c.id == id) {
                    c.review_id = r.id.clone();
                }
            }
            self.data.reviews.push(r.clone());
            Ok(())
        }

        fn update_review(&mut self, r: &Review) -> Result<()> {
            match self.data.reviews.iter_mut().find(|x| x.id == r.id) {
                Some(slot) => {
                    slot.status = r.status;
                    slot.pulled_at = r.pulled_at.clone();
                    Ok(())
                }
                None => Err(Error::internal(format!("no review {}", r.id))),
            }
        }

        fn insert_override(&mut self, o: &GateOverride) -> Result<()> {
            self.data.overrides.push(o.clone());
            Ok(())
        }

        fn overrides(&mut self) -> Result<Vec<GateOverride>> {
            Ok(self.data.overrides.clone())
        }
    }

    /// Every stamp reads 2026-10-12T09:00:00+00:00.
    pub fn fixed_clock() -> Box<dyn Fn() -> OffsetDateTime + Send + Sync> {
        let t = Date::from_calendar_date(2026, Month::October, 12)
            .expect("valid date")
            .with_hms(9, 0, 0)
            .expect("valid time")
            .assume_utc();
        Box::new(move || t)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::testing::{MemRepo, fixed_clock};
    use super::*;
    use crate::error::ErrorKind;
    use crate::types::{Actor, Verdict, VerdictKind};

    pub(crate) struct Fixture {
        pub(crate) svc: Service,
        pub(crate) ws: String,
        pub(crate) root: PathBuf,
        _dir: tempfile::TempDir,
    }

    pub(crate) fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    pub(crate) fn setup() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("proj");
        fs::create_dir_all(&root).expect("mkdir");
        git(&root, &["init", "-q"]);
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        fs::write(root.join("app.py"), "one\ntwo\nthree\nfour\n").expect("write");
        let ws = rvw_git::workspace::resolve(&root)
            .expect("resolves")
            .to_string_lossy()
            .into_owned();
        Fixture {
            svc: Service::with_clock(Box::new(MemRepo::default()), fixed_clock()),
            ws,
            root,
            _dir: dir,
        }
    }

    impl Fixture {
        pub(crate) fn add_at(&self, line: i64, text: &str, lane_name: &str, who: &str) -> Comment {
            self.svc
                .add(AddInput {
                    points_at: None,
                    workspace: self.ws.clone(),
                    file: "app.py".into(),
                    start_line: line,
                    comment: text.into(),
                    lane: lane_name.into(),
                    author: who.into(),
                    ..Default::default()
                })
                .expect("add")
        }

        pub(crate) fn pending(&self, lane: &str) -> i64 {
            self.svc
                .count(QueryInput {
                    workspace: self.ws.clone(),
                    lane: lane.into(),
                    ..Default::default()
                })
                .expect("count")
                .count
        }

        pub(crate) fn pull(&self, lane: &str) -> PullOutput {
            self.svc
                .pull(PullInput {
                    workspace: self.ws.clone(),
                    lane: lane.into(),
                    ..Default::default()
                })
                .expect("pull")
        }
    }

    fn ids(cs: &[Comment]) -> Vec<String> {
        cs.iter().map(|c| c.id.clone()).collect()
    }

    fn strs(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn pull_hands_each_comment_over_once() {
        let f = setup();
        f.add_at(1, "first", "", "");
        f.add_at(2, "second", "", "");
        let out = f.pull("");
        assert_eq!(ids(&out.comments), strs(&["r1", "r2"]));
        assert!(out.drained);
        assert_eq!(out.comments[0].status, Status::Pulled);
        assert_eq!(
            out.comments[0].pulled_at.as_deref(),
            Some("2026-10-12T09:00:00+00:00")
        );
        let again = f.pull("");
        assert_eq!(again.count(), 0);
        assert_eq!(f.pending(""), 0);
    }

    #[test]
    fn a_peek_reads_without_dequeuing() {
        let f = setup();
        f.add_at(1, "first", "", "");
        let peek = f
            .svc
            .pull(PullInput {
                workspace: f.ws.clone(),
                peek: true,
                ..Default::default()
            })
            .expect("peek");
        assert!(!peek.drained);
        assert_eq!(peek.count(), 1);
        assert_eq!(f.pending(""), 1);
    }

    #[test]
    fn pinned_pull_is_strict_and_says_what_waits_elsewhere() {
        let f = setup();
        f.add_at(1, "unlaned", "", "");
        f.add_at(2, "payments", "payments", "");
        let out = f.pull("auth");
        assert_eq!(out.count(), 0);
        assert_eq!(
            out.elsewhere,
            Some(Elsewhere {
                count: 2,
                lanes: strs(&["(no lane)", "payments"]),
            })
        );
        assert_eq!(f.pending(""), 2);
    }

    #[test]
    fn a_pinned_list_never_sees_unlaned_comments() {
        let f = setup();
        f.add_at(1, "unlaned", "", "");
        f.add_at(2, "auth one", "auth", "");
        let out = f
            .svc
            .list(QueryInput {
                workspace: f.ws.clone(),
                lane: "auth".into(),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(ids(&out.comments), strs(&["r2"]));
    }

    #[test]
    fn submitted_review_moves_as_one_handoff() {
        let f = setup();
        f.add_at(1, "first finding", "", "alice");
        f.add_at(2, "second finding", "", "alice");
        f.add_at(3, "bob's finding", "", "bob");

        let r = f
            .svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "request-changes".into(),
                summary: "Fix both.".into(),
                author: "alice".into(),
                ..Default::default()
            })
            .expect("submit");
        assert_eq!(r.comment_ids, strs(&["r1", "r2"]));
        assert_eq!(f.pending(""), 2);

        let out = f
            .svc
            .pull(PullInput {
                workspace: f.ws.clone(),
                ids: strs(&["r2"]),
                ..Default::default()
            })
            .expect("pull");
        assert_eq!(out.reviews.len(), 1);
        assert!(out.comments.is_empty());
        assert_eq!(ids(&out.reviews[0].comments), strs(&["r1", "r2"]));

        let sheet = f
            .svc
            .sheet(GetInput {
                workspace: f.ws.clone(),
                id: "rv1".into(),
            })
            .expect("sheet");
        assert_eq!(sheet.state, SheetState::Pulled);
        for id in ["r1", "r2"] {
            f.svc
                .resolve(ResolveInput {
                    workspace: f.ws.clone(),
                    id: id.into(),
                    outcome: OUTCOME_DONE.into(),
                    ..Default::default()
                })
                .expect("resolve");
        }
        let sheet = f
            .svc
            .sheet(GetInput {
                workspace: f.ws.clone(),
                id: "rv1".into(),
            })
            .expect("sheet");
        assert_eq!(sheet.state, SheetState::Complete);
    }

    #[test]
    fn a_pending_review_takes_its_comments_out_of_the_default_pull_pool() {
        let f = setup();
        f.add_at(1, "a", "", "alice");
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "comment".into(),
                summary: "ok".into(),
                author: "alice".into(),
                ..Default::default()
            })
            .expect("submit");
        // The review is the one handoff; its comment is not also standalone.
        assert_eq!(f.pending(""), 1);
        let out = f.pull("");
        assert_eq!(out.reviews.len(), 1);
        assert!(out.comments.is_empty());
    }

    #[test]
    fn submit_with_no_comments_is_a_summary_only_review() {
        let f = setup();
        let r = f
            .svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "approve".into(),
                summary: "looks good".into(),
                no_comments: true,
                ..Default::default()
            })
            .expect("submit");
        assert!(r.comment_ids.is_empty());
        assert_eq!(f.pending(""), 1);
    }

    #[test]
    fn submit_refuses_bad_input_and_conflicts() {
        let f = setup();
        let base = || SubmitInput {
            workspace: f.ws.clone(),
            decision: "comment".into(),
            summary: "ok".into(),
            ..Default::default()
        };
        let err = f
            .svc
            .submit(SubmitInput {
                summary: "  ".into(),
                ..base()
            })
            .unwrap_err();
        assert_eq!(err.message, "a submitted review needs a summary");
        let err = f
            .svc
            .submit(SubmitInput {
                decision: "lgtm".into(),
                ..base()
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        let err = f
            .svc
            .submit(SubmitInput {
                no_comments: true,
                comment_ids: strs(&["r1"]),
                ..base()
            })
            .unwrap_err();
        assert_eq!(err.message, "pass comment ids or no_comments, not both");

        f.add_at(1, "x", "auth", "");
        let err = f
            .svc
            .submit(SubmitInput {
                comment_ids: strs(&["r1"]),
                ..base()
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(err.message, "r1 belongs to a different lane");

        let err = f
            .svc
            .submit(SubmitInput {
                comment_ids: strs(&["r1"]),
                lane: "auth".into(),
                author: "bob".into(),
                ..base()
            })
            .unwrap_err();
        assert_eq!(err.message, "r1 belongs to a different author");

        f.svc
            .submit(SubmitInput {
                comment_ids: strs(&["r1"]),
                lane: "auth".into(),
                ..base()
            })
            .expect("first submit");
        let err = f
            .svc
            .submit(SubmitInput {
                comment_ids: strs(&["r1"]),
                lane: "auth".into(),
                ..base()
            })
            .unwrap_err();
        assert_eq!(err.message, "r1 already belongs to rv1");
    }

    #[test]
    fn list_and_count_scope_by_status_and_lane() {
        let f = setup();
        f.add_at(1, "one", "", "");
        f.add_at(2, "two", "auth", "");
        f.add_at(3, "three", "auth", "");
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "comment".into(),
                summary: "s".into(),
                lane: "auth".into(),
                ..Default::default()
            })
            .expect("submit");
        let count = |status: &str| -> CountOutput {
            f.svc
                .count(QueryInput {
                    workspace: f.ws.clone(),
                    status: status.into(),
                    ..Default::default()
                })
                .expect("count")
        };
        // Pending counts handoffs: the auth review once, plus the unlaned comment.
        assert_eq!(count("").count, 2);
        // Other statuses count comments.
        assert_eq!(count("all").count, 3);
        assert_eq!(count("open").count, 3);
        let err = f
            .svc
            .list(QueryInput {
                workspace: f.ws.clone(),
                status: "maybe".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(
            err.message,
            "status \"maybe\" is not one of pending, pulled, done, rejected, open, all"
        );
    }

    #[test]
    fn pull_limit_takes_the_oldest_handoffs_first() {
        let f = setup();
        f.add_at(1, "one", "", "");
        f.add_at(2, "two", "", "");
        let out = f
            .svc
            .pull(PullInput {
                workspace: f.ws.clone(),
                limit: 1,
                ..Default::default()
            })
            .expect("pull");
        assert_eq!(ids(&out.comments), strs(&["r1"]));
        assert_eq!(f.pending(""), 1);
    }

    #[test]
    fn pull_by_id_refuses_unknown_and_already_pulled_ids() {
        let f = setup();
        f.add_at(1, "one", "", "");
        f.pull("");
        let err = f
            .svc
            .pull(PullInput {
                workspace: f.ws.clone(),
                ids: strs(&["r1", "r9"]),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(err.message, "not pending in this workspace: r1, r9");
    }

    #[test]
    fn oldest_ties_keep_reviews_ahead_of_comments() {
        let review = |id: &str, at: &str| Review {
            id: id.into(),
            status: ReviewStatus::Pending,
            workspace: "/w".into(),
            lane: None,
            author: None,
            decision: "comment".into(),
            summary: "s".into(),
            comment_ids: vec![],
            created_at: at.into(),
            pulled_at: None,
        };
        let comment = |id: &str, at: &str| Comment {
            points_at: None,
            reopens: None,
            verdict: None,
            id: id.into(),
            status: Status::Pending,
            workspace: "/w".into(),
            lane: None,
            author: None,
            file: "a".into(),
            path: "/w/a".into(),
            start_line: 1,
            end_line: 1,
            filetype: String::new(),
            code: String::new(),
            comment: String::new(),
            created_at: at.into(),
            pulled_at: None,
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            file_version: String::new(),
            resolved_file_version: String::new(),
            review_id: String::new(),
            edited_at: String::new(),
        };
        let t = "2026-10-12T09:00:00+00:00";
        // A tie keeps the review ahead of the comment.
        let (rs, cs) = oldest(1, vec![review("rv1", t)], vec![comment("r1", t)]);
        assert_eq!(
            rs.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            strs(&["rv1"])
        );
        assert!(cs.is_empty());
        let (rs, cs) = oldest(2, vec![review("rv1", t)], vec![comment("r1", t)]);
        assert_eq!(rs.len(), 1);
        assert_eq!(ids(&cs), strs(&["r1"]));
        // An older comment comes before a newer review.
        let (rs, cs) = oldest(
            1,
            vec![review("rv1", t)],
            vec![comment("r2", "2026-10-11T00:00:00+00:00")],
        );
        assert!(rs.is_empty());
        assert_eq!(ids(&cs), strs(&["r2"]));
    }

    #[test]
    fn resolve_keeps_both_versions_and_diffs_them() {
        let f = setup();
        git(&f.root, &["add", "app.py"]);
        let c = f.add_at(2, "change this", "", "");
        assert!(
            !c.file_version.is_empty(),
            "a tracked file keeps its reviewed version"
        );
        fs::write(f.root.join("app.py"), "one\nchanged-two\nthree\nfour\n").expect("write");
        f.pull("");

        let done = f
            .svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: OUTCOME_DONE.into(),
                note: "renamed".into(),
                author: "impl".into(),
            })
            .expect("resolve");
        assert_eq!(done.resolved_by.as_deref(), Some("impl"));
        assert_eq!(done.resolution_note.as_deref(), Some("renamed"));
        assert!(!done.resolved_file_version.is_empty());

        let ev = f
            .svc
            .evidence(GetInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
            })
            .expect("evidence");
        assert!(ev.diff_available);
        assert!(ev.diff.contains(&"-two".to_string()));
        assert!(ev.diff.contains(&"+changed-two".to_string()));

        let err = f
            .svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: OUTCOME_REJECTED.into(),
                note: "no".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(err.message, "r1 is already done — it cannot be re-decided");
    }

    #[test]
    fn an_identical_resolution_has_an_empty_available_diff() {
        let f = setup();
        git(&f.root, &["add", "app.py"]);
        f.add_at(2, "keep", "", "");
        f.pull("");
        f.svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: OUTCOME_DONE.into(),
                ..Default::default()
            })
            .expect("resolve");
        let ev = f
            .svc
            .evidence(GetInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
            })
            .expect("evidence");
        assert!(ev.diff_available);
        assert!(ev.diff.is_empty());
    }

    #[test]
    fn evidence_is_unavailable_without_both_snapshots() {
        let f = setup();
        // Not tracked by git, so no snapshot is kept.
        f.add_at(1, "untracked", "", "");
        f.pull("");
        f.svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: OUTCOME_DONE.into(),
                ..Default::default()
            })
            .expect("resolve");
        let ev = f
            .svc
            .evidence(GetInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
            })
            .expect("evidence");
        assert!(!ev.diff_available);
        assert!(ev.diff.is_empty());
    }

    #[test]
    fn resolve_refuses_the_wrong_order() {
        let f = setup();
        f.add_at(1, "pending", "", "");
        let err = f
            .svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: OUTCOME_DONE.into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(
            err.message,
            "r1 is pending — pull it before resolving it as done"
        );
        let err = f
            .svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: "maybe".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.message, "outcome \"maybe\" is not done or rejected");
    }

    #[test]
    fn a_pending_comment_can_be_rejected_which_retracts_it() {
        let f = setup();
        f.add_at(1, "pending", "", "");
        let err = f
            .svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: OUTCOME_REJECTED.into(),
                note: "  ".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.message, "rejecting a comment needs a reason");
        let c = f
            .svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: OUTCOME_REJECTED.into(),
                note: "\nnot needed\n".into(),
                ..Default::default()
            })
            .expect("reject");
        assert_eq!(c.status, Status::Rejected);
        assert_eq!(c.resolution_note.as_deref(), Some("not needed"));
        assert_eq!(f.pending(""), 0);
    }

    #[test]
    fn a_comment_in_an_unpulled_review_names_the_review() {
        let f = setup();
        f.add_at(1, "a", "", "");
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "comment".into(),
                summary: "s".into(),
                ..Default::default()
            })
            .expect("submit");
        let err = f
            .svc
            .resolve(ResolveInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                outcome: OUTCOME_REJECTED.into(),
                note: "no".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.message, "r1 belongs to rv1 — pull rv1 first");
    }

    #[test]
    fn edit_changes_only_pending_comments() {
        let f = setup();
        f.add_at(1, "old", "", "");
        let c = f
            .svc
            .edit(EditInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                comment: "\nnew text\n".into(),
            })
            .expect("edit");
        assert_eq!(c.comment, "new text");
        assert_eq!(c.edited_at, "2026-10-12T09:00:00+00:00");
        let err = f
            .svc
            .edit(EditInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                comment: "  ".into(),
            })
            .unwrap_err();
        assert_eq!(err.message, "refusing to replace a comment with empty text");
        f.pull("");
        let err = f
            .svc
            .edit(EditInput {
                workspace: f.ws.clone(),
                id: "r1".into(),
                comment: "later".into(),
            })
            .unwrap_err();
        assert_eq!(
            err.message,
            "r1 is pulled — only a pending comment can be edited"
        );
    }

    #[test]
    fn remove_deletes_only_draft_comments_and_never_reuses_ids() {
        let f = setup();
        f.add_at(1, "retract", "", "");
        f.add_at(2, "pulled", "", "");
        f.add_at(3, "in review", "", "");
        let rm = |id: &str| {
            f.svc.remove(RemoveInput {
                workspace: f.ws.clone(),
                id: id.into(),
            })
        };
        // A pending comment inside a submitted review stays.
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "comment".into(),
                summary: "s".into(),
                comment_ids: vec!["r3".into()],
                ..Default::default()
            })
            .expect("submit");
        let err = rm("r3").unwrap_err();
        assert_eq!(
            err.message,
            "r3 belongs to review rv1 — only a comment outside a submitted review can be removed"
        );
        let gone = rm("r1").expect("remove");
        assert_eq!((gone.id.as_str(), gone.comment.as_str()), ("r1", "retract"));
        assert_eq!(gone.status, Status::Pending);
        assert!(rm("r1").is_err(), "already gone");
        assert!(rm("r9").is_err());
        f.pull("");
        let err = rm("r2").unwrap_err();
        assert_eq!(
            err.message,
            "r2 is pulled — only a pending comment can be removed"
        );
        // The next id moves on even when the last one is the one removed.
        let next = f.add_at(1, "after", "", "");
        assert_eq!(next.id, "r4");
        rm("r4").expect("remove last");
        assert_eq!(f.add_at(1, "again", "", "").id, "r5");
    }

    #[test]
    fn remove_refuses_the_follow_up_of_a_reopen() {
        let f = setup();
        f.add_at(1, "decided", "", "");
        f.add_at(2, "reopened", "", "");
        f.svc
            .write(&f.ws, |tx| {
                let mut follow_up = find(tx, "r2")?;
                follow_up.reopens = Some("r1".into());
                tx.update_comment(&follow_up)
            })
            .expect("link");
        let err = f
            .svc
            .remove(RemoveInput {
                workspace: f.ws.clone(),
                id: "r2".into(),
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Conflict);
        assert_eq!(
            err.message,
            "r2 reopens r1 — withdraw it by rejecting it instead"
        );
        assert_eq!(f.pending(""), 2, "the follow-up stays in the queue");
    }

    #[test]
    fn sheets_filter_by_state() {
        let f = setup();
        f.add_at(1, "a", "", "");
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "comment".into(),
                summary: "s".into(),
                ..Default::default()
            })
            .expect("submit");
        let sheets = |status: &str| {
            f.svc
                .sheets(SheetsInput {
                    workspace: f.ws.clone(),
                    status: status.into(),
                    ..Default::default()
                })
                .expect("sheets")
        };
        assert_eq!(sheets("").count, 1);
        assert_eq!(sheets("pulled").count, 0);
        assert_eq!(sheets("all").count, 1);
        assert_eq!(sheets("open").count, 1);
        let err = f
            .svc
            .sheets(SheetsInput {
                workspace: f.ws.clone(),
                status: "done".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(
            err.message,
            "status \"done\" is not one of pending, pulled, complete, open, all"
        );
    }

    #[test]
    fn a_review_without_comments_is_complete_once_pulled() {
        let f = setup();
        f.svc
            .submit(SubmitInput {
                workspace: f.ws.clone(),
                decision: "approve".into(),
                summary: "s".into(),
                no_comments: true,
                ..Default::default()
            })
            .expect("submit");
        f.pull("");
        let sheet = f
            .svc
            .sheet(GetInput {
                workspace: f.ws.clone(),
                id: "rv1".into(),
            })
            .expect("sheet");
        assert_eq!(sheet.state, SheetState::Complete);
    }

    #[test]
    fn workspaces_lists_pending_handoffs_unless_all() {
        let f = setup();
        f.add_at(1, "a", "", "");
        let out = f
            .svc
            .workspaces(WorkspacesInput::default())
            .expect("workspaces");
        assert_eq!(out.workspaces.len(), 1);
        assert_eq!(out.workspaces[0].pending, 1);
        assert_eq!(out.workspaces[0].pulled, 0);
        f.pull("");
        let out = f
            .svc
            .workspaces(WorkspacesInput::default())
            .expect("workspaces");
        assert!(out.workspaces.is_empty());
        let out = f
            .svc
            .workspaces(WorkspacesInput { all: true })
            .expect("workspaces");
        assert_eq!(out.workspaces[0].pulled, 1);
    }

    #[test]
    fn errors_are_classified() {
        let f = setup();
        let err = f
            .svc
            .get(GetInput {
                workspace: f.ws.clone(),
                id: "r9".into(),
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(err.message, "no review 'r9' in this workspace");

        let err = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: f.ws.clone(),
                file: "app.py".into(),
                start_line: 99,
                comment: "x".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(
            err.message,
            format!("lines 99 is past the end of {}/app.py (4 lines)", f.ws)
        );

        let err = f
            .svc
            .list(QueryInput {
                workspace: String::new(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(err.message, "workspace '' is not an absolute path");

        let err = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: "proj".into(),
                file: "app.py".into(),
                start_line: 1,
                comment: "x".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.message, "workspace 'proj' is not an absolute path");
    }

    #[test]
    fn add_validates_before_touching_the_store() {
        let f = setup();
        let err = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: f.ws.clone(),
                file: "app.py".into(),
                start_line: 1,
                comment: "  \n".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.message, "a review comment needs text");
        let err = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: f.ws.clone(),
                file: "app.py".into(),
                start_line: 0,
                comment: "x".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.message, "--lines is 1-indexed; N must be >= 1");
    }

    #[test]
    fn add_snapshots_the_buffer_and_trims_the_note() {
        let f = setup();
        let c = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: f.ws.clone(),
                file: "app.py".into(),
                start_line: 4,
                end_line: 2,
                comment: "\nswapped range\n".into(),
                source: Some("a\nb\nc\nd\ne\n".into()),
                ..Default::default()
            })
            .expect("add");
        assert_eq!((c.start_line, c.end_line), (2, 4));
        assert_eq!(c.code, "b\nc\nd");
        assert_eq!(c.comment, "swapped range");
        assert_eq!(c.filetype, "python");
        assert_eq!(c.file, "app.py");
        assert!(c.file_version.is_empty());
    }

    #[test]
    fn add_refuses_a_missing_or_oversized_source() {
        let f = setup();
        let err = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: f.ws.clone(),
                file: "missing.py".into(),
                start_line: 1,
                comment: "x".into(),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(
            err.message
                .ends_with("does not exist — pass its content to snapshot unsaved changes")
        );

        let err = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: f.ws.clone(),
                file: "app.py".into(),
                start_line: 1,
                comment: "x".into(),
                source: Some("x".repeat(MAX_SOURCE + 1)),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.message, format!("{}/app.py is larger than 4 MiB", f.ws));
    }

    #[test]
    fn a_comment_outside_the_workspace_is_refused() {
        let f = setup();
        let err = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: f.ws.clone(),
                file: "/etc/hostname".into(),
                start_line: 1,
                comment: "x".into(),
                source: Some("x\n".into()),
                ..Default::default()
            })
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert!(
            err.message
                .ends_with(&format!("is outside the workspace {}", f.ws))
        );
    }

    fn at(file: &str, start_line: i64, end_line: i64) -> Location {
        Location {
            file: file.into(),
            start_line,
            end_line,
        }
    }

    fn add_pointing(f: &Fixture, points_at: Option<Location>) -> Result<Comment> {
        f.svc.add(AddInput {
            points_at,
            workspace: f.ws.clone(),
            file: "app.py".into(),
            start_line: 1,
            comment: "x".into(),
            ..Default::default()
        })
    }

    #[test]
    fn points_at_is_stored_relative_and_null_when_absent() {
        let f = setup();
        fs::write(f.root.join("lib.py"), "a\nb\nc\nd\n").expect("write");
        let c = add_pointing(&f, Some(at("lib.py", 2, 3))).expect("add");
        assert_eq!(c.points_at, Some(at("lib.py", 2, 3)));
        // An absolute path inside the workspace is stored relative too.
        let abs = format!("{}/lib.py", f.ws);
        let c = add_pointing(&f, Some(at(&abs, 4, 4))).expect("add absolute");
        assert_eq!(c.points_at, Some(at("lib.py", 4, 4)));
        assert_eq!(add_pointing(&f, None).expect("add").points_at, None);
    }

    #[test]
    fn points_at_swaps_a_reversed_range_and_clamps_the_end() {
        let f = setup();
        fs::write(f.root.join("lib.py"), "a\nb\nc\nd\n").expect("write");
        let stored = |p| add_pointing(&f, p).expect("add").points_at;
        assert_eq!(stored(Some(at("lib.py", 3, 2))), Some(at("lib.py", 2, 3)));
        assert_eq!(stored(Some(at("lib.py", 3, 99))), Some(at("lib.py", 3, 4)));
    }

    #[test]
    fn points_at_refusals_leave_the_queue_empty() {
        let f = setup();
        fs::write(f.root.join("lib.py"), "a\nb\nc\nd\n").expect("write");
        fs::create_dir_all(f.root.join("sub")).expect("mkdir");

        let err = add_pointing(&f, Some(at("/etc/hostname", 1, 1))).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(
            err.message,
            format!("points-at /etc/hostname is outside the workspace {}", f.ws)
        );

        let err = add_pointing(&f, Some(at("missing.py", 1, 1))).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(
            err.message,
            format!("points-at {}/missing.py does not exist", f.ws)
        );

        let err = add_pointing(&f, Some(at("sub", 1, 1))).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(
            err.message,
            format!("points-at {}/sub is not a regular file", f.ws)
        );

        let err = add_pointing(&f, Some(at("lib.py", 9, 9))).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(
            err.message,
            format!(
                "points-at lines 9 is past the end of {}/lib.py (4 lines)",
                f.ws
            )
        );

        for (start, end) in [(0, 2), (1, 0)] {
            let err = add_pointing(&f, Some(at("lib.py", start, end))).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid);
            assert_eq!(err.message, "points-at lines are 1-indexed; N must be >= 1");
        }

        assert_eq!(f.pending(""), 0, "a refused comment is never enqueued");
    }

    #[test]
    fn a_file_comment_uses_the_extension_for_its_filetype() {
        let f = setup();
        let c = f
            .svc
            .add(AddInput {
                points_at: None,
                workspace: f.ws.clone(),
                file: "notes.MD".into(),
                start_line: 1,
                comment: "x".into(),
                source: Some("# hi\n".into()),
                ..Default::default()
            })
            .expect("add");
        assert_eq!(c.filetype, "markdown");
    }

    #[test]
    fn comment_json_has_the_nullable_contract() {
        let f = setup();
        let c = f.add_at(1, "a <b> & c", "", "");
        let fields = serde_json::to_value(&c).expect("json");
        for k in [
            "lane",
            "author",
            "pulled_at",
            "resolved_at",
            "resolved_by",
            "resolution_note",
            "points_at",
            "reopens",
            "verdict",
        ] {
            assert!(
                fields.get(k).is_some_and(serde_json::Value::is_null),
                "{k} should be null"
            );
        }
        for k in [
            "file_version",
            "resolved_file_version",
            "review_id",
            "edited_at",
        ] {
            assert!(fields.get(k).is_none(), "{k} should be absent");
        }
        let list = f
            .svc
            .list(QueryInput {
                workspace: f.ws.clone(),
                status: "done".into(),
                ..Default::default()
            })
            .expect("list");
        assert_eq!(
            serde_json::to_value(&list.comments).expect("json"),
            serde_json::json!([])
        );
    }

    #[test]
    fn awaits_verdict_and_is_open_follow_the_decision() {
        let f = setup();
        let mut c = f.add_at(1, "x", "", "alice");
        assert!(c.is_open());
        assert!(!c.awaits_verdict(), "a draft awaits nothing");
        c.status = Status::Done;
        assert!(!c.is_open());
        assert!(!c.awaits_verdict(), "never handed over: a withdrawal");
        c.pulled_at = Some("t".into());
        c.resolved_by = Some("claude".into());
        assert!(c.awaits_verdict());
        c.resolved_by = Some("alice".into());
        assert!(!c.awaits_verdict(), "its own author decided it");
        c.resolved_by = Some("claude".into());
        c.verdict = Some(Verdict {
            kind: VerdictKind::Accepted,
            by: "alice".into(),
            actor: Actor::Human,
            at: "t".into(),
            note: None,
            followup: None,
        });
        assert!(!c.awaits_verdict(), "judged");
    }

    #[test]
    fn stamps_use_utc_with_an_explicit_offset() {
        assert_eq!(
            format_time(OffsetDateTime::UNIX_EPOCH),
            "1970-01-01T00:00:00+00:00"
        );
        let t = OffsetDateTime::from_unix_timestamp(1_800_000_000).expect("in range")
            + time::Duration::milliseconds(999);
        assert_eq!(format_time(t), "2027-01-15T08:00:00+00:00");
    }

    #[test]
    fn split_source_drops_only_the_final_empty_line() {
        assert_eq!(split_source("a\nb\n"), vec!["a", "b"]);
        assert_eq!(split_source("a\n\n"), vec!["a", ""]);
        assert!(split_source("").is_empty());
        assert_eq!(split_source("a"), vec!["a"]);
    }

    #[test]
    fn line_range_errors_match_the_cli_wording() {
        let err = LineRange {
            start_line: 0,
            end_line: 0,
        }
        .normalized()
        .unwrap_err();
        assert_eq!(err.message, "--lines is 1-indexed; N must be >= 1");
    }
}
