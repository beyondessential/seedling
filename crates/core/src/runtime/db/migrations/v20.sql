-- Schema changes only; Rust backfill code in v20.rs is not part of the hash.
-- The [backfill] marker below is used by v20.rs to split execution around the
-- Rust data migration. Do not remove or move it.
CREATE TABLE IF NOT EXISTS script_bodies (
    hash TEXT PRIMARY KEY,
    body TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS generations (
    app            TEXT    NOT NULL,
    generation     INTEGER NOT NULL,
    created_at     TEXT    NOT NULL,
    kind           TEXT    NOT NULL,
    param_name     TEXT,
    previous_value TEXT,
    new_value      TEXT,
    script_hash    TEXT    NOT NULL,
    operation_id   TEXT,
    outcome        TEXT,
    outcome_error  TEXT,
    PRIMARY KEY (app, generation)
);

CREATE INDEX IF NOT EXISTS idx_generations_app
    ON generations (app, generation DESC);

ALTER TABLE registered_apps
    ADD COLUMN current_generation INTEGER NOT NULL DEFAULT 0;

-- [backfill]
ALTER TABLE registered_apps DROP COLUMN current_version_id;
DROP TABLE app_versions;
