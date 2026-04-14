# Skill: Debugging Seedling Runtime Issues

## Overview

This skill covers investigating runtime problems with seedling — apps stuck
in unexpected states, reconciliation not progressing, missing faults, degraded
status, etc.

## Tools Available

### seedling-ctl (CLI)

The control tool talks to the running server over QUIC. Use the pre-built
binary at `target/debug/seedling-ctl`. It is already authenticated for local
development.

Key commands:

| Command | Purpose |
|---|---|
| `seedling-ctl` | List all available commands |
| `seedling-ctl apps list` | List all apps with their current status |
| `seedling-ctl apps show <app>` | Full detail: resources, instances, lifecycle states, faults, params, scale |
| `seedling-ctl apps logs <app>` | Stream container logs |
| `seedling-ctl apps scale <app> <deployment> <n>` | Adjust deployment scale |
| `seedling-ctl op status` | Instance-level status across all apps |
| `seedling-ctl op faults` | List all active faults |
| `seedling-ctl op faults --app <app>` | Faults scoped to one app |
| `seedling-ctl op events` | Stream the live event feed (JSON) |
| `seedling-ctl op logs` | Stream infrastructure logs |

Run any command with `--help` for full usage.

### Server logs

The server writes logs to `seedling.log` in the current working directory
(not journald — it runs outside systemd in development). Grep for the app
name, instance IDs, or keywords like `error`, `fault`, `reconcil`, `tick`,
`desired`, `unschedul`.

### SQLite database

The persistent state lives at `/opt/seedling/seedling.db` (requires `sudo`
to read). Open it with:

```
sudo sqlite3 -header -column /opt/seedling/seedling.db
```

#### Key tables

| Table | What it stores |
|---|---|
| `registered_apps` | App name, script source, installed/uninstalling flags, current version ID |
| `resource_instances` | Every instance ever created — columns: `id`, `app`, `kind`, `name`, `is_scaled`, `display_name`, `created_at` |
| `world_observations` | Observation log per instance — columns: `instance_id`, `obs_kind`, `payload`, `recorded_at` (epoch ms) |
| `faults` | Active and cleared faults — check `cleared_at IS NULL` for active |
| `scaling_decisions` | Current effective scale per deployment — columns: `app`, `deployment`, `scale`, `updated_at` |
| `params` | App parameter values — columns: `app_name`, `param_name`, `value` |
| `action_log` | Operation/action call history with barrier state |
| `current_operation` | Singleton row for the in-flight operation (empty when idle) |
| `autonomous_operations` | History of autonomous ops (health checks, restarts) |
| `app_versions` | Script version history per app |

#### Useful queries

Recent observations for an instance:

```
SELECT * FROM world_observations
WHERE instance_id = '<hex_id>'
ORDER BY recorded_at DESC LIMIT 20;
```

All instances for an app:

```
SELECT id, kind, name, is_scaled, display_name
FROM resource_instances WHERE app = '<app_name>'
ORDER BY created_at ASC;
```

Active faults for an app:

```
SELECT * FROM faults
WHERE app = '<app_name>' AND cleared_at IS NULL;
```

Current scaling decisions:

```
SELECT * FROM scaling_decisions WHERE app = '<app_name>';
```

Check if an operation is in flight:

```
SELECT * FROM current_operation;
```

### Source code

This skill runs in the context of the seedling source code;
you can check on any detail directly to the source instead of guessing.

## Debugging Workflow

### 1. Establish the symptom

Start with `seedling-ctl apps list` to see which apps are in unexpected
states. Then `seedling-ctl apps show <app>` for the detailed view. Note:

- **Status values**: `running`, `degraded`, `faulted`, `operating`,
  `uninstalling`, `not_installed`
- **Lifecycle states** (per instance): `Pending`, `Scheduled`, `Running`,
  `Ready`, `Terminating`, `Terminated`, `Unscheduled`

Pay attention to which specific instances are not `Ready` and what lifecycle
state they are stuck in.

### 2. Check faults

Run `seedling-ctl op faults --app <app>`. If empty, also query the DB
directly — the CLI and DB should agree but it rules out display bugs.

### 3. Inspect instance observations

The lifecycle state is *derived* from the observation history (see
`src/runtime/barrier/oracle.rs`). Query `world_observations` for the
instance ID shown in `apps show` output. The observation sequence tells you
exactly what happened:

- **Deployments**: `container_created` → `container_running` →
  `health_check_pass` → (ready). Teardown: `stop_sent` →
  `container_exited` → `container_removed` → (unscheduled).
- **Services**: `network_created` → `backend_healthy` → (ready). Teardown:
  `stop_sent` → `network_removed` → `network_cleaned_up`.
- **Ingress**: `ingress_configured` → `ingress_ready` → (ready). Teardown:
  `stop_sent` → `ingress_removed` → `ingress_cleaned_up`.
- **Volumes**: `volume_created` → `volume_ready` → (ready). Teardown:
  `stop_sent` → `volume_removed` → `volume_cleaned_up`.

If observations stopped at an intermediate point, the reconciler may be
failing silently — check logs.

### 4. Understand the desired state

The reconciler computes a desired state per tick and drives instances toward
it. The logic lives in `src/runtime/desired.rs`:

- **Steady state** (`compute_steady`): non-deployment resources get
  singleton instances desired at `Ready`. Deployments use
  `ensure_scaled_group` which splits instances into `keep` (desired
  `Ready`) and `excess` (desired `Unscheduled`).
- **During operations**: desired state comes from `OperationProgress`.
- **Uninstalling**: all instances desired at `Unscheduled`.

If an instance's actual lifecycle matches its desired state, the reconciler
considers it converged and does nothing. This is the most common reason for
"reconciler not doing anything" — the instance *is* converged from the
reconciler's perspective, but some other component (like status display)
disagrees.

### 5. Check the status computation

App status is computed in `src/oi/handler/apps.rs` (`effective_app_status`).
It refines `Running` into `Running` or `Degraded` by checking whether all
resource instances are `Ready`. Cross-reference what this function sees
(all instances from `find_instances_for_group`) against what the reconciler
considers active (the `keep` set from `ensure_scaled_group`). Mismatches
here are a common source of false `Degraded`.

### 6. Check for stuck operations

Query `current_operation` — if a row exists, an operation is in flight and
the reconciler drives operation-mode desired state instead of steady state.
Check `action_log` for the operation ID to see what barriers are pending.
