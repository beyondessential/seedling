CREATE UNIQUE INDEX IF NOT EXISTS idx_singleton_unique
    ON resource_instances (app, kind, name)
    WHERE is_scaled = 0;
