CREATE TABLE IF NOT EXISTS registered_apps (
    name      TEXT    PRIMARY KEY,
    script    TEXT    NOT NULL,
    installed INTEGER NOT NULL DEFAULT 0
);
