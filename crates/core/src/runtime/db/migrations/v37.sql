-- r[image.pin.expiry] Optional per-pin expiration. When set and in the past,
-- the reconciler deletes the pin on its next sweep. Left NULL for pins that
-- should be kept indefinitely.
ALTER TABLE image_pins
    ADD COLUMN expires_at INTEGER NULL;

CREATE INDEX IF NOT EXISTS image_pins_expires_at_idx
    ON image_pins (expires_at);
