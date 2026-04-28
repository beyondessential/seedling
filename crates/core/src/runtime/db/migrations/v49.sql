-- r[ingress.site] Operator-managed (or provider-discovered) named entry
-- points that live outside any app. The site_ingresses row carries the
-- hostname and TLS provisioning mode; site_ingress_attachments rows bind
-- (port, protocol) tuples on that hostname to a forward-or-redirect target.
CREATE TABLE site_ingresses (
    name                TEXT    NOT NULL PRIMARY KEY,
    hostname            TEXT    NOT NULL,
    description         TEXT,
    -- 'manual' | 'discovered'
    source              TEXT    NOT NULL,
    -- non-NULL iff source='discovered': the provider identifier
    -- (currently 'tailscale').
    discovered_provider TEXT,
    -- non-NULL iff source='discovered': the stable, opaque key the
    -- provider uses to identify the host (e.g. Tailscale node id).
    -- Surviving hostname renames means we update `hostname` in place
    -- and keep attachments bound.
    discovered_key      TEXT,
    -- 'acme' | 'tailscale' | 'internal' | 'none'
    tls_provider        TEXT    NOT NULL,
    -- Discovery temporarily lost the source (e.g. tailscaled down or
    -- logged out): the row is preserved so attachments survive transient
    -- outages, but the runtime stops trying to issue/serve until cleared.
    stale               INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT    NOT NULL,
    -- One discovered entry per (provider, key); a manual ingress has
    -- both columns NULL and is exempt from this constraint thanks to
    -- SQLite's NULL-distinct unique semantics.
    UNIQUE (discovered_provider, discovered_key)
);

CREATE INDEX site_ingresses_hostname_idx ON site_ingresses(hostname);

-- r[ingress.site.attachment]
CREATE TABLE site_ingress_attachments (
    site_ingress           TEXT    NOT NULL REFERENCES site_ingresses(name) ON DELETE CASCADE,
    port                   INTEGER NOT NULL,
    -- 'tcp' | 'udp' | 'http' | 'http2'
    protocol               TEXT    NOT NULL,
    -- 'forward' | 'redirect'
    target_kind            TEXT    NOT NULL,
    -- forward: target app name and app service name
    target_app             TEXT,
    target_service         TEXT,
    -- redirect: response URL, status code, and whether to keep the
    -- request path on the redirect (1) or send the URL verbatim (0).
    redirect_url           TEXT,
    redirect_code          INTEGER,
    redirect_preserve_path INTEGER,
    created_at             TEXT    NOT NULL,
    PRIMARY KEY (site_ingress, port, protocol)
);
