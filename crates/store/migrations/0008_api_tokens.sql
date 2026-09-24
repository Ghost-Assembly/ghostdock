-- API tokens.
--
-- Only a SHA-256 of the secret is kept. The secret is 256 random bits, so a
-- fast hash is enough: there is no low-entropy password here for a slow one
-- to protect. `prefix` is the start of the secret, kept so a token found in a
-- config file can be matched to its row.
--
-- `permissions` is a space-separated list of names. A name this version does
-- not know is ignored when read, which can only ever grant less.
CREATE TABLE IF NOT EXISTS api_tokens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id      INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    prefix       TEXT    NOT NULL,
    secret_hash  BLOB    NOT NULL UNIQUE,
    permissions  TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    last_used_at INTEGER,
    expires_at   INTEGER,
    UNIQUE (user_id, name)
) STRICT;
