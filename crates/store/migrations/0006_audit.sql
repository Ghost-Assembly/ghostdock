-- Who did what.
--
-- `username` is stored alongside the id rather than only referenced. An
-- audit trail that loses its names when an account is deleted answers the
-- question "who did this" with "someone", which is the one thing it exists
-- not to do.
CREATE TABLE IF NOT EXISTS audit_log (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    at       INTEGER NOT NULL,
    user_id  INTEGER REFERENCES users (id) ON DELETE SET NULL,
    username TEXT    NOT NULL,
    -- What was done, as a stable identifier rather than prose.
    action   TEXT    NOT NULL,
    -- What it was done to, in words a person recognises.
    target   TEXT    NOT NULL,
    detail   TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS idx_audit_at ON audit_log (at DESC);
