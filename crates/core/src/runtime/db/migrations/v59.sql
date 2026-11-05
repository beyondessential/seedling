-- Backup apps and backup strategies no longer exist: the runtime has no
-- backup role, no strategy schedule and no backup execution path.
DROP TABLE IF EXISTS backup_strategies;
DROP TABLE IF EXISTS backup_apps;

-- The faults the backup path filed can no longer be cleared by anything that
-- would have cleared them, so any still active would stay on the fault list
-- for good. Retire them; the rows stay as history.
UPDATE faults
SET cleared_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE cleared_at IS NULL
  AND kind IN ('backup_failed', 'backup_source_unavailable', 'backup_app_unavailable');
