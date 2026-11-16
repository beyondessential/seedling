-- A cached Canopy server identifier is good only for the key the offering
-- client it was resolved through authenticated with. One cached before the
-- key was recorded cannot be tied to a key, so it is dropped to be resolved again.
ALTER TABLE canopy_settings ADD COLUMN server_id_key TEXT;
UPDATE canopy_settings SET server_id = NULL;
