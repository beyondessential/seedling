-- r[impl canopy.app.key]
-- r[impl canopy.app.health]
-- r[impl canopy.offer.absent]
-- The Canopy application type each registered app declares, and what the
-- reporting loop needs to remember across a restart.
--
-- `since` is when the app came to carry its current type, so the missing-offer
-- period can run from it. `last_grade` is the `health/app` result graded before
-- a lifecycle operation began, which is re-sent while the app is operating:
-- Canopy closes a check's issue when the check is skipped.
CREATE TABLE IF NOT EXISTS canopy_apps (
    app        TEXT PRIMARY KEY,
    type       TEXT NOT NULL,
    since      INTEGER NOT NULL,
    last_grade TEXT
);

-- The last moment an offer was live, and the moment Canopy access was last
-- enabled, both unix seconds. NULL when never.
ALTER TABLE canopy_settings ADD COLUMN last_offer_live_at INTEGER;
ALTER TABLE canopy_settings ADD COLUMN enabled_at INTEGER;
