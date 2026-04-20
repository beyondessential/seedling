-- i[action.invoke.install] Installing phase persistence.
ALTER TABLE registered_apps ADD COLUMN installing INTEGER NOT NULL DEFAULT 0;

-- r[operation.params] Install params persist encrypted alongside the
-- current_operation row so the operation can be replayed after a
-- restart. Cleared when the install completes (success or failure).
ALTER TABLE current_operation ADD COLUMN install_params_ciphertext BLOB;
