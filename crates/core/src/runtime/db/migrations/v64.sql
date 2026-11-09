-- r[impl deregister.purge]
-- Drop external mappings left behind by apps deregistered before
-- deregistration purged them.
--
-- Neither table has ever had a foreign key to registered_apps, and nothing
-- removed their rows when an app went, so a mapping made for an app could
-- outlive it indefinitely. The next app registered under that name picked up
-- those rows as its own: an external volume mapping carries a target volume
-- and a read-only flag, so the new app came up mounting data an operator had
-- granted to a different app entirely, having declared only that it wanted an
-- external volume by that name.
--
-- A mapping whose app is not registered has no app to belong to, so there is
-- nothing an operator could want it for.
DELETE FROM external_volume_mappings
 WHERE app NOT IN (SELECT name FROM registered_apps);

DELETE FROM external_service_mappings
 WHERE app NOT IN (SELECT name FROM registered_apps);
