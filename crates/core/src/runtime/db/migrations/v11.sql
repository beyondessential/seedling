CREATE TABLE IF NOT EXISTS allowed_registries (
    registry TEXT PRIMARY KEY
);

INSERT OR IGNORE INTO allowed_registries (registry) VALUES ('docker.io');
INSERT OR IGNORE INTO allowed_registries (registry) VALUES ('ghcr.io');
