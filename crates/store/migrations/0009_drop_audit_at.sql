-- The trail is read newest first by id, which is both the rowid and the
-- order entries were written in, and breaks ties within a second that `at`
-- cannot. Nothing reads by `at`, so its index only cost every write.
DROP INDEX IF EXISTS idx_audit_at;
