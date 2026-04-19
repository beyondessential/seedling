CREATE TABLE IF NOT EXISTS dynamic_resources (
    instance_id  TEXT PRIMARY KEY,
    app          TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    kind         TEXT NOT NULL,
    display_name TEXT NOT NULL
);
