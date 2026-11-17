-- The operation-scoped volume bindings of the persisted operation, as a JSON
-- object keyed by each binding's generated name, so a replay can establish
-- them again under the names its persisted parameters hold. NULL is none.
ALTER TABLE current_operation ADD COLUMN volume_bindings TEXT;
