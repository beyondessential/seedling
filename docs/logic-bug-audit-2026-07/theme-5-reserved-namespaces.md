# Theme 5: Name/prefix matching without reserved namespaces

> Companion to the [logic bug audit](../logic-bug-audit-2026-07.md), cross-cutting theme 5.

## The failure pattern

Seedling grants itself identifiers inside namespaces it does not own exclusively, then recognises "its" objects later by string shape (or, in one case, by 8 bits of an ID). The four findings split into two sub-classes.

**(a) Prefix matching where exact identity exists.**

`run_uninstall_phase` (`crates/core/src/system/reconcile.rs:1448`) recognises an app's units with `list_units("seedling-{app}-")`, a plain `starts_with` filter (`crates/core/src/system/systemd.rs:410`). Unit names are `seedling-{display_name}.service`, where `display_name` is `{app}-{name}[-{suffix}]` or `{app}-{kind_slug}[-{name}]` (`crates/core/src/runtime/identity.rs`). Both `AppName` and resource names may contain hyphens, so the encoding is not prefix-free: `seedling-app-` matches every unit of app `app-db`, and the retry branch then `reset_failed_unit`s and `stop_unit`s a healthy sibling's units every five-second tick while the uninstall never completes.

The irony is that the exact identities are already recorded in two places:

- `resource_instances` rows — deleted only at uninstall *completion* (`reconcile.rs:1461-1464`), so they are available for the entire window in which the prefix match runs — hold every `display_name` the app ever actuated.
- Podman containers carry `seedling.app` and `seedling.instance` labels (`crates/core/src/system/translate/container.rs:345-347`), and the stray-shell sweep already reads a `seedling.display-name` label (`reconcile.rs:1136-1141`).

The prefix match re-derives, lossily, what the registry already knows losslessly.

**(b) Shared namespaces with no reservation.**

- The daemon claims `backup-snap-*` in the site-volume namespace (`SNAPSHOT_NAME_PREFIX`, `crates/core/src/runtime/backup_execution.rs:14`) and deletes everything under it at startup (`crates/daemon/src/main.rs:616`, via `list_sites_with_prefix` at `crates/core/src/system/volume_store.rs:330`) — without cross-checking the `site_volumes` table. But `create_site_volume` and `restore_held` (`crates/core/src/oi/handler/volumes.rs:260`, `volumes.rs:109-140`) accept any valid `SiteVolumeName`, including `backup-snap-archive`.
- The tailscale provider claims the name `tailscale` in the site-ingress namespace (`TAILSCALE_INGRESS_NAME`, `crates/core/src/runtime/tailscale.rs:30`), and `mark_existing_stale` (`tailscale.rs:287`) disables whatever row holds that name without checking `source.is_discovered()`. `create_site_ingress` (`crates/core/src/oi/handler/ingresses.rs:200`) happily creates a manual ingress under that name; `upsert_discovered_row` (`tailscale.rs:354-403`) then livelocks on the PK, so the operator's row stays stale forever.
- The pod /64 prefix is the same disease in a numeric namespace: `pod_network_prefix` (`crates/core/src/system/translate/proxy.rs:82`) "claims" a /64 by deriving bytes 6–7 of the address from `(kind, uuid[0])` — 8 bits of instance entropy per kind, and *zero* for static Jobs, whose `InstanceId` is nil (`identity.rs:86`). Derivation is treated as if it were allocation; nothing guarantees the claimed subnet is unclaimed, and netavark rejects the second network on a duplicate subnet.

Both sub-classes are one discipline violated at two different points: **an identity must be granted once, recorded, and every later match made against the record — never inferred from the shape of a name or the bits of an ID at the point of use.** Sub-class (a) violates it at match time; sub-class (b) violates it at grant time.

## Affected findings

| Finding | Section | Severity |
|---|---|---|
| Uninstall unit-prefix match stops sibling apps whose names extend the uninstalling app's name (H2) | [§12](../logic-bug-audit-2026-07.md#12-system-reconciliation-engine) | high |
| Operator site volumes named `backup-snap-*` are destroyed by startup cleanup | [§6](../logic-bug-audit-2026-07.md#6-backups-volumes-scheduling) | medium |
| `mark_existing_stale` / upsert have no ownership check, so a manual ingress named "tailscale" is permanently disabled | [§11](../logic-bug-audit-2026-07.md#11-runtime-site-networking-site-services-ingresses-attachments-external-mappings-tailscale) | medium |
| Pod /64 network prefixes collide: all static Jobs share one prefix; scaled replicas birthday-collide on one UUID byte (H12) | [§14](../logic-bug-audit-2026-07.md#14-system-networking-caddy-data-plane-translate-resolver-jool-nat64-netinfo) | high |

Near neighbour, same namespace but a different defect: "Snapshot/promote to an existing site-volume name nests a subvolume inside a live volume" ([§6](../logic-bug-audit-2026-07.md#6-backups-volumes-scheduling)) is a missing existence check at grant time rather than a missing reservation; the creation-time validation proposed below is the natural place to add it.

## Would a high-level change help?

**Sub-class (a): yes, decisively.** The uninstall bug is not fixable by a cleverer prefix — with app names and resource names sharing the `-` alphabet, `seedling-{app}-` can never be prefix-free without changing the encoding, and the encoding is frozen (see Migration path). But no prefix is needed: at every point where the reconciler asks "which units belong to app X", the registry can answer exactly. Matching against enumerated identities eliminates the entire bug class rather than shrinking it, and it composes with orphan handling — a sweep over *all* `seedling-*` units reconciled against the union of all apps' recorded instances is still exact per app.

**Sub-class (b): yes, but it needs both halves.**

- Creation-time reservation (reject `backup-snap-*` volumes and a manual ingress named `tailscale`) stops new collisions, but cannot repair pre-existing ones and does not protect against future code paths that forget the check.
- So the destructive consumers must *also* match on recorded identity: the startup sweep must skip DB-registered `site_volumes` rows, and `mark_existing_stale` / `upsert_discovered_row` must key on `source.is_discovered()` rather than the name. Reservation makes collisions impossible going forward; ownership checks make them harmless regardless of history.
- For the pod /64, "reservation" means actual allocation. Sixteen bits (bytes 6–7 of a /64 under the node /48) is too small a space for hash-derivation to masquerade as allocation at fleet scale — even a perfect 16-bit hash over the full identity gives roughly 7% collision odds at 100 concurrent instances. Uniqueness has to come from an allocator with a uniqueness guarantee, not from entropy.

One discipline covers both sub-classes: names and numbers the system grants itself go through a single authority that records the grant, and consumers match against the record. That is worth adopting as a rule, not just as four point fixes.

## Proposed pattern

**1. Exact-identity matching for uninstall.** `run_uninstall_phase` builds the expected unit set from the registry instead of a prefix:

```rust
// resource_instances rows survive until uninstall completes (reconcile.rs:1461)
let expected: HashSet<String> = instances_for_app(db, &app.name)
    .map(|dn| format!("seedling-{dn}.service"))
    .collect();
let leftover: Vec<_> = self.driver.process.list_units("seedling-").await?
    .into_iter()
    .filter(|u| expected.contains(&u.name))
    .collect();
```

Completion is `leftover.is_empty()`; the retry branch stops only `leftover`. `app-db`'s units can never appear, because its display names live in different `resource_instances` rows. On the podman side the same rule already has infrastructure: container listing should filter by `seedling.app` label equality rather than name prefix (`podman.rs:346`, `podman.rs:450`). Systemd offers no queryable custom unit property via `ListUnits` (the `SEEDLING_APP` field at `crates/core/src/system/actuator/pod.rs:361` is journal metadata only), which is exactly why the registry, not the unit name, must be the source of truth for units.

**2. A single reserved-names module.** One home for every name Seedling grants itself in an operator-shared namespace — either a new `crates/core/src/reserved.rs` or an extension of `crates/core/src/sysconst.rs`, which already plays the "single point of truth for system-wide facts" role:

```rust
pub const RESERVED_SITE_VOLUME_PREFIXES: &[&str] = &[SNAPSHOT_NAME_PREFIX]; // "backup-snap-"
pub const RESERVED_SITE_INGRESS_NAMES: &[&str] = &[TAILSCALE_INGRESS_NAME]; // "tailscale"

pub fn check_site_volume_name(name: &SiteVolumeName) -> Result<(), ReservedName> { ... }
pub fn check_site_ingress_name(name: &SiteIngressName) -> Result<(), ReservedName> { ... }
```

Creation-time validation consults it in exactly four places, each rejecting with `requirements_invalid`:

- `create_site_volume` (`volumes.rs:260`);
- `restore_held`, on its `target_name` path (`volumes.rs:109-120`);
- the snapshot/promote handlers (`volumes.rs:494-633`) — which should also gain the target-existence pre-check that `restore_held` already demonstrates at `volumes.rs:123-140`;
- `create_site_ingress` (`ingresses.rs:200`).

Scattering the constants is what allowed the gap: today `backup_execution.rs:14` and `tailscale.rs:30` know nothing about each other or about the creation handlers.

**3. Ownership checks at the destructive consumers.** The startup snapshot sweep skips any name present in `site_volumes` — the DB row *is* the record that an operator owns it. `mark_existing_stale` and `upsert_discovered_row` operate only on rows whose `source` is `Discovered { provider: Tailscale, .. }`, never on a name match alone.

**4. Allocated pod subnet IDs.** Replace the `(kind, uuid[0])` derivation in `pod_network_prefix` with a 16-bit subnet ID allocated per instance from a small DB table — `instance_id → u16`, a unique index on the u16, allocated at first actuation, freed when the instance is garbage-collected or the app uninstalled. Static Jobs key naturally by their deterministic identity, so each distinct Job gets a distinct subnet without needing per-run state. `instance_ipv6`'s interface-ID bytes (`uuid[1..9]`, `proxy.rs:73`) stay as they are — they only need uniqueness within the pod's own /64. If a DB round-trip inside `translate` is unpalatable, the minimum acceptable fallback is a 16-bit hash over the *full* identity (`app`, `kind`, `name`, whole UUID — not `uuid[0]`) plus collision detection against live networks at creation time; but allocation is the version that actually upholds the discipline, and it is the same table-plus-unique-index shape as the name reservations.

**Not proposed: re-encoding unit names.** A prefix-free encoding (a delimiter outside the `[a-z0-9-]` name alphabet — systemd permits `.` and `_`, so `seedling.{app}.{rest}.service` would work) would fix sub-class (a) in principle, but `display_name` is deliberately stored stably in `resource_instances` ("never changes even if the derivation logic does", `identity.rs:53`), and both actuator and observer derive unit names from it at runtime (`observer.rs:53`, `actuator/pod.rs:28`) — changing the format orphans every running unit at the next daemon restart. Exact matching gets the same guarantee with zero renames.

## What it prevents — and what it does not

**Prevents:**

- Every cross-app unit action — stop/reset of a sibling's units, never-completing uninstalls — for any present or future hyphenated app pair, with no constraint on operator naming.
- Destruction of operator volumes by the snapshot sweep, both for reserved-prefix names (rejected at creation) and for any legacy or out-of-band name (DB ownership check).
- The permanent disablement of a manual `tailscale` ingress and the PK-collision livelock in `upsert_discovered_row`.
- Deterministic static-Job subnet collisions and probabilistic replica collisions, at any scale the /48 can express (65,536 concurrent pods).

**Does not prevent:**

- Collisions with objects Seedling never recorded: a unit or podman network some other tool names `seedling-…` will still be swept by orphan cleanup, because "not in the registry" is indistinguishable from "orphaned". That residual risk is inherent to sharing host namespaces and is worth a line in the operator docs instead.
- The *other* tailscale finding — never marking the discovered ingress stale on `Unreachable` ([§11](../logic-bug-audit-2026-07.md#11-runtime-site-networking-site-services-ingresses-attachments-external-mappings-tailscale)) — which is missing behaviour, not misattributed identity.
- Name *reuse over time*: external service/volume mappings inherited by a later app of the same name ([§11](../logic-bug-audit-2026-07.md#11-runtime-site-networking-site-services-ingresses-attachments-external-mappings-tailscale)) are a lifecycle-cleanup bug. Reservation scopes namespaces in space, not in time.

## Migration path

Deployed state constrains every fix here; the proposals were chosen to need no renames.

- **Unit names are frozen.** Existing units on every deployed node are `seedling-{display_name}.service`, and `display_name` values are persisted per instance. The exact-match fix reads names as they are; nothing is renamed, so there is no restart churn and no window where the observer loses sight of running containers. Deploying it mid-uninstall is safe: the registry rows the match needs are exactly the ones the current code deletes last.
- **`backup-snap-*` operator volumes may already exist.** Creation-time rejection applies only to new names, so the sweep's DB cross-check is not optional hardening — it is the only protection for a pre-existing `backup-snap-archive`. Ship the sweep check in the same release as (or before) the creation check. Do not attempt to rename legacy volumes: on-disk `site-{name}` paths are mounted into running containers.
- **A manual ingress named `tailscale` may already exist.** The ownership check makes it permanently safe. The reservation check must apply to *create* only — never to update or delete paths — or the operator loses the ability to manage the legacy row out of existence.
- **Pod subnets: seed the allocator from live state.** On first startup after the change, enumerate existing `seedling-*` podman networks and reserve their current subnets' 16-bit IDs for the owning instances (the network name embeds `display_name`, so ownership is recoverable). New allocations then avoid live legacy subnets; each instance moves to its allocated /64 naturally at its next pod recreation, when its per-instance network is torn down and remade. No flag day, no forced restarts.

## Enforcement

- **Unit tests.**
  - Prefix-freedom regression: install stub apps `app` and `app-db`, run uninstall of `app` against the stub `System` (`stub.rs` already mirrors `list_units`' prefix semantics at `stub.rs:317`), assert `app-db`'s units receive no `stop_unit`/`reset_failed_unit` and the uninstall completes — the report's own suggested test for H2.
  - Reserved-name rejection via TestOi: `create_site_volume`, `restore_held`, and `snapshot`/`promote` with `backup-snap-x`, and `create_site_ingress` with `tailscale`, all return `requirements_invalid`.
  - Ownership: `Db::open_in_memory()`, create a manual `tailscale` ingress, drive the provider's poll handling, assert `stale` is untouched and no delete occurs.
  - Subnet allocation: two static Jobs and two instances sharing `uuid[0]` receive distinct /64s; allocator exhaustion errors cleanly; free-on-GC round-trips.
- **Tracey spec items.** These are interface contracts external actors depend on, so they belong in `docs/spec` (phrased as *what*, not *how*): site-volume names beginning `backup-snap-` and the site-ingress name `tailscale` are reserved and rejected at creation; uninstalling an app affects only resources belonging to that app; every concurrently running pod instance has a distinct /64. Annotate the reserved-names module and the four fixes with `r[impl ...]` / `r[verify ...]` so `tracey query uncovered` keeps the reservation list and its consumers from drifting apart.
- **Review checklist.** Any `starts_with` (or SQL `LIKE 'x%'`) on a runtime identity — unit, container, network, volume, or ingress name — is a red flag. Today's inventory: `systemd.rs:410`, `podman.rs:346`, `podman.rs:450`, `volume_store.rs:343`, and their `stub.rs` mirrors. A prefix scan is acceptable only for enumerating candidates that are then matched exactly against recorded identity. Second flag: any new constant name or prefix the daemon grants itself that is not added to the reserved-names module in the same change.
