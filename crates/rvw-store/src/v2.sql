-- rvw store, schema version 2: verdicts, "points at" references and gate
-- overrides. A fresh database applies schema.sql then this file; a version 1
-- database applies this file alone. Tracked by PRAGMA user_version.

-- The comment columns are NULL unless the feature is used. Each verdict column
-- is set together with `verdict`; the followup and reopen links are comment
-- seqs, like `seq` itself. The "points at" range is set only with points_file.
ALTER TABLE comments ADD COLUMN reopens_seq INTEGER;          -- the reopened comment's seq
ALTER TABLE comments ADD COLUMN points_file TEXT;             -- relative to the workspace
ALTER TABLE comments ADD COLUMN points_start INTEGER;         -- 1-indexed, inclusive
ALTER TABLE comments ADD COLUMN points_end INTEGER;           -- 1-indexed, inclusive
ALTER TABLE comments ADD COLUMN verdict TEXT CHECK (verdict IS NULL OR verdict IN ('accepted', 'reopened'));
ALTER TABLE comments ADD COLUMN verdict_by TEXT;              -- who gave the verdict
ALTER TABLE comments ADD COLUMN verdict_actor TEXT CHECK (verdict_actor IS NULL OR verdict_actor IN ('human', 'agent'));
ALTER TABLE comments ADD COLUMN verdict_at TEXT;              -- RFC 3339, UTC
ALTER TABLE comments ADD COLUMN verdict_note TEXT;
ALTER TABLE comments ADD COLUMN verdict_followup_seq INTEGER; -- the follow-up a reopen created

-- A recorded decision to go ahead past the commit or push gate. The id lists
-- are space-separated; an empty list is the empty string.
CREATE TABLE gate_overrides (
  workspace_id INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
  seq          INTEGER NOT NULL, -- per workspace, oldest first
  at           TEXT    NOT NULL, -- RFC 3339, UTC
  lane         TEXT,             -- NULL: the gate looked at every lane
  action       TEXT    NOT NULL CHECK (action IN ('commit', 'push')),
  reason       TEXT    NOT NULL,
  author       TEXT    NOT NULL,
  actor        TEXT    NOT NULL CHECK (actor IN ('human', 'agent')),
  open         TEXT    NOT NULL, -- open comment ids when overridden
  unjudged     TEXT    NOT NULL, -- decided comment ids still awaiting a verdict
  PRIMARY KEY (workspace_id, seq)
);
