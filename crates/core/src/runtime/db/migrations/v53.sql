-- r[impl canopy.settings.enabled]
-- r[impl canopy.report.identity]
-- Canopy access settings and the identity Canopy knows this instance by.
-- One row, enforced via the singleton primary key.
--
-- `enabled` defaults to on. On a host with no client offering to carry Canopy
-- requests, nothing is registered and nothing runs, so defaulting to on costs
-- an operator nothing and saves one on a packaged host a configuration step.
--
-- `server_id` caches what Canopy answered when asked which server the offering
-- client's identity is enrolled as, so that resolution is not repeated on every
-- report. NULL until first resolved.
CREATE TABLE IF NOT EXISTS canopy_settings (
    singleton  INTEGER PRIMARY KEY DEFAULT 1 CHECK (singleton = 1),
    enabled    INTEGER NOT NULL DEFAULT 1,
    server_id  TEXT,
    updated_at INTEGER NOT NULL DEFAULT 0
);

INSERT OR IGNORE INTO canopy_settings (singleton, enabled, server_id, updated_at)
    VALUES (1, 1, NULL, 0);
