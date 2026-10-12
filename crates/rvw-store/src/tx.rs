//! The per-workspace transaction: the SQL behind [`rvw_core::Tx`]. Port of the
//! `tx` type in internal/store.

use rusqlite::types::{Type, Value};
use rusqlite::{params, params_from_iter, OptionalExtension, Row, Transaction};
use rvw_core::types::{comment_id, parse_seq, review_id, Comment, Review, ReviewStatus, Status};
use rvw_core::{Error, Result, Tx};

use crate::internal;

const COMMENT_COLUMNS: &str = "c.seq, c.status, c.lane, c.author, c.file, c.path, c.start_line, \
    c.end_line, c.filetype, c.code, c.comment, c.created_at, c.pulled_at, c.resolved_at, \
    c.resolved_by, c.resolution_note, c.file_version, c.resolved_file_version, c.edited_at, \
    l.review_seq";

const COMMENT_FROM: &str = " FROM comments c \
    LEFT JOIN review_comments l ON l.workspace_id = c.workspace_id AND l.comment_seq = c.seq \
    WHERE c.workspace_id = ?";

const REVIEW_COLUMNS: &str = "seq, status, lane, author, decision, summary, created_at, pulled_at";

const INSERT_COMMENT: &str =
    "INSERT INTO comments (workspace_id, seq, status, lane, author, file, path, \
    start_line, end_line, filetype, code, comment, created_at, pulled_at, resolved_at, \
    resolved_by, resolution_note, file_version, resolved_file_version, edited_at) \
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

const UPDATE_COMMENT: &str =
    "UPDATE comments SET status = ?, comment = ?, pulled_at = ?, resolved_at = ?, \
    resolved_by = ?, resolution_note = ?, resolved_file_version = ?, edited_at = ? \
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
                ],
            )
            .map(drop)
            .map_err(internal)
    }

    fn update_comment(&mut self, c: &Comment) -> Result<()> {
        let seq = comment_seq(&c.id)?;
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
                    self.ws_id,
                    seq,
                ],
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
            let rows = stmt
                .query_map(params_from_iter(args), |row| {
                    scan_review(row, &self.workspace)
                })
                .map_err(internal)?
                .collect::<rusqlite::Result<Vec<Review>>>()
                .map_err(internal)?;
            rows
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
}

/// The sequence number of a comment id, or an internal error for a malformed one.
fn comment_seq(id: &str) -> Result<i64> {
    parse_seq(id, "r").ok_or_else(|| Error::internal(format!("malformed comment id {id:?}")))
}

fn scan_comment(row: &Row<'_>, workspace: &str) -> rusqlite::Result<Comment> {
    let seq: i64 = row.get(0)?;
    let raw_status: String = row.get(1)?;
    let status = Status::parse(&raw_status)
        .ok_or_else(|| bad_column(1, format!("unknown status {raw_status:?}")))?;
    let review_seq: Option<i64> = row.get(19)?;
    Ok(Comment {
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
