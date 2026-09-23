-- rvw store, schema version 1. Applied once, tracked by PRAGMA user_version.

-- One row per workspace (a git worktree root, or a plain directory). It owns
-- the id sequences, so ids stay stable and are never reused after a drop.
CREATE TABLE workspaces (
  id               INTEGER PRIMARY KEY,
  path             TEXT    NOT NULL UNIQUE, -- absolute, symlinks resolved
  next_comment_seq INTEGER NOT NULL DEFAULT 1, -- next r<N>
  next_review_seq  INTEGER NOT NULL DEFAULT 1  -- next rv<N>
);

-- A note on a line range of one file, with the code as it stood when written.
CREATE TABLE comments (
  workspace_id          INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
  seq                   INTEGER NOT NULL, -- public id is 'r' || seq
  status                TEXT    NOT NULL CHECK (status IN ('pending', 'pulled', 'done', 'rejected')),
  lane                  TEXT,             -- NULL: not scoped to a lane
  author                TEXT,
  file                  TEXT    NOT NULL, -- relative to the workspace when inside it
  path                  TEXT    NOT NULL, -- absolute
  start_line            INTEGER NOT NULL CHECK (start_line >= 1),
  end_line              INTEGER NOT NULL CHECK (end_line >= start_line),
  filetype              TEXT    NOT NULL DEFAULT '',
  code                  TEXT    NOT NULL,
  comment               TEXT    NOT NULL,
  created_at            TEXT    NOT NULL, -- RFC 3339, UTC, second resolution
  pulled_at             TEXT,
  resolved_at           TEXT,
  resolved_by           TEXT,
  resolution_note       TEXT,
  file_version          TEXT,             -- git blob of the file as reviewed
  resolved_file_version TEXT,             -- git blob of the file when resolved
  edited_at             TEXT,
  PRIMARY KEY (workspace_id, seq)
);

-- A submitted review: one decision and summary over zero or more comments.
CREATE TABLE reviews (
  workspace_id INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
  seq          INTEGER NOT NULL, -- public id is 'rv' || seq
  status       TEXT    NOT NULL CHECK (status IN ('pending', 'pulled')),
  lane         TEXT,
  author       TEXT,
  decision     TEXT    NOT NULL CHECK (decision IN ('comment', 'approve', 'request-changes')),
  summary      TEXT    NOT NULL,
  created_at   TEXT    NOT NULL,
  pulled_at    TEXT,
  PRIMARY KEY (workspace_id, seq)
);

-- Ordered links from a review to its comments. A comment belongs to at most one
-- review. comment_seq deliberately has no foreign key: a link whose comment is
-- gone is reported as missing evidence rather than silently vanishing.
CREATE TABLE review_comments (
  workspace_id INTEGER NOT NULL,
  review_seq   INTEGER NOT NULL,
  position     INTEGER NOT NULL, -- order within the review
  comment_seq  INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, review_seq, position),
  UNIQUE (workspace_id, comment_seq),
  FOREIGN KEY (workspace_id, review_seq) REFERENCES reviews (workspace_id, seq) ON DELETE CASCADE
);
