-- Git-backed stacks, the credentials to reach a remote, and stack
-- environment variables.

-- A secret for reaching a Git remote.
--
-- `secret_sealed` is encrypted and bound to its purpose; it must never be
-- returned by the API, only used.
CREATE TABLE IF NOT EXISTS credentials (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT    NOT NULL UNIQUE,
    username      TEXT    NOT NULL,
    secret_sealed TEXT    NOT NULL,
    created_at    INTEGER NOT NULL
) STRICT;

-- A Git remote. Several stacks commonly live in one repository.
CREATE TABLE IF NOT EXISTS repos (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    url           TEXT    NOT NULL UNIQUE,
    -- Kept when the credential is deleted, so the repository does not vanish
    -- with it; the next poll simply fails and says why.
    credential_id INTEGER REFERENCES credentials (id) ON DELETE SET NULL,
    created_at    INTEGER NOT NULL
) STRICT;

-- Stack environment variables.
--
-- Values are sealed: they routinely hold API keys, and a stack's .env is
-- written to disk for compose to read.
CREATE TABLE IF NOT EXISTS stack_env (
    stack_id     INTEGER NOT NULL REFERENCES stacks (id) ON DELETE CASCADE,
    key          TEXT    NOT NULL,
    value_sealed TEXT    NOT NULL,
    PRIMARY KEY (stack_id, key)
) STRICT;

-- Rebuilding `stacks` to allow a second source kind.
--
-- SQLite cannot alter a CHECK constraint, so the table is recreated. The
-- order below matters: `deployments` references `stacks` with ON DELETE
-- CASCADE, so dropping the parent first would silently delete every
-- deployment. The child is therefore rebuilt against the new table and the
-- old child dropped before the old parent.
CREATE TABLE stacks_new (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    host_id      INTEGER NOT NULL REFERENCES hosts (id) ON DELETE CASCADE,
    slug         TEXT    NOT NULL UNIQUE,
    name         TEXT    NOT NULL,
    source_kind  TEXT    NOT NULL CHECK (source_kind IN ('inline', 'git')),
    -- The compose file for an inline stack; empty for a Git-backed one,
    -- whose file is read from the repository at deploy time.
    compose_yaml TEXT    NOT NULL DEFAULT '',
    -- NO ACTION rather than RESTRICT: both refuse the delete, but SQLite
    -- reports a RESTRICT violation as SQLITE_CONSTRAINT_TRIGGER (1811) while
    -- NO ACTION reports SQLITE_CONSTRAINT_FOREIGNKEY (787), which is the code
    -- drivers actually classify as a foreign-key violation. Verified against
    -- SQLite directly.
    repo_id      INTEGER REFERENCES repos (id) ON DELETE NO ACTION,
    git_ref      TEXT,
    compose_path TEXT,
    -- The commit last deployed, for reporting how far behind a stack is.
    last_commit  TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    -- A Git stack is not addressable without all three, and an inline stack
    -- has no business carrying any of them.
    CHECK (
        (source_kind = 'inline'
            AND repo_id IS NULL AND git_ref IS NULL AND compose_path IS NULL)
        OR
        (source_kind = 'git'
            AND repo_id IS NOT NULL AND git_ref IS NOT NULL AND compose_path IS NOT NULL)
    )
) STRICT;

INSERT INTO stacks_new (id, host_id, slug, name, source_kind, compose_yaml, created_at, updated_at)
SELECT id, host_id, slug, name, source_kind, compose_yaml, created_at, updated_at FROM stacks;

CREATE TABLE deployments_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    stack_id    INTEGER NOT NULL REFERENCES stacks_new (id) ON DELETE CASCADE,
    action      TEXT    NOT NULL CHECK (action IN ('deploy', 'stop', 'restart', 'remove')),
    trigger     TEXT    NOT NULL CHECK (trigger IN ('manual', 'schedule')),
    status      TEXT    NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    exit_code   INTEGER,
    log         TEXT    NOT NULL DEFAULT '',
    -- The commit this attempt deployed, for a Git-backed stack.
    commit_sha  TEXT,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER
) STRICT;

INSERT INTO deployments_new (id, stack_id, action, trigger, status, exit_code, log, started_at, finished_at)
SELECT id, stack_id, action, trigger, status, exit_code, log, started_at, finished_at FROM deployments;

DROP TABLE deployments;
DROP TABLE stacks;

ALTER TABLE stacks_new RENAME TO stacks;
ALTER TABLE deployments_new RENAME TO deployments;

CREATE INDEX IF NOT EXISTS idx_stacks_host ON stacks (host_id);
CREATE INDEX IF NOT EXISTS idx_stacks_repo ON stacks (repo_id);
CREATE INDEX IF NOT EXISTS idx_deployments_stack ON deployments (stack_id, started_at DESC);
