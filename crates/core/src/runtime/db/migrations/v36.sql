-- r[image.track] Reference → image_id mapping, refreshed by the reconciler
-- from the container runtime's image list on every tick. The barrier driving
-- `rt.warm_images(...).ready()` consults this table to decide whether each
-- pinned reference is locally present without requiring the oracle to make
-- async podman calls.
CREATE TABLE IF NOT EXISTS image_references (
    reference    TEXT PRIMARY KEY,
    image_id     TEXT NOT NULL,
    observed_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS image_references_image_id_idx
    ON image_references (image_id);
