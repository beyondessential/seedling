CREATE TABLE IF NOT EXISTS site_volumes (
    name       TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    host_path  TEXT,
    created_at TEXT NOT NULL
);
