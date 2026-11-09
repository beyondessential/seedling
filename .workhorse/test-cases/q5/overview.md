# Test cases: volume materialisation and lifecycle

Seven bugs about what a volume is backed by, and when it is created, written
and removed. Most are covered by automated tests written alongside the fix.
The tmpfs cases are not: `TMPFS_VOLUMES_DIR` is a fixed absolute path under
`/run`, so nothing below the daemon can point it at a temporary directory.
Those need a real host and are listed here for manual verification.

## Named tmpfs volumes

Unticked because they need a host with the daemon running; nothing here is
reachable from a unit test as the path stands.

- [ ] A named tmpfs volume declaring a `writes` entry has that file inside the
      container, and the file appears nowhere under podman's storage directory.
      Verifies spec: `actuate.volume.tmpfs`.
- [ ] No podman volume is created for a named tmpfs volume.
      Verifies spec: `actuate.volume.tmpfs`.
- [ ] Removing a named tmpfs volume from an app definition removes
      `/run/seedling/tmpfs-volumes/<app>-volume-<name>` and everything in it.
      Verifies spec: `actuate.volume.tmpfs`.
- [ ] Re-adding a volume of the same name within the same boot gives a container
      that sees only the declared writes, with no file from the previous tenant.
      Verifies spec: `actuate.volume.tmpfs`.
- [ ] Upgrading a daemon that had stood a podman tmpfs-driver volume up beside
      the directory: stopping the volume reaps that podman volume too.
- [ ] A declared tmpfs write is re-applied after a host reboot.
      Verifies spec: `actuate.volume.tmpfs`.
- [ ] With a symlink standing at `/run/seedling/tmpfs-volumes/<app>-volume-<name>`,
      starting the volume is refused with an error naming the path, and nothing
      is written through the link. Stopping it refuses likewise and leaves the
      link in place.
      Verifies spec: `storage.confinement`.

## Declared writes and the pods/volumes race

- [x] Creating a named volume seeds it with its declared writes in the same step.
      Verifies spec: `actuate.volume.start`.
- [x] A volume already on disk is not seeded again, so an app's own edits to a
      declared file survive. Verifies spec: `actuate.volume.start`.
- [x] A declared write that fails takes the storage the same call created back
      out, so the next attempt seeds afresh. Verifies spec: `actuate.volume.start`.
- [ ] An app whose Deployment mounts a named volume with declared writes,
      installed with its image already local so both phases actuate on the same
      tick, comes up with the declared file present. Run repeatedly: the bug
      depended on which phase won. Verifies spec: `actuate.volume.start`.

## Site volume names

- [x] Snapshotting onto a name already taken is refused before any btrfs call.
      Verifies spec: `volume.site.name-taken`.
- [x] Promoting onto a name already taken is refused.
      Verifies spec: `volume.site.name-taken`.
- [x] Creating a site volume under a name already taken is refused.
      Verifies spec: `volume.site.name-taken`.
- [x] Storage standing at the name with no record is refused rather than reused,
      and is left where it is. Verifies spec: `volume.site.name-taken`.
- [ ] On a BTRFS host, a refused snapshot leaves no subvolume nested inside the
      target volume. The nesting itself needs real btrfs to reproduce.
      Verifies spec: `volume.site.name-taken`.

## Holding and deleting

- [x] Deleting a managed site volume whose storage is gone removes the record and
      records no held volume. Verifies spec: `actuate.volume.hold`.
- [x] Deleting a managed site volume that cannot be held leaves the record and the
      data alone. Verifies spec: `actuate.volume.hold`.
- [x] Migrating a legacy volume path onto a canonical path that is a regular file
      is refused with an error naming the path, both paths left intact.
      Verifies spec: `storage.confinement`.
- [x] Migrating onto an empty canonical path replaces it; onto one holding data,
      that data is held first. Verifies spec: `actuate.volume.hold`.

## Deregistration

- [x] Deregistering an app discards the external volume and service mappings made
      for it. Verifies spec: `deregister.purge`.
- [x] Deleting one app's mappings leaves other apps' mappings alone.
      Verifies spec: `deregister.purge`.
- [ ] Upgrading a daemon whose database carries mappings for apps already
      deregistered: those rows are gone after the migration, and mappings for
      registered apps are untouched. Verifies spec: `deregister.purge`.
