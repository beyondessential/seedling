# H1 — Deployment priority and app priority

Scenarios that verify the two priority levers: a per-Deployment level declared in
BSL, and a per-app priority set by the operator. Ticked cases are covered by
automated tests; unticked ones are coverage the card still owes, most of them
needing a real systemd host or a browser.

## BSL surface

- [x] `Priority` exposes `Critical`, `Elevated`, `Normal` and `Low` in the script scope (verifies spec: `const.priority.enum`)
- [x] `deployment.priority(level)` accepts each of the four levels (verifies spec: `deployment.priority`)
- [x] A Deployment that declares no priority is `Normal` (verifies spec: `deployment.priority`)
- [x] Calling `.priority()` on a Job is an evaluation error naming the method and the type (verifies spec: `deployment.priority`)
- [x] `.priority()` chains with the other Deployment builders
- [ ] A definition declaring a priority survives an update and re-install without the level being lost

## Ordering

- [x] Kill order is app-major: every workload of a higher-priority app outranks every workload of a lower-priority one (verifies spec: `priority.kill-order`)
- [x] A `Critical` Deployment in a `low` app is shed before a `Normal` Deployment in a `high` app (verifies spec: `priority.kill-order`)
- [x] Within one app, the declared level orders the workloads (verifies spec: `priority.kill-order`)
- [x] Infrastructure outranks every app workload (verifies spec: `priority.kill-order`)
- [x] Every kill preference falls inside the kernel's accepted range (verifies spec: `priority.kill-order`)
- [x] Weights rise with priority and stay inside the supervisor's range (verifies spec: `priority.scheduling`)

## Resource-control groups

- [x] Each declared level gets a tier slice under its app slice (verifies spec: `priority.actuation`)
- [x] The `Normal` tier always exists, for Jobs and other non-Deployment workloads (verifies spec: `priority.actuation`)
- [x] The app slice carries the app weight; the tier slice carries the tier weight (verifies spec: `priority.scheduling`)
- [x] Changing the app priority changes the app-slice weight and leaves the tier weights alone (verifies spec: `priority.settings`)
- [x] A hyphenated app name does not nest its slice inside another app's (verifies spec: `priority.groups-owned`)
- [x] An app cannot be registered under the name reserved for the infrastructure slice (verifies spec: `priority.groups-owned`)
- [ ] On a real host, a started Deployment's unit reports the expected `Slice=` and `OOMScoreAdjust=` (verifies spec: `priority.actuation`)
- [ ] On a real host, changing an app priority reweights the live slice without restarting the workloads (verifies spec: `priority.settings`)
- [ ] On a real host, the proxy and resolver sit in the infrastructure slice (verifies spec: `priority.groups-owned`)
- [ ] A slice whose app is deregistered is removed; a slice is kept when its app merely failed to compute desired state (verifies spec: `reconciliation.absolute-state`)
- [ ] Under real memory pressure the kernel sheds workloads in the specified order (verifies spec: `priority.kill-order`)
- [ ] Under real CPU and I/O contention, capacity divides by app and then by tier; an app alone on an idle host is not throttled (verifies spec: `priority.scheduling`)

## Operator interface

- [x] `/apps/priority` stores the level and reports it back (verifies spec: `app.priority.set`)
- [x] Every accepted level round-trips through `/apps/show` (verifies spec: `app.priority.set`)
- [x] An app that was never set describes as `normal` in both `/apps/show` and `/apps/list` (verifies spec: `app.priority.describe`)
- [x] `/apps/show` carries each Deployment's declared level (verifies spec: `app.priority.describe`)
- [x] An unknown level returns `requirements_invalid` and leaves the stored value untouched (verifies spec: `app.priority.set`)
- [x] An unregistered app returns `not_found` (verifies spec: `app.priority.set`)
- [x] Uninstall discards the stored priority, so a reinstall starts at `normal` (verifies spec: `app.priority.reset-on-uninstall`)
- [x] Storage round-trips, overwrites, and deletes per app (verifies spec: `priority.app`, `priority.settings`)
- [ ] Deregistering an app discards its stored priority (verifies spec: `app.priority.reset-on-uninstall`)
- [ ] A stored value that no longer parses is read as `normal` rather than surfacing a level nobody chose
- [ ] `ctl apps priority <app> <level>` sets the level and prints the result
- [ ] The migration adds the table to an existing database without disturbing the rows already there

## Web interface

- [ ] The app detail page shows the app's priority and lets the operator set any accepted level (verifies spec: `routes.apps.priority`)
- [ ] The apps table shows a chip only for an app whose priority is not `normal` (verifies spec: `routes.apps.priority`)
- [ ] The chip distinguishes a raised priority from a lowered one by colour (verifies spec: `routes.apps.priority`)
- [ ] A priority change is reflected in both the detail page and the apps table without a page reload (verifies spec: `routes.apps.priority`)
- [ ] Each Deployment declaring a level other than `normal` shows an indicator beside its lifecycle state (verifies spec: `routes.apps.priority-indicator`)
- [ ] The Deployment indicator reads as declared configuration rather than a control (verifies spec: `routes.apps.priority-indicator`)
- [ ] The indicator conveys standing within the app, not across apps (verifies spec: `routes.apps.priority-indicator`)
