CREATE TABLE IF NOT EXISTS backup_strategies (
    name     TEXT PRIMARY KEY,
    via      TEXT NOT NULL,
    schedule TEXT NOT NULL,
    volumes  TEXT NOT NULL
);
