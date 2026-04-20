-- i[impl backup.app.register]
-- Drop the backup-app nickname. The operator-chosen `name` field on
-- backup_apps added nothing but an extra indirection that was the source
-- of a wrong-name lookup bug (run_operation_for_backup keyed on the
-- nickname but the registry keyed on the BSL app name). A backup app is
-- now just a BSL app that's been opted-in to the backup role; its
-- identifier is the BSL app name.

-- Rewrite every strategy's `via` from the old nickname to the BSL app name
-- BEFORE we drop the name column. Strategies that referenced a now-gone
-- nickname keep their existing (broken) via value — nothing fires for them
-- until the operator corrects them manually.
UPDATE backup_strategies
SET via = (
    SELECT app
    FROM backup_apps
    WHERE backup_apps.name = backup_strategies.via
)
WHERE EXISTS (
    SELECT 1 FROM backup_apps WHERE backup_apps.name = backup_strategies.via
);

-- Collapse backup_apps to a single-column table. SQLite can't drop a
-- PRIMARY KEY column in place, so migrate via a rebuild.
CREATE TABLE backup_apps_new (
    app TEXT PRIMARY KEY
);

INSERT OR IGNORE INTO backup_apps_new (app)
SELECT app FROM backup_apps;

DROP TABLE backup_apps;
ALTER TABLE backup_apps_new RENAME TO backup_apps;
