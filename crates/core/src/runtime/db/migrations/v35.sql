-- r[image.pin] Durable pin table for rt.warm_images. A pin lives until either
-- a running container using the referenced image is observed (evicted by
-- the reconciler) or an operator clears it via /images/pins/clear. The
-- reference string is stored verbatim as written by the BSL; resolution to
-- an image_id happens at reconcile time via the container runtime.
CREATE TABLE IF NOT EXISTS image_pins (
    app         TEXT NOT NULL,
    reference   TEXT NOT NULL,
    pinned_at   INTEGER NOT NULL,
    PRIMARY KEY (app, reference)
);

CREATE INDEX IF NOT EXISTS image_pins_reference_idx
    ON image_pins (reference);

-- r[image.track] Per-image last-used timestamp, updated by the reconciler
-- when a running container is observed using the image. Seeds to the first
-- time the image is seen locally; drives the 30-day autonomous GC rule.
CREATE TABLE IF NOT EXISTS image_tracking (
    image_id        TEXT PRIMARY KEY,
    first_seen_at   INTEGER NOT NULL,
    last_used_at    INTEGER NOT NULL
);
