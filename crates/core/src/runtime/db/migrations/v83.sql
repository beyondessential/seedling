-- spec: SEC#app-seeds
CREATE TABLE app_seeds (
    app_name   TEXT PRIMARY KEY NOT NULL,
    ciphertext BLOB NOT NULL
);

-- Apps registered before seeds existed get theirs once the secret key is
-- loaded, which happens after migrations run; this row says that is owed.
CREATE TABLE app_seed_backfill (
    singleton INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1)
);
INSERT INTO app_seed_backfill DEFAULT VALUES;

-- spec: SEC#operation-seeds
ALTER TABLE current_operation ADD COLUMN seed_ciphertext BLOB;
