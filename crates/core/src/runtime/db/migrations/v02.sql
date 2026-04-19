-- Identity overhaul: drop pre-v2 tables and recreate with instance_id references.
DROP TABLE IF EXISTS world_observations;
DROP TABLE IF EXISTS autonomous_operations;
DELETE FROM schema_version;

CREATE TABLE IF NOT EXISTS resource_instances (
    id           TEXT    PRIMARY KEY,
    app          TEXT    NOT NULL,
    kind         TEXT    NOT NULL,
    name         TEXT,
    is_scaled    INTEGER NOT NULL DEFAULT 0,
    display_name TEXT    NOT NULL,
    created_at   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS world_observations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at INTEGER NOT NULL,
    instance_id TEXT    NOT NULL,
    obs_kind    TEXT    NOT NULL,
    payload     TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS autonomous_operations (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at  INTEGER NOT NULL,
    instance_id  TEXT    NOT NULL,
    operation    TEXT    NOT NULL,
    provenance   TEXT    NOT NULL,
    outcome      TEXT,
    completed_at INTEGER
);

CREATE TABLE IF NOT EXISTS action_log (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at        INTEGER NOT NULL,
    operation_id       TEXT    NOT NULL,
    app                TEXT    NOT NULL,
    action_name        TEXT    NOT NULL,
    call_index         INTEGER NOT NULL,
    call_kind          TEXT    NOT NULL,
    resources          TEXT    NOT NULL,
    barrier_state      TEXT,
    barrier_deadline   INTEGER,
    barrier_satisfied  INTEGER,
    barrier_started_at INTEGER,
    UNIQUE (operation_id, call_index)
);

CREATE TABLE IF NOT EXISTS current_operation (
    singleton    INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    operation_id TEXT    NOT NULL,
    app          TEXT    NOT NULL,
    action_name  TEXT    NOT NULL
);
