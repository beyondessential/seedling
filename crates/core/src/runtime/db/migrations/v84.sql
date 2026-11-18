-- A change to a parameter no longer runs a handler, so a request queued for
-- one has nothing to run.
DELETE FROM queued_operations WHERE trigger = 'param_change';
