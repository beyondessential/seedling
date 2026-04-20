CREATE TABLE IF NOT EXISTS restart_generations (
    app        TEXT    NOT NULL,
    deployment TEXT    NOT NULL,
    generation INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT,
    PRIMARY KEY (app, deployment)
);
