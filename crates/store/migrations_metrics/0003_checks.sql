-- What each uptime check run found: raw for a week, by the hour for a
-- year. Keyed by the check's id in ghostdock.db; a check that is removed
-- takes its rows with it.
-- Not keyed by (check_id, t): a run asked for by hand can land in the same
-- second as a scheduled one, and both happened.
CREATE TABLE IF NOT EXISTS check_samples (
    check_id   INTEGER NOT NULL,
    t          INTEGER NOT NULL,
    ok         INTEGER NOT NULL,
    latency_ms INTEGER
) STRICT;

CREATE INDEX IF NOT EXISTS idx_check_samples_check ON check_samples (check_id, t);

CREATE TABLE IF NOT EXISTS check_hourly (
    check_id    INTEGER NOT NULL,
    t           INTEGER NOT NULL,
    up          INTEGER NOT NULL,
    total       INTEGER NOT NULL,
    latency_avg REAL,
    latency_max REAL,
    PRIMARY KEY (check_id, t)
) STRICT, WITHOUT ROWID;

-- Rolling up and pruning select by time alone.
CREATE INDEX IF NOT EXISTS idx_check_samples_t ON check_samples (t);
CREATE INDEX IF NOT EXISTS idx_check_hourly_t ON check_hourly (t);
