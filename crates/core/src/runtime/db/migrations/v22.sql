CREATE TABLE IF NOT EXISTS action_schedules (
    app           TEXT NOT NULL,
    action        TEXT NOT NULL,
    cronexpr      TEXT NOT NULL,
    last_fired_at TEXT,
    PRIMARY KEY (app, action, cronexpr)
);
