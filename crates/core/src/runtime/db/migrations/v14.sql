-- Schema changes only; Rust backfill code in v14.rs is not part of the hash.
CREATE TABLE IF NOT EXISTS app_versions (
    id         TEXT PRIMARY KEY,
    app        TEXT NOT NULL,
    script     TEXT NOT NULL,
    created_at TEXT NOT NULL
);

ALTER TABLE registered_apps ADD COLUMN current_version_id TEXT;
