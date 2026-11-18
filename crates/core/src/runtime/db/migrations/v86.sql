-- A generation no longer records the outcome of an operation: a change
-- initiates no operation, so there is none to record.
ALTER TABLE generations DROP COLUMN operation_id;
ALTER TABLE generations DROP COLUMN outcome;
ALTER TABLE generations DROP COLUMN outcome_error;
