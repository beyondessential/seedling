-- l[impl bsl.resource.description]
-- Persist the BSL-level description on dynamic resource records so the
-- operator-facing `apps show` view can render anonymous jobs and other
-- dynamic resources by their declared purpose rather than just by their
-- generated display_name.
ALTER TABLE dynamic_resources ADD COLUMN description TEXT;
