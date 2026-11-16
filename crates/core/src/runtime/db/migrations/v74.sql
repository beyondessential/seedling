-- An external volume's lifecycle state, which the runtime derives from the
-- desired state each reconciliation iteration rather than from observations,
-- since it makes none of an external volume.
CREATE TABLE external_volume_states (
    instance_id TEXT NOT NULL PRIMARY KEY REFERENCES resource_instances (id) ON DELETE CASCADE,
    state       TEXT NOT NULL,
    changed_at  INTEGER NOT NULL
);

-- Observations earlier versions recorded of external volumes were never made.
DELETE FROM world_observations
WHERE instance_id IN (SELECT id FROM resource_instances WHERE kind = 'ExternalVolume');
