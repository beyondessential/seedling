# Writing a Backup App

A backup app is an ordinary Seedling application that has been _registered_ as a backup provider. Seedling treats it specially: it invokes three well-defined actions to save, list, and restore snapshots of other apps' volumes.

Before reading this, you should be familiar with [BSL scripting](bsl-scripting.md).

## What Seedling provides

When Seedling invokes a backup action, it injects _operation-scoped volume bindings_ into the backup app for the duration of that invocation. These bindings are accessed via `app.external_volume(name)` inside the action closure, exactly like any other external volume — but they only exist for the duration of that one invocation.

The external volume names are fixed and documented per action below.

## Required actions

A backup app must define exactly these three actions using `app.on_action()`. The names must match exactly.

### `save-snapshot`

Called by Seedling when a scheduled or manually triggered backup runs for a volume.

**Bindings:**

| Name | Access | Contents |
|---|---|---|
| `source` | read-only | A point-in-time snapshot of the volume being backed up |

**Params:** none beyond what Seedling provides internally.

**Contract:** read from `source`, persist the data to whatever external backend the backup app uses (object storage, a remote server, etc.). Exit successfully on success; Seedling will retry up to twice on failure and file a `backup_failed` fault if all attempts fail.

```rhai
app.on_action("save-snapshot", |rt, _param| {
    let source = app.external_volume("source");
    let job = app.job()
        .image("ghcr.io/example/backup-tool:latest")
        .mount("/source", source)
        .env("DEST_BUCKET", bucket.value())
        .command(["backup-tool", "upload", "/source"]);
    rt.start(job).terminated().ensure_success();
}, #{ description: "Upload a snapshot to object storage" });
```

### `list-snapshots`

Called synchronously when an operator requests a list of available snapshots for a given volume and strategy.

**Bindings:**

| Name | Access | Contents |
|---|---|---|
| `output` | read-write | Empty directory; the action must write `snapshots.json` here |

**Params:**

| Key | Type | Description |
|---|---|---|
| `volume` | string | The volume ID being queried, e.g. `myapp/data` or `_site/myvol` |

**Contract:** write a file at `/output/snapshots.json` (where `/output` is the mountpoint of the `output` binding) containing valid JSON describing the available snapshots. The JSON is returned verbatim to the operator via the API and displayed in the UI — it can be any structure, but an array of objects is the most useful because the UI will render them as a table and offer a per-row restore button.

The `snapshot` identifier used in `restore-snapshot` must be derivable from the data in this file. A common convention is an `id` or `name` field on each object.

```rhai
app.on_action("list-snapshots", |rt, param| {
    let output = app.external_volume("output");
    let vol = param["volume"];
    let job = app.job()
        .image("ghcr.io/example/backup-tool:latest")
        .mount("/output", output)
        .env("DEST_BUCKET", bucket.value())
        .env("VOLUME", vol)
        .command(["backup-tool", "list", "--volume", vol, "--out", "/output/snapshots.json"]);
    rt.start(job).terminated().ensure_success();
}, #{ description: "List available snapshots" });
```

The JSON written to `snapshots.json` might look like:

```json
[
  { "id": "2024-01-15T03:00:00Z", "size_bytes": 104857600 },
  { "id": "2024-01-14T03:00:00Z", "size_bytes": 98304000 }
]
```

### `restore-snapshot`

Called synchronously when an operator requests that a specific snapshot be restored.

**Bindings:**

| Name | Access | Contents |
|---|---|---|
| `destination` | read-write | Empty managed site volume; the action must populate it with the restored data |

**Params:**

| Key | Type | Description |
|---|---|---|
| `snapshot` | string | The snapshot identifier chosen by the operator (from the `list-snapshots` output) |
| `volume` | string | The original volume ID that was backed up, e.g. `myapp/data` |

**Contract:** write the restored snapshot data into `destination`. On success, Seedling makes the site volume available to the operator under a generated name (e.g. `restore-mystrategy-<uuid>`). On failure, the site volume is deleted and an error is returned.

```rhai
app.on_action("restore-snapshot", |rt, param| {
    let dest = app.external_volume("destination");
    let snapshot = param["snapshot"];
    let vol = param["volume"];
    let job = app.job()
        .image("ghcr.io/example/backup-tool:latest")
        .mount("/destination", dest)
        .env("DEST_BUCKET", bucket.value())
        .env("SNAPSHOT", snapshot)
        .env("VOLUME", vol)
        .command(["backup-tool", "restore", snapshot, "--to", "/destination"]);
    rt.start(job).terminated().ensure_success();
}, #{ description: "Restore a snapshot" });
```

## Registration

Once the app is registered in Seedling, it must be registered as a backup provider before it can be used in backup strategies:

```
seedling-ctl backups apps register --name my-backup --app my-backup-app
```

Seedling validates at registration time that all three required actions are present. If the underlying app is later updated and an action disappears, Seedling re-validates and will refuse the update.

Only one invocation of the backup app runs at a time: Seedling serialises concurrent backup operations for the same provider.

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

app.on_action("save-snapshot", |rt, _param| {
    let source = app.external_volume("source");
    rt.start(
        app.job()
            .image(image)
            .env(base_env.call())
            .mount("/source", source)
            .command(["s3-backup", "save", "/source"])
    ).terminated().ensure_success();
}, #{ description: "Upload a volume snapshot to S3" });

app.on_action("list-snapshots", |rt, param| {
    let output = app.external_volume("output");
    rt.start(
        app.job()
            .image(image)
            .env(base_env.call())
            .env("VOLUME", param["volume"])
            .mount("/output", output)
            .command(["s3-backup", "list", "--volume", param["volume"], "--out", "/output/snapshots.json"])
    ).terminated().ensure_success();
}, #{ description: "List available snapshots on S3" });

app.on_action("restore-snapshot", |rt, param| {
    let dest = app.external_volume("destination");
    rt.start(
        app.job()
            .image(image)
            .env(base_env.call())
            .env("VOLUME", param["volume"])
            .env("SNAPSHOT", param["snapshot"])
            .mount("/destination", dest)
            .command(["s3-backup", "restore", param["snapshot"], "--to", "/destination"])
    ).terminated().ensure_success();
}, #{ description: "Restore a snapshot from S3" });
```

## Notes for implementors

- **Idempotency**: `save-snapshot` may be retried by Seedling on failure (up to twice). Make sure your upload is idempotent or uses a staging key before committing.
- **`snapshots.json` format**: make the snapshot identifier (`id`, `name`, or similar) a stable, opaque string that your `restore-snapshot` action can use directly. ISO 8601 timestamps work well when combined with a volume path prefix.
- **Concurrency**: Seedling serialises all invocations of your backup app's actions — `save-snapshot`, `list-snapshots`, and `restore-snapshot` will never overlap with each other for the same registered backup app.
- **Credentials**: store secrets as `kind: "password"` parameters so the UI masks them. They are never stored in plaintext by Seedling itself.
- **Multiple volumes**: a single backup app registration can serve any number of strategies covering any volumes. The `volume` param tells you which volume is being operated on, so you can namespace your remote storage accordingly.
