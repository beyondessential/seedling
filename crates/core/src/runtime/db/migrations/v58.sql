-- Schema changes only; Rust backfill code in v58.rs is not part of the hash.
-- The [backfill] marker below is used by v58.rs to split execution around the
-- Rust data migration. Do not remove or move it.

-- r[impl generation.script-storage]
-- App definitions are bundles of files, stored content-addressed by the
-- bundle's content hash in its canonical encoding.
CREATE TABLE IF NOT EXISTS definition_bundles (
    hash     TEXT PRIMARY KEY,
    contents BLOB NOT NULL
);

ALTER TABLE generations RENAME COLUMN script_hash TO bundle_hash;

-- i[impl definition.provenance]
-- The provenance source of the definition a Register or ScriptUpdate
-- generation installed, as JSON.
ALTER TABLE generations ADD COLUMN provenance TEXT;

ALTER TABLE templates ADD COLUMN bundle_hash TEXT;
ALTER TABLE templates ADD COLUMN provenance TEXT;

-- [backfill]
DROP TABLE script_bodies;
ALTER TABLE templates DROP COLUMN body;
