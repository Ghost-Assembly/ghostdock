-- A counter moved on whenever an account's password changes.
--
-- Each session records the value it was issued under; a session carrying an
-- older one is refused. That signs out every other device on a password
-- change without having to find sessions by user in the session store, whose
-- records are opaque blobs.
ALTER TABLE users ADD COLUMN session_epoch INTEGER NOT NULL DEFAULT 0;
