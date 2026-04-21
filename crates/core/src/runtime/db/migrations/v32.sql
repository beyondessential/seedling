-- r[operation.params] Generalise install_params_ciphertext to hold the
-- encrypted params for any replayable lifecycle operation, not just install.
-- The column shape is unchanged; this is a pure rename so existing rows
-- continue to decode correctly.
ALTER TABLE current_operation
    RENAME COLUMN install_params_ciphertext TO params_ciphertext;
