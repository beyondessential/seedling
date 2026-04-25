-- r[impl rt.signal]
-- Add an `extra` column to action_log so the rt.signal entry can record the
-- signal name alongside the target instances. Existing rows have NULL.
ALTER TABLE action_log ADD COLUMN extra TEXT;
