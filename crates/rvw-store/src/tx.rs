//! The per-workspace transaction: the SQL behind [`rvw_core::Tx`]. Port of the
//! `tx` type in internal/store.

use rusqlite::types::{Type, Value};
use rusqlite::{OptionalExtension, Row, Transaction, params, params_from_iter};
use rvw_core::types::{
    Actor, Comment, GateOverride, Location, Review, ReviewStatus, Session, Status, Verdict,
    VerdictKind, comment_id, parse_seq, review_id,
};
use rvw_core::{Error, Result, Tx};

use crate::{internal, workspace_id};

const SESSION_COLUMNS: &str = "session_id, tool, model, summary, reported_at";

// The v2 columns come last, after l.review_seq, so the positional indexes that
// scan_comment reads for the v1 columns do not move.
const COMMENT_COLUMNS: &str = "c.seq, c.status, c.lane, c.author, c.file, c.path, c.start_line, \
    c.end_line, c.filetype, c.code, c.comment, c.created_at, c.pulled_at, c.resolved_at, \
    c.resolved_by, c.resolution_note, c.file_version, c.resolved_file_version, c.edited_at, \
    l.review_seq, c.reopens_seq, c.points_file, c.points_start, c.points_end, c.verdict, \
    c.verdict_by, c.verdict_actor, c.verdict_at, c.verdict_note, c.verdict_followup_seq";

const COMMENT_FROM: &str = " FROM comments c \
    LEFT JOIN review_comments l ON l.workspace_id = c.workspace_id AND l.comment_seq = c.seq \
    WHERE c.workspace_id = ?";

const REVIEW_COLUMNS: &str = "seq, status, lane, author, decision, summary, created_at, pulled_at";

const INSERT_COMMENT: &str = "INSERT INTO comments (workspace_id, seq, status, lane, author, file, path, \
    start_line, end_line, filetype, code, comment, created_at, pulled_at, resolved_at, \
    resolved_by, resolution_note, file_version, resolved_file_version, edited_at, \
    reopens_seq, points_file, points_start, points_end, verdict, verdict_by, verdict_actor, \
    verdict_at, verdict_note, verdict_followup_seq) \
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

// "points at" and "reopens" are fixed when the comment is written, so the
// update leaves them alone.
const UPDATE_COMMENT: &str = "UPDATE comments SET status = ?, comment = ?, pulled_at = ?, resolved_at = ?, \
    resolved_by = ?, resolution_note = ?, resolved_file_version = ?, edited_at = ?, \
    verdict = ?, verdict_by = ?, verdict_actor = ?, verdict_at = ?, verdict_note = ?, \
    verdict_followup_seq = ? \
    WHERE workspace_id = ? AND seq = ?";

/// One write or read transaction on a workspace. `ws_id` is 0 for a workspace
/// that does not exist yet, which no row carries.
pub(crate) struct SqlTx<'conn> {
    pub(crate) tx: Transaction<'conn>,
    pub(crate) workspace: String,
    pub(crate) ws_id: i64,
}

impl SqlTx<'_> {
    fn next_seq(&self, column: &str) -> Result<i64> {
        self.tx
            .query_row(
                &format!(
                    "UPDATE workspaces SET {column} = {column} + 1 WHERE id = ? RETURNING {column} - 1"
                ),
                [self.ws_id],
                |row| row.get(0),
            )
            .map_err(internal)
    }

    /// The comment ids a review links to, in position order.
    fn links(&self, review_id: &str) -> Result<Vec<String>> {
        let seq = parse_seq(review_id, "rv").unwrap_or_default();
        let mut stmt = self
            .tx
            .prepare(
                "SELECT comment_seq FROM review_comments \
                 WHERE workspace_id = ? AND review_seq = ? ORDER BY position",
            )
            .map_err(internal)?;
        let seqs = stmt
            .query_map(params![self.ws_id, seq], |row| row.get::<_, i64>(0))
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<i64>>>()
            .map_err(internal)?;
        Ok(seqs.into_iter().map(comment_id).collect())
    }
}

impl Tx for SqlTx<'_> {
    fn next_comment_seq(&mut self) -> Result<i64> {
        self.next_seq("next_comment_seq")
    }

    fn next_review_seq(&mut self) -> Result<i64> {
        self.next_seq("next_review_seq")
    }

    fn comments(&mut self, statuses: &[Status]) -> Result<Vec<Comment>> {
        let mut sql = format!("SELECT {COMMENT_COLUMNS}{COMMENT_FROM}");
        let mut args = vec![Value::Integer(self.ws_id)];
        if !statuses.is_empty() {
            sql = format!("{sql} AND c.status IN ({})", placeholders(statuses.len()));
            args.extend(statuses.iter().map(|s| Value::Text(s.as_str().to_owned())));
        }
        sql.push_str(" ORDER BY c.seq");
        let mut stmt = self.tx.prepare(&sql).map_err(internal)?;
        let comments = stmt
            .query_map(params_from_iter(args), |row| {
                scan_comment(row, &self.workspace)
            })
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<Comment>>>()
            .map_err(internal)?;
        Ok(comments)
    }

    fn comment(&mut self, id: &str) -> Result<Option<Comment>> {
        let Some(seq) = parse_seq(id, "r") else {
            return Ok(None);
        };
        let sql = format!("SELECT {COMMENT_COLUMNS}{COMMENT_FROM} AND c.seq = ?");
        self.tx
            .query_row(&sql, params![self.ws_id, seq], |row| {
                scan_comment(row, &self.workspace)
            })
            .optional()
            .map_err(internal)
    }

    fn insert_comment(&mut self, c: &Comment) -> Result<()> {
        let seq = comment_seq(&c.id)?;
        let x = Extras::of(c)?;
        self.tx
            .execute(
                INSERT_COMMENT,
                params![
                    self.ws_id,
                    seq,
                    c.status.as_str(),
                    c.lane,
                    c.author,
                    c.file,
                    c.path,
                    c.start_line,
                    c.end_line,
                    c.filetype,
                    c.code,
                    c.comment,
                    c.created_at,
                    c.pulled_at,
                    c.resolved_at,
                    c.resolved_by,
                    c.resolution_note,
                    non_empty(&c.file_version),
                    non_empty(&c.resolved_file_version),
                    non_empty(&c.edited_at),
                    x.reopens_seq,
                    x.points_file,
                    x.points_start,
                    x.points_end,
                    x.verdict,
                    x.verdict_by,
                    x.verdict_actor,
                    x.verdict_at,
                    x.verdict_note,
                    x.verdict_followup_seq,
                ],
            )
            .map(drop)
            .map_err(internal)
    }

    fn update_comment(&mut self, c: &Comment) -> Result<()> {
        let seq = comment_seq(&c.id)?;
        let x = Extras::of(c)?;
        self.tx
            .execute(
                UPDATE_COMMENT,
                params![
                    c.status.as_str(),
                    c.comment,
                    c.pulled_at,
                    c.resolved_at,
                    c.resolved_by,
                    c.resolution_note,
                    non_empty(&c.resolved_file_version),
                    non_empty(&c.edited_at),
                    x.verdict,
                    x.verdict_by,
                    x.verdict_actor,
                    x.verdict_at,
                    x.verdict_note,
                    x.verdict_followup_seq,
                    self.ws_id,
                    seq,
                ],
            )
            .map(drop)
            .map_err(internal)
    }

    fn delete_comment(&mut self, id: &str) -> Result<()> {
        let seq = comment_seq(id)?;
        self.tx
            .execute(
                "DELETE FROM comments WHERE workspace_id = ? AND seq = ?",
                params![self.ws_id, seq],
            )
            .map(drop)
            .map_err(internal)
    }

    fn reviews(&mut self, statuses: &[ReviewStatus]) -> Result<Vec<Review>> {
        let mut sql = format!("SELECT {REVIEW_COLUMNS} FROM reviews WHERE workspace_id = ?");
        let mut args = vec![Value::Integer(self.ws_id)];
        if !statuses.is_empty() {
            sql = format!("{sql} AND status IN ({})", placeholders(statuses.len()));
            args.extend(statuses.iter().map(|s| Value::Text(s.as_str().to_owned())));
        }
        sql.push_str(" ORDER BY seq");
        let mut reviews = {
            let mut stmt = self.tx.prepare(&sql).map_err(internal)?;

            stmt.query_map(params_from_iter(args), |row| {
                scan_review(row, &self.workspace)
            })
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<Review>>>()
            .map_err(internal)?
        };
        for r in &mut reviews {
            r.comment_ids = self.links(&r.id)?;
        }
        Ok(reviews)
    }

    fn review(&mut self, id: &str) -> Result<Option<Review>> {
        let Some(seq) = parse_seq(id, "rv") else {
            return Ok(None);
        };
        let sql =
            format!("SELECT {REVIEW_COLUMNS} FROM reviews WHERE workspace_id = ? AND seq = ?");
        let found = self
            .tx
            .query_row(&sql, params![self.ws_id, seq], |row| {
                scan_review(row, &self.workspace)
            })
            .optional()
            .map_err(internal)?;
        match found {
            Some(mut r) => {
                r.comment_ids = self.links(&r.id)?;
                Ok(Some(r))
            }
            None => Ok(None),
        }
    }

    fn insert_review(&mut self, r: &Review) -> Result<()> {
        let seq = parse_seq(&r.id, "rv")
            .ok_or_else(|| Error::internal(format!("malformed review id {:?}", r.id)))?;
        self.tx
            .execute(
                "INSERT INTO reviews (workspace_id, seq, status, lane, author, decision, summary, \
                 created_at, pulled_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    self.ws_id,
                    seq,
                    r.status.as_str(),
                    r.lane,
                    r.author,
                    r.decision,
                    r.summary,
                    r.created_at,
                    r.pulled_at,
                ],
            )
            .map_err(internal)?;
        for (pos, id) in r.comment_ids.iter().enumerate() {
            let cseq = comment_seq(id)?;
            let pos = i64::try_from(pos).map_err(internal)?;
            self.tx
                .execute(
                    "INSERT INTO review_comments (workspace_id, review_seq, position, comment_seq) \
                     VALUES (?, ?, ?, ?)",
                    params![self.ws_id, seq, pos, cseq],
                )
                .map_err(internal)?;
        }
        Ok(())
    }

    fn update_review(&mut self, r: &Review) -> Result<()> {
        let seq = parse_seq(&r.id, "rv")
            .ok_or_else(|| Error::internal(format!("malformed review id {:?}", r.id)))?;
        self.tx
            .execute(
                "UPDATE reviews SET status = ?, pulled_at = ? WHERE workspace_id = ? AND seq = ?",
                params![r.status.as_str(), r.pulled_at, self.ws_id, seq],
            )
            .map(drop)
            .map_err(internal)
    }

    fn insert_override(&mut self, o: &GateOverride) -> Result<()> {
        let seq: i64 = self
            .tx
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM gate_overrides WHERE workspace_id = ?",
                [self.ws_id],
                |row| row.get(0),
            )
            .map_err(internal)?;
        self.tx
            .execute(
                "INSERT INTO gate_overrides (workspace_id, seq, at, lane, action, reason, author, \
                 actor, open, unjudged) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    self.ws_id,
                    seq,
                    o.at,
                    o.lane,
                    o.action,
                    o.reason,
                    o.by,
                    o.actor.as_str(),
                    o.open.join(" "),
                    o.unjudged.join(" "),
                ],
            )
            .map(drop)
            .map_err(internal)
    }

    fn overrides(&mut self) -> Result<Vec<GateOverride>> {
        let workspace = self.workspace.clone();
        let mut stmt = self
            .tx
            .prepare(
                "SELECT at, lane, action, reason, author, actor, open, unjudged \
                 FROM gate_overrides WHERE workspace_id = ? ORDER BY seq",
            )
            .map_err(internal)?;
        let overrides = stmt
            .query_map([self.ws_id], |row| {
                let raw_actor: String = row.get(5)?;
                let actor = Actor::parse(&raw_actor).ok_or_else(|| {
                    bad_column(5, format!("unknown override actor {raw_actor:?}"))
                })?;
                Ok(GateOverride {
                    workspace: workspace.clone(),
                    lane: row.get(1)?,
                    action: row.get(2)?,
                    reason: row.get(3)?,
                    by: row.get(4)?,
                    actor,
                    at: row.get(0)?,
                    open: split_ids(&row.get::<_, String>(6)?),
                    unjudged: split_ids(&row.get::<_, String>(7)?),
                })
            })
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<GateOverride>>>()
            .map_err(internal)?;
        Ok(overrides)
    }

    fn session(&self, workspace: &str, session_id: &str) -> Result<Option<Session>> {
        let ws_id = workspace_id(&self.tx, workspace, false)?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE workspace_id = ? AND session_id = ?"
        );
        let Some(mut session) = self
            .tx
            .query_row(&sql, params![ws_id, session_id], scan_session)
            .optional()
            .map_err(internal)?
        else {
            return Ok(None);
        };
        session.commits = session_commits(&self.tx, ws_id, session_id)?;
        Ok(Some(session))
    }

    fn insert_session(&mut self, workspace: &str, s: &Session) -> Result<()> {
        let ws_id = workspace_id(&self.tx, workspace, false)?;
        self.tx
            .execute(
                "INSERT INTO sessions (workspace_id, session_id, tool, model, summary, reported_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![ws_id, s.session_id, s.tool, s.model, s.summary, s.reported_at],
            )
            .map_err(internal)?;
        add_commits(&self.tx, ws_id, &s.session_id, &s.commits)
    }

    fn add_session_commits(
        &mut self,
        workspace: &str,
        session_id: &str,
        commits: &[String],
    ) -> Result<()> {
        let ws_id = workspace_id(&self.tx, workspace, false)?;
        let known: bool = self
            .tx
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sessions WHERE workspace_id = ? AND session_id = ?)",
                params![ws_id, session_id],
                |row| row.get(0),
            )
            .map_err(internal)?;
        if !known {
            return Err(Error::internal(format!("no session {session_id}")));
        }
        add_commits(&self.tx, ws_id, session_id, commits)
    }

    fn sessions(&self, workspace: &str) -> Result<Vec<Session>> {
        let ws_id = workspace_id(&self.tx, workspace, false)?;
        let sql = format!(
            "SELECT {SESSION_COLUMNS} FROM sessions WHERE workspace_id = ? \
             ORDER BY reported_at DESC, session_id"
        );
        let mut stmt = self.tx.prepare(&sql).map_err(internal)?;
        let mut sessions = stmt
            .query_map(params![ws_id], scan_session)
            .map_err(internal)?
            .collect::<rusqlite::Result<Vec<Session>>>()
            .map_err(internal)?;
        for s in &mut sessions {
            s.commits = session_commits(&self.tx, ws_id, &s.session_id)?;
        }
        Ok(sessions)
    }
}

/// Appends `commits` to a session after the ones it has. A commit already
/// stored is skipped and takes no position, so report order holds.
fn add_commits(
    tx: &Transaction<'_>,
    ws_id: i64,
    session_id: &str,
    commits: &[String],
) -> Result<()> {
    let mut next: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM session_commits \
             WHERE workspace_id = ? AND session_id = ?",
            params![ws_id, session_id],
            |row| row.get(0),
        )
        .map_err(internal)?;
    for commit in commits {
        let added = tx
            .execute(
                "INSERT OR IGNORE INTO session_commits (workspace_id, session_id, commit_id, position) \
                 VALUES (?, ?, ?, ?)",
                params![ws_id, session_id, commit, next],
            )
            .map_err(internal)?;
        if added > 0 {
            next += 1;
        }
    }
    Ok(())
}

/// The commit ids a session reported, in report order.
fn session_commits(tx: &Transaction<'_>, ws_id: i64, session_id: &str) -> Result<Vec<String>> {
    let mut stmt = tx
        .prepare(
            "SELECT commit_id FROM session_commits \
             WHERE workspace_id = ? AND session_id = ? ORDER BY position",
        )
        .map_err(internal)?;
    let commits = stmt
        .query_map(params![ws_id, session_id], |row| row.get::<_, String>(0))
        .map_err(internal)?
        .collect::<rusqlite::Result<Vec<String>>>()
        .map_err(internal)?;
    Ok(commits)
}

/// A session's own columns; its commits are filled in by the caller.
fn scan_session(row: &Row<'_>) -> rusqlite::Result<Session> {
    Ok(Session {
        session_id: row.get(0)?,
        tool: row.get(1)?,
        model: row.get(2)?,
        summary: row.get(3)?,
        commits: Vec::new(),
        reported_at: row.get(4)?,
    })
}

/// The sequence number of a comment id, or an internal error for a malformed one.
fn comment_seq(id: &str) -> Result<i64> {
    parse_seq(id, "r").ok_or_else(|| Error::internal(format!("malformed comment id {id:?}")))
}

/// The v2 columns of a comment as SQL values. Each verdict column is set only
/// when there is a verdict, and the "points at" columns only with a range.
struct Extras<'a> {
    reopens_seq: Option<i64>,
    points_file: Option<&'a str>,
    points_start: Option<i64>,
    points_end: Option<i64>,
    verdict: Option<&'static str>,
    verdict_by: Option<&'a str>,
    verdict_actor: Option<&'static str>,
    verdict_at: Option<&'a str>,
    verdict_note: Option<&'a str>,
    verdict_followup_seq: Option<i64>,
}

impl<'a> Extras<'a> {
    fn of(c: &'a Comment) -> Result<Self> {
        let (points_file, points_start, points_end) = match &c.points_at {
            Some(at) => (
                Some(at.file.as_str()),
                Some(at.start_line),
                Some(at.end_line),
            ),
            None => (None, None, None),
        };
        let verdict = c.verdict.as_ref();
        let followup = verdict.and_then(|v| v.followup.as_deref());
        Ok(Extras {
            reopens_seq: c.reopens.as_deref().map(comment_seq).transpose()?,
            points_file,
            points_start,
            points_end,
            verdict: verdict.map(|v| v.kind.as_str()),
            verdict_by: verdict.map(|v| v.by.as_str()),
            verdict_actor: verdict.map(|v| v.actor.as_str()),
            verdict_at: verdict.map(|v| v.at.as_str()),
            verdict_note: verdict.and_then(|v| v.note.as_deref()),
            verdict_followup_seq: followup.map(comment_seq).transpose()?,
        })
    }
}

fn scan_comment(row: &Row<'_>, workspace: &str) -> rusqlite::Result<Comment> {
    let seq: i64 = row.get(0)?;
    let raw_status: String = row.get(1)?;
    let status = Status::parse(&raw_status)
        .ok_or_else(|| bad_column(1, format!("unknown status {raw_status:?}")))?;
    let review_seq: Option<i64> = row.get(19)?;
    let reopens_seq: Option<i64> = row.get(20)?;
    let points_file: Option<String> = row.get(21)?;
    let points_at = match points_file {
        Some(file) => Some(Location {
            file,
            start_line: row.get(22)?,
            end_line: row.get(23)?,
        }),
        None => None,
    };
    let verdict = scan_verdict(row)?;
    Ok(Comment {
        points_at,
        reopens: reopens_seq.map(comment_id),
        verdict,
        id: comment_id(seq),
        status,
        workspace: workspace.to_owned(),
        lane: row.get(2)?,
        author: row.get(3)?,
        file: row.get(4)?,
        path: row.get(5)?,
        start_line: row.get(6)?,
        end_line: row.get(7)?,
        filetype: row.get(8)?,
        code: row.get(9)?,
        comment: row.get(10)?,
        created_at: row.get(11)?,
        pulled_at: row.get(12)?,
        resolved_at: row.get(13)?,
        resolved_by: row.get(14)?,
        resolution_note: row.get(15)?,
        file_version: text_or_empty(row, 16)?,
        resolved_file_version: text_or_empty(row, 17)?,
        review_id: review_seq.map(review_id).unwrap_or_default(),
        edited_at: text_or_empty(row, 18)?,
    })
}

/// The verdict columns (24 to 29) of a comment row: `None` unless the verdict
/// column is set, and then all of its fields must be valid.
fn scan_verdict(row: &Row<'_>) -> rusqlite::Result<Option<Verdict>> {
    let Some(raw_kind) = row.get::<_, Option<String>>(24)? else {
        return Ok(None);
    };
    let kind = VerdictKind::parse(&raw_kind)
        .ok_or_else(|| bad_column(24, format!("unknown verdict {raw_kind:?}")))?;
    let raw_actor = text_or_empty(row, 26)?;
    let actor = Actor::parse(&raw_actor)
        .ok_or_else(|| bad_column(26, format!("unknown verdict actor {raw_actor:?}")))?;
    let followup_seq: Option<i64> = row.get(29)?;
    Ok(Some(Verdict {
        kind,
        by: text_or_empty(row, 25)?,
        actor,
        at: text_or_empty(row, 27)?,
        note: row.get(28)?,
        followup: followup_seq.map(comment_id),
    }))
}

/// A space-separated id list as stored in `gate_overrides`; "" is the empty list.
fn split_ids(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_owned).collect()
}

fn scan_review(row: &Row<'_>, workspace: &str) -> rusqlite::Result<Review> {
    let seq: i64 = row.get(0)?;
    let raw_status: String = row.get(1)?;
    let status = match raw_status.as_str() {
        "pending" => ReviewStatus::Pending,
        "pulled" => ReviewStatus::Pulled,
        other => return Err(bad_column(1, format!("unknown review status {other:?}"))),
    };
    Ok(Review {
        id: review_id(seq),
        status,
        workspace: workspace.to_owned(),
        lane: row.get(2)?,
        author: row.get(3)?,
        decision: row.get(4)?,
        summary: row.get(5)?,
        comment_ids: Vec::new(),
        created_at: row.get(6)?,
        pulled_at: row.get(7)?,
    })
}

/// A nullable text column read as "" when NULL, for the fields the domain keeps as strings.
fn text_or_empty(row: &Row<'_>, idx: usize) -> rusqlite::Result<String> {
    Ok(row.get::<_, Option<String>>(idx)?.unwrap_or_default())
}

fn bad_column(idx: usize, msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, msg.into())
}

/// Empty strings are stored as NULL for the optional version and edit columns.
fn non_empty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}
