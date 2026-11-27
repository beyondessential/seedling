-- The breaker of an app's definition or of one of its action closures: the
-- digest of the definition and inputs an evaluation of it last ran past the
-- evaluation time limit with. A breaker is tripped while its
-- `evaluation_runaway` fault is active; a row whose fault has cleared is not.
CREATE TABLE breakers (
    app TEXT NOT NULL,
    subject TEXT NOT NULL,
    digest BLOB NOT NULL,
    generation INTEGER NOT NULL,
    tripped_at TEXT NOT NULL,
    PRIMARY KEY (app, subject)
);
