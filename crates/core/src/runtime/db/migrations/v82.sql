-- A parameter change may move several parameters, so what a generation did to
-- each parameter is a row of its own rather than columns of the generation.
-- A value is held in plaintext or, for one that was protected, as ciphertext.
CREATE TABLE generation_params (
    app                       TEXT    NOT NULL,
    generation                INTEGER NOT NULL,
    param_name                TEXT    NOT NULL,
    previous_value            TEXT,
    previous_value_ciphertext BLOB,
    new_value                 TEXT,
    new_value_ciphertext      BLOB,
    PRIMARY KEY (app, generation, param_name),
    FOREIGN KEY (app, generation) REFERENCES generations (app, generation)
        ON DELETE CASCADE
);

INSERT INTO generation_params
    (app, generation, param_name, previous_value, previous_value_ciphertext,
     new_value, new_value_ciphertext)
SELECT app, generation, param_name, previous_value, previous_value_ciphertext,
       new_value, new_value_ciphertext
FROM generations
WHERE param_name IS NOT NULL;

UPDATE generations
SET kind = 'param_change'
WHERE kind IN ('param_set', 'param_unset');

ALTER TABLE generations DROP COLUMN param_name;
ALTER TABLE generations DROP COLUMN previous_value;
ALTER TABLE generations DROP COLUMN previous_value_ciphertext;
ALTER TABLE generations DROP COLUMN new_value;
ALTER TABLE generations DROP COLUMN new_value_ciphertext;
