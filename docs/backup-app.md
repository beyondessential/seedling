# Writing a Backup App

A backup app is an ordinary Seedling application that has been _registered_ as a backup provider. Seedling treats it specially: it invokes three well-defined actions to save, list, and restore snapshots of other apps' volumes.

Before reading this, you should be familiar with [BSL scripting](bsl-scripting.md).

## What Seedling provides

When Seedling invokes a backup action, it injects _operation-scoped volume bindings_ into the backup app for the duration of that invocation. These bindings are accessed via `app.external_volume(name)` inside the action closure, exactly like any other external volume — but they only exist for the duration of that one invocation, and the name is chosen by Seedling, not by your script.

Seedling delivers the chosen name to your action by way of a reserved `<key>_volume` param. So for every logical binding documented per action below — `source`, `output`, `destination` — your closure looks up the volume like this:

```rhai
let source = app.external_volume(param["source_volume"]);
```

Do **not** hard-code a name like `app.external_volume("source")` — it won't match. Seedling generates a distinct name per invocation so that concurrent operations cannot collide with each other, and so the operator cannot cause collisions by configuring a static external volume with the same name. Param keys ending in `_volume` (and `_filename`, see below) are reserved — the operator-facing action invocation endpoints reject any attempt to pass them in.

For some actions, Seedling also chooses the _filename_ the action must write under the bound volume, and delivers it via a companion `<key>_filename` param. Use `param["<key>_filename"]` rather than hard-coding a filename; the runtime reserves the right to change what filename it expects.

Seedling also passes every backup action a structured **backup object** in its `param` map, under the `backup` key. It describes the identity of the backup operation:

```
param["backup"] = #{
    strategy: "daily-kopia",   // strategy that caused this invocation
    app:      "myapp",         // BSL app whose volume is being backed up,
                               //   or "_site" for site-scoped volumes
    volume:   "data",          // named volume within `app`
}
```

Backup apps use the object for two things that make multi-volume backups work correctly:

1. **save-snapshot** stamps `app` + `volume` (and optionally `strategy`) onto the snapshot, via the backend's tag / hostname / prefix mechanism. Without this, there's no way to attribute a snapshot to a source volume after the fact.
2. **list-snapshots** filters its output to only snapshots matching the `app` + `volume` pair. Without filtering, an operator listing snapshots for `myapp/data` would see snapshots of `otherapp/logs` too if both are stored in the same backend repository — `restore-snapshot` takes the snapshot id as an opaque string, so a wrong pick would silently restore the wrong data into the wrong target.

`restore-snapshot` receives the same `backup` object plus an opaque `snapshot` string naming the specific snapshot to restore.

## Required actions

A backup app must define exactly these three actions using `app.on_action()`. The names must match exactly.

### `save-snapshot`

Called by Seedling when a scheduled or manually triggered backup runs for a volume.

**Bindings:**

| Logical key | Access | Contents |
|---|---|---|
| `source` | read-only | A point-in-time snapshot of the volume being backed up |

**Params:**

| Key | Type | Description |
|---|---|---|
| `source_volume` | string | Generated name for the `source` binding; look up via `app.external_volume(param["source_volume"])` |
| `backup` | object | The [backup object](#what-seedling-provides) identifying this backup operation |

**Contract:** read from the `source` binding and persist the data to whatever external backend the backup app uses (object storage, a remote server, etc.). Stamp `backup.app` + `backup.volume` onto the snapshot so that [`list-snapshots`](#list-snapshots) can filter by volume later. Exit successfully on success; Seedling will retry up to twice on failure and file a `backup_failed` fault if all attempts fail.

```rhai
app.on_action("save-snapshot", |rt, param| {
    let source = app.external_volume(param["source_volume"]);
    let backup = param["backup"];
    let job = app.job()
        .image("ghcr.io/example/backup-tool:latest")
        .mount("/source", source)
        .env("DEST_BUCKET", bucket.value())
        // Stamp app+volume so list-snapshots can filter by them.
        .command(["backup-tool", "upload",
                  "--app", backup["app"],
                  "--volume", backup["volume"],
                  "/source"]);
    rt.start(job).terminated().ensure_success();
}, #{ description: "Upload a snapshot to object storage" });
```

### `list-snapshots`

Called synchronously when an operator requests a list of available snapshots for a given volume and strategy.

**Bindings:**

| Logical key | Access | Contents |
|---|---|---|
| `output` | read-write | Empty directory; the action must write the output file named by `param["output_filename"]` here |

**Params:**

| Key | Type | Description |
|---|---|---|
| `output_volume` | string | Generated name for the `output` binding; look up via `app.external_volume(param["output_volume"])` |
| `output_filename` | string | Filename (relative to the output volume) the action must write the snapshot list to |
| `backup` | object | The [backup object](#what-seedling-provides) identifying which volume's snapshots to list |

**Contract:** write a file inside the `output` binding at the filename given by `param["output_filename"]` containing a valid JSON array of objects describing the available snapshots. Each object must have an `id` key which will be used for restoring. Do **not** hard-code the filename — Seedling picks it, and may change it in future versions.

The output **must be filtered to only include snapshots that were taken via this app's `save-snapshot` action for the same `backup.app` + `backup.volume`**. A single backup app commonly stores multiple volumes' snapshots in the same remote backend (one Kopia repository, one S3 bucket with prefix-per-volume, etc.), and the operator requesting a list for `myapp/data` would be dangerously confused by snapshots of `otherapp/logs` showing up in the same list — the restore step takes the snapshot identifier as an opaque string, so nothing prevents an operator from accidentally selecting a snapshot that belongs to a different volume and writing it over the wrong target.

How you implement the filter depends on your backend:

- **Kopia**: tag snapshots on save (`--tags app:<app> --tags volume:<volume>`), filter on list (`--tags app:<app> --tags volume:<volume>`).
- **restic**: tag snapshots on save (`--tag app=<app> --tag volume=<volume>`), filter on list (`--tag app=<app> --tag volume=<volume>`).
- **Object-storage roll-your-own**: prefix keys with `<app>/<volume>/` on save; list under that prefix on retrieve.

The `snapshot` identifier used in `restore-snapshot` must be derivable from the data in this file. A common convention is an `id` or `name` field on each object.

```rhai
app.on_action("list-snapshots", |rt, param| {
    let output = app.external_volume(param["output_volume"]);
    let output_file = param["output_filename"];
    let backup = param["backup"];
    let job = app.job()
        .image("ghcr.io/example/backup-tool:latest")
        .mount("/output", output)
        .env("DEST_BUCKET", bucket.value())
        .command(["backup-tool", "list",
                  "--app", backup["app"],
                  "--volume", backup["volume"],
                  "--out", "/output/" + output_file]);
    rt.start(job).terminated().ensure_success();
}, #{ description: "List available snapshots" });
```

The JSON written to that file might look like:

```json
[
  { "id": "2024-01-15T03:00:00Z", "size_bytes": 104857600 },
  { "id": "2024-01-14T03:00:00Z", "size_bytes": 98304000 }
]
```

It's preferable to post-process the output to return a well-formed snapshot list without extra fields; the web UI will not render nested objects and arrays for example.

### `restore-snapshot`

Called synchronously when an operator requests that a specific snapshot be restored.

**Bindings:**

| Logical key | Access | Contents |
|---|---|---|
| `destination` | read-write | Empty managed site volume; the action must populate it with the restored data |

**Params:**

| Key | Type | Description |
|---|---|---|
| `destination_volume` | string | Generated name for the `destination` binding; look up via `app.external_volume(param["destination_volume"])` |
| `backup` | object | The [backup object](#what-seedling-provides) identifying the source volume |
| `snapshot` | string | The snapshot identifier chosen by the operator (from the `list-snapshots` output) |

**Contract:** verify that the requested `snapshot` belongs to the `backup.app` + `backup.volume` pair (a restore mismatch would silently overwrite unrelated data), then write the restored snapshot data into the `destination` binding. On success, Seedling makes the site volume available to the operator under a generated name (e.g. `restore-mystrategy-<uuid>`). On failure (including the app/volume mismatch check), exit non-zero; the site volume is deleted and an error is returned to the operator.

```rhai
app.on_action("restore-snapshot", |rt, param| {
    let dest = app.external_volume(param["destination_volume"]);
    let backup = param["backup"];
    let snapshot = param["snapshot"];
    let job = app.job()
        .image("ghcr.io/example/backup-tool:latest")
        .mount("/destination", dest)
        .env("DEST_BUCKET", bucket.value())
        .command(["backup-tool", "restore",
                  "--app", backup["app"],
                  "--volume", backup["volume"],
                  "--snapshot", snapshot,
                  "--to", "/destination"]);
    rt.start(job).terminated().ensure_success();
}, #{ description: "Restore a snapshot" });
```

## Registration

Once the app is registered in Seedling, it must be opted in to the backup role before it can be used in backup strategies:

```
seedling-ctl backups apps register --app my-backup-app
```

Seedling validates at registration time that all three required actions are present. If the app is later updated and an action disappears, Seedling re-validates and will refuse the update.

Only one invocation of the backup app runs at a time: Seedling serialises concurrent backup operations for the same app.

## Complete example

```rhai
// A backup app that stores snapshots on an S3-compatible object store.
// The bucket and credentials are provided as parameters.

let bucket = app.param("bucket")
    .required(true)
    .description("S3 bucket name");

let endpoint = app.param("endpoint")
    .required(false)
    .description("S3-compatible endpoint URL (leave unset for AWS S3)");

let access_key = app.param("access-key")
    .required(true)
    .description("S3 access key ID");

let secret_key = app.param("secret-key")
    .kind("password")
    .required(true)
    .description("S3 secret access key");

// Helper to build the base environment for all backup jobs.
let base_env = || {
    let envs = [
        #{ name: "S3_BUCKET",     value: bucket.value() },
        #{ name: "S3_ACCESS_KEY", value: access_key.value() },
        #{ name: "S3_SECRET_KEY", value: secret_key.value() },
    ];
    if endpoint.is_set() {
        envs + [#{ name: "S3_ENDPOINT", value: endpoint.value() }]
    } else {
        envs
    }
};

let image = "ghcr.io/example/s3-backup:1.0.0";

// ── Required actions ─────────────────────────────────────────────────────────

app.on_action("save-snapshot", |rt, param| {
    let source = app.external_volume(param["source_volume"]);
    let backup = param["backup"];
    rt.start(
        app.job()
            .image(image)
            .env(base_env.call())
            .mount("/source", source)
            // Stamp app+volume onto the snapshot so list-snapshots can filter.
            .command(["s3-backup", "save",
                      "--app", backup["app"],
                      "--volume", backup["volume"],
                      "/source"])
    ).terminated().ensure_success();
}, #{ description: "Upload a volume snapshot to S3" });

app.on_action("list-snapshots", |rt, param| {
    let output = app.external_volume(param["output_volume"]);
    let output_file = param["output_filename"];
    let backup = param["backup"];
    // Filter by app+volume so the operator only sees matching snapshots.
    rt.start(
        app.job()
            .image(image)
            .env(base_env.call())
            .mount("/output", output)
            .command(["s3-backup", "list",
                      "--app", backup["app"],
                      "--volume", backup["volume"],
                      "--out", "/output/" + output_file])
    ).terminated().ensure_success();
}, #{ description: "List available snapshots on S3" });

app.on_action("restore-snapshot", |rt, param| {
    let dest = app.external_volume(param["destination_volume"]);
    let backup = param["backup"];
    rt.start(
        app.job()
            .image(image)
            .env(base_env.call())
            .mount("/destination", dest)
            // The backup tool must refuse to restore if the snapshot's
            // stamped app+volume don't match these.
            .command(["s3-backup", "restore",
                      "--app", backup["app"],
                      "--volume", backup["volume"],
                      "--snapshot", param["snapshot"],
                      "--to", "/destination"])
    ).terminated().ensure_success();
}, #{ description: "Restore a snapshot from S3" });
```

## Notes for implementors

- **Idempotency**: `save-snapshot` may be retried by Seedling on failure (up to twice). Make sure your upload is idempotent or uses a staging key before committing.
- **Snapshot list format**: make the snapshot identifier (`id`, `name`, or similar) a stable, opaque string that your `restore-snapshot` action can use directly. ISO 8601 timestamps work well when combined with a volume path prefix.
- **Concurrency**: Seedling serialises all invocations of your backup app's actions — `save-snapshot`, `list-snapshots`, and `restore-snapshot` will never overlap with each other for the same registered backup app.
- **Credentials**: store secrets as `kind: "password"` parameters so the UI masks them. They are never stored in plaintext by Seedling itself.
- **Multiple volumes**: a single backup app registration can serve any number of strategies covering any volumes. The [backup object](#what-seedling-provides) in `param["backup"]` tells you which (strategy, app, volume) triple is being operated on — use it both to stamp stored snapshots (on `save-snapshot`) and to filter the returned list (on `list-snapshots`). See the filtering note under [`list-snapshots`](#list-snapshots).
