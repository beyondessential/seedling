-- r[impl secret.storage]
CREATE TABLE IF NOT EXISTS secret_params (
    app_name   TEXT NOT NULL,
    param_name TEXT NOT NULL,
    ciphertext BLOB NOT NULL,
    PRIMARY KEY (app_name, param_name)
);

-- r[impl secret.history]
ALTER TABLE generations ADD COLUMN previous_value_ciphertext BLOB;
ALTER TABLE generations ADD COLUMN new_value_ciphertext BLOB;
