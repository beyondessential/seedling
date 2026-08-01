-- r[impl autonomous.restart.record]
-- One row per observed or performed restart of a container instance.
--
-- `initiator` is 'supervisor' when the platform's service supervisor actioned
-- the restart, and 'runtime' when seedling itself did (rolling update, health
-- check replacement, operator-requested restart). Only supervisor rows count
-- towards the crash-loop rate.
--
-- `exit_kind` is 'exited', 'signalled' or 'dumped'; `exit_code` is the exit
-- status for 'exited' and the signal number otherwise. Both are NULL when the
-- platform did not report an exit for the run that ended.
CREATE TABLE IF NOT EXISTS instance_restarts (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    instance_id   TEXT    NOT NULL,
    app           TEXT    NOT NULL,
    resource_type TEXT,
    resource_name TEXT,
    generation    INTEGER,
    recorded_at   INTEGER NOT NULL,
    initiator     TEXT    NOT NULL,
    exit_code     INTEGER,
    exit_kind     TEXT
);

CREATE INDEX IF NOT EXISTS idx_instance_restarts_instance
    ON instance_restarts (instance_id, recorded_at);
CREATE INDEX IF NOT EXISTS idx_instance_restarts_app
    ON instance_restarts (app, recorded_at);

-- Last restart counter read from the supervisor for an instance's unit. The
-- counter is monotonic per unit but resets when the unit is recreated or its
-- failed state is cleared, so a decrease is a reset to re-baseline against,
-- not a negative delta.
CREATE TABLE IF NOT EXISTS instance_restart_counters (
    instance_id TEXT    PRIMARY KEY,
    counter     INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- r[impl autonomous.restart.rate.settings]
-- Crash-loop rate threshold and window. One row, enforced via the singleton
-- primary key.
--
-- The default of five supervisor-actioned restarts within thirty minutes is
-- loose enough that a container taking seconds to crash gets several chances
-- across a deploy, and tight enough that persistent flapping surfaces within
-- an operator's working session rather than a day later.
CREATE TABLE IF NOT EXISTS restart_settings (
    singleton   INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    threshold   INTEGER NOT NULL DEFAULT 5,
    window_secs INTEGER NOT NULL DEFAULT 1800,
    updated_at  INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO restart_settings (singleton, threshold, window_secs, updated_at)
    VALUES (1, 5, 1800, 0);
