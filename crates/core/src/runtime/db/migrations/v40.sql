-- r[service.site] Reshape site_service_endpoints to a 5-tuple:
--   (site_service, service_port, protocol, remote_host, remote_port)
-- so operators can declare a site-side port distinct from the backend's
-- listening port (classic "service exposes 80, backends listen on 8080"
-- pattern). Traffic routing fans out per (service_port, protocol).
--
-- SQLite doesn't support adding columns with FK-affecting changes in
-- place, so we rebuild: existing rows are copied with
-- service_port = remote_port (the old 4-column shape didn't distinguish
-- them). The feature has no deployed data yet, so this degrades nothing.
CREATE TABLE site_service_endpoints_new (
    site_service TEXT    NOT NULL REFERENCES site_services(name) ON DELETE CASCADE,
    service_port INTEGER NOT NULL,
    protocol     TEXT    NOT NULL,
    remote_host  TEXT    NOT NULL,
    remote_port  INTEGER NOT NULL,
    PRIMARY KEY (site_service, service_port, protocol, remote_host, remote_port)
);

INSERT INTO site_service_endpoints_new
    (site_service, service_port, protocol, remote_host, remote_port)
SELECT site_service, port, protocol, host, port FROM site_service_endpoints;

DROP TABLE site_service_endpoints;
ALTER TABLE site_service_endpoints_new RENAME TO site_service_endpoints;
