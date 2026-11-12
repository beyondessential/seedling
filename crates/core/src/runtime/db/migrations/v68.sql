-- Record the order in which apps completed their installs.
--
-- When ingresses of two apps claim the same hostname and port, the app whose
-- install completed first is served. `install_seq` rises by one with each
-- completed install, so it orders installs exactly where a timestamp could
-- tie. Apps already installed are ordered by name, there being no record of
-- which finished first.
ALTER TABLE registered_apps ADD COLUMN install_seq INTEGER;

UPDATE registered_apps
   SET install_seq = (
       SELECT COUNT(*) FROM registered_apps AS earlier
        WHERE (earlier.installed = 1 OR earlier.uninstalling = 1)
          AND earlier.name <= registered_apps.name
   )
 WHERE installed = 1 OR uninstalling = 1;
