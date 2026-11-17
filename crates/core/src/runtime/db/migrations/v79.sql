-- The address each service is reached at on the host.
--
-- An address derived from an instance's identity is unique only to a
-- probability, so the suffix after the host prefix is allocated and recorded
-- here, with a unique index to stand behind it. It goes with its instance.
CREATE TABLE service_addresses (
    instance_id TEXT NOT NULL PRIMARY KEY REFERENCES resource_instances (id) ON DELETE CASCADE,
    suffix      TEXT NOT NULL UNIQUE
);
