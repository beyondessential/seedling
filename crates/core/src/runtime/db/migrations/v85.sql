-- An operation runs against one generation, the one current when it was
-- dispatched, so the source and target pair collapses to the target.
ALTER TABLE queued_operations DROP COLUMN source_generation;
ALTER TABLE queued_operations RENAME COLUMN target_generation TO generation;
ALTER TABLE current_operation DROP COLUMN source_generation;
ALTER TABLE current_operation RENAME COLUMN target_generation TO generation;
