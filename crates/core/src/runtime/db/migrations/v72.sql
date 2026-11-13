-- The pod network each pod instance is connected to.
--
-- A pod network's /64 within the host prefix is allocated and recorded here
-- rather than derived from the instance's identity, which can only avoid a
-- collision to a probability. The slot is the last byte of the /64's
-- discriminant, unique among pod networks of one resource kind.
CREATE TABLE pod_networks (
    network_name TEXT NOT NULL PRIMARY KEY,
    kind         INTEGER NOT NULL,
    slot         INTEGER NOT NULL,
    UNIQUE (kind, slot)
);
