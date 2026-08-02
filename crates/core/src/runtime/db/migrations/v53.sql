-- r[impl fault.lifecycle]
-- Give faults a first-class subject: the thing that is faulty.
--
-- Until now the subject was smeared across three columns and, where none of
-- them fitted, the description text — image refs, `host:port` tuples, `[key]`
-- prefixes. Matching by description-substring is fragile, and where no column
-- fitted at all the subject was simply absent, so clearing had to fall back to
-- app + kind. That is what made a successful backup of one volume clear every
-- other volume's `backup_failed` fault.
--
-- resource_type / resource_name / instance_id stay as display metadata;
-- identity is (app, kind, subject).
ALTER TABLE faults ADD COLUMN subject TEXT NOT NULL DEFAULT '';

-- Backfill the existing active faults from whichever column carried their
-- subject, so a fault filed before this migration still matches the key its
-- site will compute after it and can be cleared rather than stranded.
UPDATE faults
SET subject = COALESCE(instance_id, resource_name, '')
WHERE cleared_at IS NULL AND subject = '';

-- Duplicate active faults for one key already exist (audit_lag files without
-- any dedup at all), and the index below would refuse to build over them.
-- Clear all but the newest of each group rather than deleting: the fault list
-- is built from this table's history.
UPDATE faults
SET cleared_at = timestamp
WHERE cleared_at IS NULL
  AND id NOT IN (
      SELECT id FROM (
          SELECT id,
                 ROW_NUMBER() OVER (
                     PARTITION BY app, kind, subject
                     ORDER BY timestamp DESC, id DESC
                 ) AS rn
          FROM faults
          WHERE cleared_at IS NULL
      )
      WHERE rn = 1
  );

-- At most one active fault per key. Partial, so cleared rows accumulate freely
-- for the history the fault list is built from.
CREATE UNIQUE INDEX IF NOT EXISTS faults_active_key
    ON faults (app, kind, subject)
    WHERE cleared_at IS NULL;
