-- Remove the overly broad UNIQUE constraint on display_name: Service/Ingress/Volume
-- resources may share a display name across kinds, and the silent INSERT OR IGNORE
-- failure caused those resources to never persist a stable instance ID.
CREATE TABLE IF NOT EXISTS resource_instances_new (
    id           TEXT    PRIMARY KEY,
    app          TEXT    NOT NULL,
    kind         TEXT    NOT NULL,
    name         TEXT,
    is_scaled    INTEGER NOT NULL DEFAULT 0,
    display_name TEXT    NOT NULL,
    created_at   INTEGER NOT NULL
);

INSERT OR IGNORE INTO resource_instances_new
    SELECT id, app, kind, name, is_scaled, display_name, created_at
    FROM resource_instances;

DROP TABLE resource_instances;

ALTER TABLE resource_instances_new RENAME TO resource_instances;
