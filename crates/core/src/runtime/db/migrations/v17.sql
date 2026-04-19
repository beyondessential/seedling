CREATE TABLE IF NOT EXISTS external_volume_mappings (
    app           TEXT    NOT NULL,
    external_name TEXT    NOT NULL,
    target_kind   TEXT    NOT NULL,
    target_app    TEXT,
    target_volume TEXT    NOT NULL,
    read_only     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (app, external_name)
);
