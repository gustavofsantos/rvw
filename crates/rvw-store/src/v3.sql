-- rvw store, schema version 3: the sessions agents report and the commits they
-- made. A fresh database applies schema.sql, v2.sql, then this file; an older
-- one applies the files after its version. Tracked by PRAGMA user_version.

-- One row per session an agent reported. The first report fixes tool, model and
-- summary; later reports only add commits.
CREATE TABLE sessions (
  workspace_id INTEGER NOT NULL REFERENCES workspaces (id) ON DELETE CASCADE,
  session_id   TEXT    NOT NULL, -- the agent's own id, unique per workspace
  tool         TEXT    NOT NULL, -- the agent tool, e.g. claude-code
  model        TEXT    NOT NULL, -- the model the agent reported
  summary      TEXT,             -- NULL: the agent gave none
  reported_at  TEXT    NOT NULL, -- RFC 3339, UTC, when the first report arrived
  PRIMARY KEY (workspace_id, session_id)
);

-- The commits a session reported, in report order. Ids are stored as reported,
-- full or abbreviated; a commit is stored once per session.
CREATE TABLE session_commits (
  workspace_id INTEGER NOT NULL,
  session_id   TEXT    NOT NULL,
  commit_id    TEXT    NOT NULL,
  position     INTEGER NOT NULL, -- order within the session
  PRIMARY KEY (workspace_id, session_id, commit_id),
  UNIQUE (workspace_id, session_id, position),
  FOREIGN KEY (workspace_id, session_id) REFERENCES sessions (workspace_id, session_id) ON DELETE CASCADE
);
