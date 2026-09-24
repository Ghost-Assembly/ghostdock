-- Resource history. A separate database from ghostdock.db: losing it loses
-- only history, and its steady writes do not weigh on the database that
-- holds accounts and secrets.

-- What is measured. A container is keyed by its name, not its Docker id: a
-- redeploy recreates it with a new id, and history must carry on.
CREATE TABLE IF NOT EXISTS subjects (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    kind       TEXT    NOT NULL CHECK (kind IN ('host', 'container', 'disk', 'network')),
    key        TEXT    NOT NULL,
    project    TEXT,
    service    TEXT,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL,
    UNIQUE (kind, key)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_subjects_project ON subjects (project);

-- For a disk, mem is space used and mem_limit capacity. A stopped
-- container has no rows: a gap, not zeros.
CREATE TABLE IF NOT EXISTS samples_1m (
    subject_id INTEGER NOT NULL REFERENCES subjects (id) ON DELETE CASCADE,
    t          INTEGER NOT NULL,
    cpu        REAL, cpu_max REAL,
    mem        INTEGER, mem_max INTEGER, mem_limit INTEGER,
    net_rx     REAL, net_tx REAL, io_read REAL, io_write REAL,
    throttled  REAL, load REAL,
    PRIMARY KEY (subject_id, t)
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS samples_15m (
    subject_id INTEGER NOT NULL REFERENCES subjects (id) ON DELETE CASCADE,
    t          INTEGER NOT NULL,
    cpu        REAL, cpu_max REAL,
    mem        INTEGER, mem_max INTEGER, mem_limit INTEGER,
    net_rx     REAL, net_tx REAL, io_read REAL, io_write REAL,
    throttled  REAL, load REAL,
    PRIMARY KEY (subject_id, t)
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS events (
    subject_id INTEGER NOT NULL REFERENCES subjects (id) ON DELETE CASCADE,
    t          INTEGER NOT NULL,
    kind       TEXT    NOT NULL CHECK (kind IN ('oom', 'restart'))
) STRICT;

CREATE INDEX IF NOT EXISTS idx_events_subject ON events (subject_id, kind, t);
