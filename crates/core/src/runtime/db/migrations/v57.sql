-- r[impl priority.settings]
-- The operator-set app priority for each installed app. One row per app; the
-- absence of a row means the app is at its default priority (`normal`). The
-- value is one of `high`, `normal`, or `low`. Discarded when the app is
-- uninstalled or deregistered, so a later reinstall starts again at `normal`.
CREATE TABLE IF NOT EXISTS app_priorities (
    app        TEXT NOT NULL PRIMARY KEY,
    priority   TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
