CREATE TABLE IF NOT EXISTS stopped_resources (
    app  TEXT NOT NULL,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    PRIMARY KEY (app, kind, name)
);
