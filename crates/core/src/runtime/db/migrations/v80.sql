-- The storage of each anonymous volume created for a pod instance.
--
-- An anonymous volume is removed when the pod instance it was created for
-- stops. A pod an operation created may be stopped once its definition is
-- gone, from its recorded identity alone, so each anonymous volume's storage
-- is recorded here before it is created, and the record removed once the
-- storage is. `tmpfs` says which of the two storage addresses the volume name
-- is at.
CREATE TABLE anonymous_volumes (
    instance_id TEXT NOT NULL,
    app         TEXT NOT NULL,
    volume      TEXT NOT NULL,
    tmpfs       INTEGER NOT NULL,
    PRIMARY KEY (instance_id, volume, tmpfs)
);
CREATE INDEX anonymous_volumes_by_volume ON anonymous_volumes (volume, tmpfs);
CREATE INDEX anonymous_volumes_by_app ON anonymous_volumes (app);
