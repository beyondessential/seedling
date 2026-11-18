-- Whether a dynamic resource was created by a shell session rather than by a
-- lifecycle operation. `operation_id` holds the id of whichever created it;
-- this says which kind of id that is.
ALTER TABLE dynamic_resources ADD COLUMN shell_session INTEGER NOT NULL DEFAULT 0;
