-- Level-triggered gates replace change handlers and the install and start
-- actions, and a parameter change may move several parameters at once.

-- What a generation did to each parameter is a row of its own rather than
-- columns of the generation. A value is held in plaintext or, for one that was
-- protected, as ciphertext.
CREATE TABLE generation_params (
    app                       TEXT    NOT NULL,
    generation                INTEGER NOT NULL,
    param_name                TEXT    NOT NULL,
    previous_value            TEXT,
    previous_value_ciphertext BLOB,
    new_value                 TEXT,
    new_value_ciphertext      BLOB,
    PRIMARY KEY (app, generation, param_name),
    FOREIGN KEY (app, generation) REFERENCES generations (app, generation)
        ON DELETE CASCADE
);
-- Reconstructing a generation reads the latest change of each parameter up
-- to it.
CREATE INDEX generation_params_by_name
    ON generation_params (app, param_name, generation DESC);

INSERT INTO generation_params
    (app, generation, param_name, previous_value, previous_value_ciphertext,
     new_value, new_value_ciphertext)
SELECT app, generation, param_name, previous_value, previous_value_ciphertext,
       new_value, new_value_ciphertext
FROM generations
WHERE param_name IS NOT NULL;

UPDATE generations
SET kind = 'param_change'
WHERE kind IN ('param_set', 'param_unset');

ALTER TABLE generations DROP COLUMN param_name;
ALTER TABLE generations DROP COLUMN previous_value;
ALTER TABLE generations DROP COLUMN previous_value_ciphertext;
ALTER TABLE generations DROP COLUMN new_value;
ALTER TABLE generations DROP COLUMN new_value_ciphertext;

-- A generation no longer records the outcome of an operation: a change
-- initiates no operation, so there is none to record.
ALTER TABLE generations DROP COLUMN operation_id;
ALTER TABLE generations DROP COLUMN outcome;
ALTER TABLE generations DROP COLUMN outcome_error;

-- Installing an app no longer runs a closure, so an app is never part-way
-- through an install: one that was becomes installed.
UPDATE registered_apps SET installed = 1 WHERE installing = 1;
ALTER TABLE registered_apps DROP COLUMN installing;

-- Requests queued for the install and start actions, for a boot, or for a
-- parameter change's handler have nothing left to run.
DELETE FROM queued_operations
WHERE trigger IN ('boot', 'install', 'param_change')
   OR action IN ('install', 'start');
-- Nor has an install, a start or a parameter's change handler interrupted by
-- a restart, the last persisted under the parameter's name. An action that
-- shares a parameter's name goes with it: it is not replayed, which is safer
-- than replaying a handler as that action.
DELETE FROM current_operation
WHERE action_name IN ('install', 'start')
   OR action_name IN (SELECT param_name FROM generation_params WHERE app = current_operation.app)
   OR action_name IN (SELECT param_name FROM params WHERE app_name = current_operation.app)
   OR action_name IN (SELECT param_name FROM secret_params WHERE app_name = current_operation.app);

-- An operation runs against one generation, the one current when it was
-- dispatched, and is a run of an action or of a gate: never both.
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
SELECT seq, operation_id, app, action, trigger, target_generation, params_ciphertext, actor
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
SELECT singleton, operation_id, app, action_name, target_generation, params_ciphertext,
       cancel_requested, volume_bindings
FROM current_operation;
DROP TABLE current_operation;
ALTER TABLE current_operation_new RENAME TO current_operation;

-- What the runtime knows of each gate an app declares: the digest of the key
-- of its last success and the generation that run was for, whether that
-- success still stands, the runs that failed since, the key a cancelled run
-- paused it for, the key a failed run was for (so that a move of the key makes
-- the next run due at once), and how its check has failed to run. Only a
-- digest of a key is held, since a key may carry a secret.
CREATE TABLE gate_records (
    app                  TEXT    NOT NULL,
    gate                 TEXT    NOT NULL,
    key_digest           BLOB,
    success_generation   INTEGER,
    valid                INTEGER NOT NULL DEFAULT 0,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_failure_at      TEXT,
    last_failure         TEXT,
    paused_key_digest    BLOB,
    failed_key_digest    BLOB,
    check_failures       INTEGER NOT NULL DEFAULT 0,
    check_unavailable_at TEXT,
    check_unavailable    TEXT,
    PRIMARY KEY (app, gate)
);

-- The resources a gate's run holds stopped, so that the hold outlasts a
-- restart of the runtime and a failed run.
CREATE TABLE gate_holds (
    app  TEXT NOT NULL,
    gate TEXT NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (app, gate, kind, name)
);

-- An app that was installed before gates ran its setup then.
ALTER TABLE registered_apps ADD COLUMN set_up INTEGER NOT NULL DEFAULT 0;
UPDATE registered_apps SET set_up = installed;

-- A dynamic resource's record names what created it, and so what its
-- operation_id names: a lifecycle operation, a shell session, or a check of a
-- gate, whose Job is neither an operation's nor a session's.
ALTER TABLE dynamic_resources ADD COLUMN creator TEXT NOT NULL DEFAULT 'operation'
    CHECK (creator IN ('operation', 'shell_session', 'gate_check'));
UPDATE dynamic_resources SET creator = 'shell_session' WHERE shell_session != 0;
ALTER TABLE dynamic_resources DROP COLUMN shell_session;
