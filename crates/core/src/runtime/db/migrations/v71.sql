-- Whether an action log entry records only that a call was begun.
--
-- rt.signal, rt.write and rt.exec act on the host, so each commits an entry
-- before its effect and replaces it with the completed entry afterwards. A
-- replay that finds the entry still begun knows the effect may have happened.
ALTER TABLE action_log ADD COLUMN begun INTEGER NOT NULL DEFAULT 0;
