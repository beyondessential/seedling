-- Plumb source/target generation through the operation record so replay can
-- reconstruct the right AppDef and the previous generation's state.
ALTER TABLE current_operation
    ADD COLUMN source_generation INTEGER NOT NULL DEFAULT 0;
ALTER TABLE current_operation
    ADD COLUMN target_generation INTEGER NOT NULL DEFAULT 0;
