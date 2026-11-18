-- Installing an app no longer runs a closure, so an app is never part-way
-- through an install: one that was becomes installed. Requests queued for the
-- install and start actions, which no longer exist, are dropped with it.
UPDATE registered_apps SET installed = 1 WHERE installing = 1;
ALTER TABLE registered_apps DROP COLUMN installing;

DELETE FROM queued_operations
WHERE trigger IN ('boot', 'install')
   OR action IN ('install', 'start');
