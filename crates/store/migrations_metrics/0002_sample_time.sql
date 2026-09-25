-- Rolling up a quarter hour and pruning past retention both select rows by
-- time alone. The (subject_id, t) key cannot serve that, so without these
-- each reads every row kept: a month of minutes for every subject.
CREATE INDEX IF NOT EXISTS idx_samples_1m_t ON samples_1m (t);
CREATE INDEX IF NOT EXISTS idx_samples_15m_t ON samples_15m (t);
