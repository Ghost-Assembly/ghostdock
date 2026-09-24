-- Local user accounts.
--
-- `username` collates case-insensitively so "Admin" and "admin" cannot both
-- exist; a login field that is unique only by byte value invites confusion.
-- `password_hash` is a PHC string (argon2id) and must never leave the server.
CREATE TABLE IF NOT EXISTS users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    password_hash TEXT    NOT NULL,
    created_at    INTEGER NOT NULL
) STRICT;

-- Docker hosts.
--
-- v1 manages exactly one, seeded below. The table exists from the start so
-- that every resource can be addressed beneath a host id, making multi-host
-- a later feature rather than a migration through every table.
CREATE TABLE IF NOT EXISTS hosts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
) STRICT;

INSERT INTO hosts (id, name, created_at)
VALUES (1, 'local', unixepoch())
ON CONFLICT (id) DO NOTHING;
