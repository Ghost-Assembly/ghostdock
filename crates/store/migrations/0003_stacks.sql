-- Registered stacks.
--
-- `slug` is both the Compose project name and the on-disk directory name, so
-- it is validated as a path component before it ever reaches this table.
--
-- Environment variables are deliberately absent: they hold secrets, and they
-- arrive with Git credentials so that one encryption implementation covers
-- both rather than leaving plaintext to migrate later.
CREATE TABLE IF NOT EXISTS stacks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    host_id      INTEGER NOT NULL REFERENCES hosts (id) ON DELETE CASCADE,
    slug         TEXT    NOT NULL UNIQUE,
    name         TEXT    NOT NULL,
    -- 'inline' today; 'git' arrives in M4.
    source_kind  TEXT    NOT NULL CHECK (source_kind IN ('inline')),
    compose_yaml TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_stacks_host ON stacks (host_id);

-- One row per attempt to change a stack.
--
-- Rows are written before the command runs and updated when it finishes, so
-- an attempt that never completes -- a crash mid-deploy -- is visible as a
-- deployment still marked running rather than vanishing.
CREATE TABLE IF NOT EXISTS deployments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    stack_id    INTEGER NOT NULL REFERENCES stacks (id) ON DELETE CASCADE,
    action      TEXT    NOT NULL CHECK (action IN ('deploy', 'stop', 'restart', 'remove')),
    trigger     TEXT    NOT NULL CHECK (trigger IN ('manual', 'schedule')),
    status      TEXT    NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    exit_code   INTEGER,
    -- Compose's own output, verbatim. Opaque failure is the loudest complaint
    -- about existing tools, so the real text is kept rather than a summary.
    log         TEXT    NOT NULL DEFAULT '',
    started_at  INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;

CREATE INDEX IF NOT EXISTS idx_deployments_stack ON deployments (stack_id, started_at DESC);
