-- r[service.site] Site-level services: operator-managed named services,
-- independent of any app. The name is the stable handle; backing endpoints
-- live in site_service_endpoints (1:N) so operators can rotate backends
-- without recreating the service record.
CREATE TABLE IF NOT EXISTS site_services (
    name        TEXT PRIMARY KEY,
    description TEXT,
    created_at  TEXT NOT NULL
);

-- r[service.site] Endpoints for a site service. The primary key is the
-- full tuple so operators may add the same host under different ports or
-- protocols without collision.
CREATE TABLE IF NOT EXISTS site_service_endpoints (
    site_service TEXT    NOT NULL REFERENCES site_services(name) ON DELETE CASCADE,
    host         TEXT    NOT NULL,
    port         INTEGER NOT NULL,
    protocol     TEXT    NOT NULL,
    PRIMARY KEY (site_service, host, port, protocol)
);

-- r[service.external.mapping.events] Operator-configured mappings from an
-- app's declared external-service slot to a concrete target service (either
-- another app's service or a site service).
CREATE TABLE IF NOT EXISTS external_service_mappings (
    app            TEXT NOT NULL,
    external_name  TEXT NOT NULL,
    target_kind    TEXT NOT NULL,
    target_app     TEXT,
    target_service TEXT NOT NULL,
    PRIMARY KEY (app, external_name)
);
