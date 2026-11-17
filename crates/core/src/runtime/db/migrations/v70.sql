-- Lifecycle operation requests waiting behind another app's operation.
--
-- A request is stored before it is accepted, in the order it was made, with
-- everything the operation needs to start: its parameters encrypted under the
-- secret key as a persisted operation's are, and the actor that asked, so the
-- operation's events can still be attributed to them after a restart.
CREATE TABLE queued_operations (
    seq               INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id      TEXT NOT NULL UNIQUE,
    app               TEXT NOT NULL,
    action            TEXT NOT NULL,
    trigger           TEXT NOT NULL,
    source_generation INTEGER NOT NULL,
    target_generation INTEGER NOT NULL,
    params_ciphertext BLOB NOT NULL,
    actor             TEXT
);
