-- Uptime checks, their incidents, and where alerts go.
--
-- What each run found is history, and lives in metrics.db beside the
-- resource figures; losing it loses only history. What is here is set up
-- by hand and worth keeping.

CREATE TABLE IF NOT EXISTS checks (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    host_id           INTEGER NOT NULL REFERENCES hosts (id) ON DELETE CASCADE,
    name              TEXT    NOT NULL,
    kind              TEXT    NOT NULL CHECK (kind IN ('http', 'tcp', 'container')),
    -- A URL, host:port, or a container name. Not a secret: anyone who can
    -- see the host can see what is checked.
    target            TEXT    NOT NULL,
    interval_s        INTEGER NOT NULL DEFAULT 60 CHECK (interval_s >= 20),
    timeout_s         INTEGER NOT NULL DEFAULT 10 CHECK (timeout_s >= 1),
    retries           INTEGER NOT NULL DEFAULT 2 CHECK (retries >= 1),
    expect_status_min INTEGER NOT NULL DEFAULT 200,
    expect_status_max INTEGER NOT NULL DEFAULT 399,
    keyword           TEXT,
    latency_warn_ms   INTEGER,
    -- The stack whose page and board row show it. Forgetting the stack
    -- keeps the check.
    stack_id          INTEGER REFERENCES stacks (id) ON DELETE SET NULL,
    enabled           INTEGER NOT NULL DEFAULT 1,
    notify            INTEGER NOT NULL DEFAULT 1,
    created_at        INTEGER NOT NULL,
    UNIQUE (host_id, name)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_checks_stack ON checks (stack_id);

-- A stretch of time a check was down. Open while ended_at is NULL, which
-- survives a restart: coming back up later closes it.
CREATE TABLE IF NOT EXISTS incidents (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    check_id   INTEGER NOT NULL REFERENCES checks (id) ON DELETE CASCADE,
    started_at INTEGER NOT NULL,
    ended_at   INTEGER,
    cause      TEXT    NOT NULL
) STRICT;

CREATE INDEX IF NOT EXISTS idx_incidents_check ON incidents (check_id, started_at DESC);

-- Where alerts go. The URL and token are sealed, each under its own
-- purpose, and never returned: webhook URLs carry their secret in the path.
-- `host` is the part of the URL that is shown.
CREATE TABLE IF NOT EXISTS alert_channels (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL UNIQUE,
    kind       TEXT    NOT NULL CHECK (kind IN ('webhook', 'ntfy')),
    url_enc    TEXT    NOT NULL,
    token_enc  TEXT,
    host       TEXT    NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

-- Resource rules: alert when a metric stays over a threshold.
CREATE TABLE IF NOT EXISTS alert_rules (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    subject    TEXT    NOT NULL,
    metric     TEXT    NOT NULL CHECK (metric IN ('cpu', 'memory', 'disk')),
    above_pct  REAL    NOT NULL CHECK (above_pct > 0 AND above_pct <= 100),
    for_min    INTEGER NOT NULL CHECK (for_min >= 1),
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL
) STRICT;

-- What was sent where, and how it went: the latest 200.
CREATE TABLE IF NOT EXISTS alert_deliveries (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    channel  TEXT    NOT NULL,
    subject  TEXT    NOT NULL,
    state    TEXT    NOT NULL,
    at       INTEGER NOT NULL,
    ok       INTEGER NOT NULL,
    attempts INTEGER NOT NULL,
    error    TEXT
) STRICT;
