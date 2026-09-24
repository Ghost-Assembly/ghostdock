-- Whether a stack applies updates on its own.
--
-- A plain column addition, so no table rebuild: SQLite appends it in place.
ALTER TABLE stacks ADD COLUMN auto_apply INTEGER NOT NULL DEFAULT 0;

-- The result of the last time a stack was checked for updates.
--
-- One row per stack rather than a history: the question is always "what is
-- waiting now", and keeping every past answer would grow without bound for
-- information nobody reads.
CREATE TABLE IF NOT EXISTS update_checks (
    stack_id      INTEGER PRIMARY KEY REFERENCES stacks (id) ON DELETE CASCADE,
    checked_at    INTEGER NOT NULL,
    -- What the tracked ref points at now, for a Git-backed stack.
    remote_commit TEXT,
    -- Why the check failed, if it did. A failed check is reported as such
    -- rather than as "no update", which would be indistinguishable from
    -- everything being current.
    error         TEXT
) STRICT;

-- Per-image digest comparison.
CREATE TABLE IF NOT EXISTS image_checks (
    stack_id         INTEGER NOT NULL REFERENCES stacks (id) ON DELETE CASCADE,
    -- The reference as the compose file wrote it.
    image            TEXT    NOT NULL,
    running_digest   TEXT,
    available_digest TEXT,
    error            TEXT,
    PRIMARY KEY (stack_id, image)
) STRICT;
