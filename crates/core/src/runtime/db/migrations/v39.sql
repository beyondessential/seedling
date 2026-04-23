-- r[service.external.mapping.events] SQLite doesn't support conditional
-- foreign keys directly, but external_service_mappings.target_service is a
-- polymorphic column: a site-service name when target_kind = 'site', or an
-- app-service name (qualified by target_app) when target_kind = 'app'.
-- These triggers add RESTRICT semantics for the site-target case so that
-- (a) a mapping can't be created or retargeted onto a non-existent site
-- service, and (b) a site service can't be dropped while any mapping still
-- points at it.
--
-- App-level code in the OI handler refuses the same deletions with a
-- friendlier error message; these triggers are defence in depth against
-- direct SQL or code paths that bypass the handler.

CREATE TRIGGER IF NOT EXISTS ext_svc_map_site_target_ins
BEFORE INSERT ON external_service_mappings
WHEN NEW.target_kind = 'site'
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (SELECT 1 FROM site_services WHERE name = NEW.target_service)
        THEN RAISE(ABORT, 'external_service_mappings: target site service does not exist')
    END;
END;

CREATE TRIGGER IF NOT EXISTS ext_svc_map_site_target_upd
BEFORE UPDATE OF target_kind, target_service ON external_service_mappings
WHEN NEW.target_kind = 'site'
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (SELECT 1 FROM site_services WHERE name = NEW.target_service)
        THEN RAISE(ABORT, 'external_service_mappings: target site service does not exist')
    END;
END;

CREATE TRIGGER IF NOT EXISTS site_services_restrict_if_mapped
BEFORE DELETE ON site_services
BEGIN
    SELECT CASE
        WHEN EXISTS (
            SELECT 1 FROM external_service_mappings
            WHERE target_kind = 'site' AND target_service = OLD.name
        )
        THEN RAISE(ABORT, 'site_services: external_service_mappings still target this site service')
    END;
END;
