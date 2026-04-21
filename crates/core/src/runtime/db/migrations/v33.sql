CREATE TABLE IF NOT EXISTS templates (
    name        TEXT    PRIMARY KEY,
    body        TEXT    NOT NULL,
    description TEXT,
    created_at  TEXT    NOT NULL
);
