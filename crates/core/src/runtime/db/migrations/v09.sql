CREATE TABLE IF NOT EXISTS faults (
    id            TEXT PRIMARY KEY,
    app           TEXT NOT NULL,
    resource_type TEXT,
    resource_name TEXT,
    instance_id   TEXT,
    kind          TEXT NOT NULL,
    timestamp     TEXT NOT NULL,
    description   TEXT NOT NULL,
    cleared_at    TEXT
);
