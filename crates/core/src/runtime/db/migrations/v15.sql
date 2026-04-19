CREATE TABLE IF NOT EXISTS scaling_decisions (
    app        TEXT    NOT NULL,
    deployment TEXT    NOT NULL,
    scale      INTEGER NOT NULL,
    updated_at TEXT    NOT NULL,
    PRIMARY KEY (app, deployment)
);
