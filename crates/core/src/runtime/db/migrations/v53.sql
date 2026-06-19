-- g[impl identity]
-- g[impl membership.canonical]
-- g[impl versioning.seq]
-- g[impl versioning.payload-fields]
-- The grove this node belongs to. Single-row table (id is constrained to 1):
-- a node belongs to at most one grove at a time. The currently-applied
-- signed payload is stored verbatim so it can be re-applied (idempotent),
-- re-served to peers, and re-verified against the pinned leader key.
CREATE TABLE grove_membership (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    grove_id            BLOB    NOT NULL,
    role                TEXT    NOT NULL CHECK (role IN ('leader', 'follower')),
    leader_fingerprint  TEXT    NOT NULL,
    current_seq         INTEGER NOT NULL,
    current_payload     BLOB    NOT NULL,
    current_signature   BLOB    NOT NULL,
    joined_at           TEXT    NOT NULL
);

-- g[impl peers.dial]
-- Hint cache for the dial loop and the `grove peers` operator surface.
-- Membership itself is derived from the currently-applied signed payload
-- in grove_membership; this table only caches connection metadata.
CREATE TABLE grove_peers (
    fingerprint         TEXT PRIMARY KEY,
    label               TEXT,
    addresses_json      TEXT NOT NULL,
    last_seen_at        TEXT,
    last_connected_at   TEXT
);

-- g[impl params.kind]
-- g[impl params.set]
-- Denormalised projection of the latest signed payload's grove parameters.
-- The signed payload in grove_membership remains canonical; if the two
-- ever disagree, the payload wins.
CREATE TABLE grove_params (
    name                TEXT PRIMARY KEY,
    kind                TEXT    NOT NULL,
    value               TEXT    NOT NULL,
    version_seq         INTEGER NOT NULL
);

-- g[impl mapping]
-- g[impl mapping.reject-local-set]
-- Local-only bindings between local app params and grove parameter names.
-- Never replicated; each node maintains its own mappings.
CREATE TABLE grove_param_mappings (
    app_name            TEXT NOT NULL,
    app_param_name      TEXT NOT NULL,
    grove_param_name    TEXT NOT NULL,
    PRIMARY KEY (app_name, app_param_name)
);

-- History of all applied signed payloads, for replay and debug. Idempotent
-- apply uses INSERT OR IGNORE on the primary key to no-op a duplicate
-- delivery of the same sequence number.
CREATE TABLE grove_versions (
    seq                 INTEGER PRIMARY KEY,
    payload             BLOB NOT NULL,
    signature           BLOB NOT NULL,
    received_at         TEXT NOT NULL
);
