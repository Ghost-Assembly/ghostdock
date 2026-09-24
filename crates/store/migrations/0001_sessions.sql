-- Server-side session storage.
--
-- `id` is tower-sessions' 128-bit session id in its canonical base64 form
-- (22 chars, URL-safe, unpadded). SQLite has no 128-bit integer type, so the
-- textual form is the honest representation rather than a lossy split.
--
-- `expiry_date` is a Unix timestamp in seconds. Storing an integer keeps the
-- expiry predicate indexable and avoids depending on SQLite's date functions.
CREATE TABLE IF NOT EXISTS sessions (
    id          TEXT    PRIMARY KEY NOT NULL,
    data        BLOB    NOT NULL,
    expiry_date INTEGER NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_sessions_expiry_date ON sessions (expiry_date);
