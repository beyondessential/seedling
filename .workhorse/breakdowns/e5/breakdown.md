---
base: default
---

# Integrate Canopy

Seedling adopts Canopy for certificates, backups, DNS names and workload identity tokens, with every call going through the existing relay. The groundwork card lands first; the strands after it are independent of each other.

## Report apps to Canopy as applications · H5

An app definition declares its Canopy application type in BSL, and Seedling reports each typed app to Canopy as an application whose health is the app's status, with its faults as detail. The first app to claim a type holds it; a later app declaring the same type gets a non-critical fault and no Canopy features, and takes the type over when the holder is removed. Also covers the fault filed when an app relies on Canopy and no offer has been live past a grace period. Every Canopy card here depends on this one.

## Issue certificates through Canopy · J5

For a hostname with no operator policy, Seedling requests its certificate from Canopy when an offer is live and the entitlements allow the hostname's app to have it, with HTTP-01 as the fallback. Seedling holds the key and submits a CSR; Canopy renews and Seedling collects, replacing the key when Canopy says it must. Adds a Canopy-only policy that drops the HTTP-01 fallback, for hostnames Seedling does not yet receive traffic for. Canopy certificates appear in the hostname view and satisfy warm-cert readiness like other runtime-managed certificates.

## Remove the backup-app framework · G5

Remove backup apps, backup strategies, Seedling's own backup scheduling and the `backup_failed` path across the runtime, OI, ctl and web interface, with a new migration dropping their tables. Operation-scoped volume bindings and the reserved `_volume` and `_filename` param keys stay. Independent of the Canopy work and can land at any point.

## Back up apps through Canopy

An app declares its backups in BSL: each names a Canopy backup type, the volumes it covers, and optional closures that quiesce and resume the app around the snapshot. Seedling registers those types as its own, runs kopia against the target and refreshed credentials Canopy hands out, and reports progress and outcome. A `backup_now` arriving during a lifecycle operation waits for it to finish, up to a deadline. A type refused because another source owns it faults the app. Needs Canopy card Q5, the change that records an owning source per backup type and routes `backup_now` and credentials to it.

## Restore apps from Canopy backups

An operator restores an app's backup type from a chosen snapshot through ctl, the OI or the web interface, within the restore window opened in Canopy. Seedling stops the app, swaps the data in and keeps the previous data aside. Follows the backups card.

## Publish DNS names through Canopy

The operator sets the host's public addresses and turns registration on. Seedling then registers every TLS ingress hostname Canopy allows the app to publish with those addresses, and withdraws names that are no longer declared. Includes a helper that detects the public IP and suggests it for the operator to confirm, across ctl, the OI and the web interface.

## Serve identity tokens to app containers

Seedling serves a token endpoint at a fixed IPv6 address with a hostname injected into every container, in the manner of a cloud metadata service. It identifies the calling app by the pod the request comes from and mints a fresh Canopy identity token through the relay as that app's application, for any audience; Canopy's grants are the only gate.
