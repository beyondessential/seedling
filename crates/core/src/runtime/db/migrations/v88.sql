-- A gate run is a lifecycle operation, so the operation records name the gate
-- beside the action: an operation runs one or the other.
CREATE TABLE queued_operations_new (
    seq               INTEGER PRIMARY KEY AUTOINCREMENT,
    operation_id      TEXT NOT NULL UNIQUE,
    app               TEXT NOT NULL,
    action            TEXT,
    gate              TEXT,
    trigger           TEXT NOT NULL,
    generation        INTEGER NOT NULL,
    params_ciphertext BLOB NOT NULL,
    actor             TEXT,
    CHECK ((action IS NULL) != (gate IS NULL))
);
INSERT INTO queued_operations_new
    (seq, operation_id, app, action, trigger, generation, params_ciphertext, actor)
SELECT seq, operation_id, app, action, trigger, generation, params_ciphertext, actor
FROM queued_operations;
DROP TABLE queued_operations;
ALTER TABLE queued_operations_new RENAME TO queued_operations;

CREATE TABLE current_operation_new (
    singleton         INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    operation_id      TEXT NOT NULL,
    app               TEXT NOT NULL,
    action_name       TEXT,
    gate              TEXT,
    generation        INTEGER NOT NULL DEFAULT 0,
    params_ciphertext BLOB,
    cancel_requested  INTEGER NOT NULL DEFAULT 0,
    volume_bindings   TEXT,
    CHECK ((action_name IS NULL) != (gate IS NULL))
);
INSERT INTO current_operation_new
    (singleton, operation_id, app, action_name, generation, params_ciphertext,
     cancel_requested, volume_bindings)
SELECT singleton, operation_id, app, action_name, generation, params_ciphertext,
       cancel_requested, volume_bindings
FROM current_operation;
DROP TABLE current_operation;
ALTER TABLE current_operation_new RENAME TO current_operation;

-- What the runtime also keeps of a gate between ticks: the key a failed run
-- was for, so that a move of the key makes the next run due at once, and how
-- its check has failed to run.
ALTER TABLE gate_records ADD COLUMN failed_key_digest BLOB;
ALTER TABLE gate_records ADD COLUMN check_failures INTEGER NOT NULL DEFAULT 0;
ALTER TABLE gate_records ADD COLUMN check_unavailable_at TEXT;
ALTER TABLE gate_records ADD COLUMN check_unavailable TEXT;
