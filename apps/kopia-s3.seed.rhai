// Backup app using Kopia (kopia.io) backed by an S3-compatible object store.
//
// Usage:
//   1. Register the app in Seedling.
//   2. Set all required params (bucket, access-key, secret-key, hostname, passphrase).
//   3. Run the install action to create the Kopia repository and apply the initial policy.
//   4. Register as a backup provider:
//        seedling-ctl backups apps register --name <name> --app <this-app>
//
// Each registered instance corresponds to one Kopia repository.  If you need
// to back up multiple independent volumes to the same bucket, use separate
// instances with different --prefix values.
//
// Run the "connect" action after rotating credentials or changing
// bucket/prefix/endpoint/region.

// ── Image ─────────────────────────────────────────────────────────────────────

let version = app.param("version")
    .required(false)
    .default_value("0.22.3")
    .description("Kopia image tag (docker.io/kopia/kopia:<tag>)");

let image = || `docker.io/kopia/kopia:${if version.is_set() { version.value() } else { "latest" }}`;

// ── Connection params ─────────────────────────────────────────────────────────

let bucket = app.param("bucket")
    .required(true)
    .description("S3 bucket name");

let prefix = app.param("prefix")
    .required(false)
    .description("S3 key prefix for this repository (e.g. backups/myapp); leave unset to use the bucket root");

let region = app.param("region")
    .required(false)
    .description("AWS region (e.g. us-east-1); leave unset when specifying a custom endpoint");

let endpoint = app.param("endpoint")
    .required(false)
    .description("S3-compatible endpoint URL; leave unset for AWS S3");

let access_key = app.param("access-key")
    .required(true)
    .description("AWS access key ID");

let secret_key = app.param("secret-key")
    .kind("password")
    .required(true)
    .description("AWS secret access key");

let hostname = app.param("hostname")
    .required(true)
    .description("Kopia hostname label associated with snapshots (any stable, unique name for this node)");

let passphrase = app.param("passphrase")
    .kind("password")
    .required(true)
    .description("Repository encryption passphrase");

// ── Policy params ─────────────────────────────────────────────────────────────

let compression = app.param("compression")
    .required(false)
    .default_value("zstd-fastest")
    .description("Compression algorithm applied to stored content (zstd-fastest, zstd, s2-default, none)");

let retention_latest = app.param("retention-latest")
    .required(false)
    .default_value("3")
    .description("Most-recent snapshots to keep per source regardless of age");

let retention_daily = app.param("retention-daily")
    .required(false)
    .default_value("7")
    .description("Daily snapshots to keep");

let retention_weekly = app.param("retention-weekly")
    .required(false)
    .default_value("4")
    .description("Weekly snapshots to keep");

let retention_monthly = app.param("retention-monthly")
    .required(false)
    .default_value("12")
    .description("Monthly snapshots to keep");

let retention_annual = app.param("retention-annual")
    .required(false)
    .default_value("3")
    .description("Annual snapshots to keep");

// ── State volume ──────────────────────────────────────────────────────────────

// Persistent Kopia state: repository config, content cache, and logs.
// Populated by the install (or connect) action; read by every other action
// so they do not need to re-connect to the repository on every run.
let state = app.volume("state");

// ── Helpers ───────────────────────────────────────────────────────────────────

// Environment for every Kopia job.  Credentials come from the env so the AWS
// SDK and Kopia pick them up automatically; path env vars redirect Kopia's
// config, cache, and logs into the state volume.
let kopia_env = || {
    let e = [
        #{ name: "KOPIA_PASSWORD",        value: passphrase.value() },
        #{ name: "AWS_ACCESS_KEY_ID",     value: access_key.value() },
        #{ name: "AWS_SECRET_ACCESS_KEY", value: secret_key.value() },
        #{ name: "KOPIA_CONFIG_PATH",     value: "/kopia-state/repository.config" },
        #{ name: "KOPIA_CACHE_DIRECTORY", value: "/kopia-state/cache" },
        #{ name: "KOPIA_LOG_DIR",         value: "/kopia-state/logs" },
        #{ name: "KOPIA_CHECK_FOR_UPDATES", value: "false" },
    ];
    if region.is_set() {
        e + [#{ name: "AWS_REGION", value: region.value() }]
    } else {
        e
    }
};

// Shell fragment: S3 flags shared by `repository create` and `repository connect`.
// Inline values are not sensitive; secrets come from env vars.  The image is
// Alpine-based so busybox sh and its single-quote handling are available.
let s3_args_sh = || {
    let args = " '--bucket=" + bucket.value() + "'" +
        " '--override-hostname=" + hostname.value() + "'";
    let args = if prefix.is_set() {
        args + " '--prefix=" + prefix.value() + "'"
    } else { args };
    if endpoint.is_set() {
        args + " '--endpoint=" + endpoint.value() + "'"
    } else if region.is_set() {
        args + " --region=$AWS_REGION"
    } else { args }
};

// Shell fragment: write the global policy from current param values.
let policy_sh = || {
    "kopia policy set --global" +
    " '--compression=" + (if compression.is_set() { compression.value() } else { "zstd-fastest" }) + "'" +
    " '--keep-latest=" + (if retention_latest.is_set() { retention_latest.value() } else { "3" }) + "'" +
    " '--keep-daily=" + (if retention_daily.is_set() { retention_daily.value() } else { "7" }) + "'" +
    " '--keep-weekly=" + (if retention_weekly.is_set() { retention_weekly.value() } else { "4" }) + "'" +
    " '--keep-monthly=" + (if retention_monthly.is_set() { retention_monthly.value() } else { "12" }) + "'" +
    " '--keep-annual=" + (if retention_annual.is_set() { retention_annual.value() } else { "3" }) + "'"
};

// Base job for every Kopia task.  Writable rootfs covers any stray writes
// (e.g. to /tmp); the repository config and cache live in the state volume.
let kopia_job = || {
    app.job()
        .image(image.call())
        .env(kopia_env.call())
        .writable_rootfs()
        .mount("/kopia-state", state)
};

// ── Install ───────────────────────────────────────────────────────────────────

// Creates the Kopia repository in S3 and applies the initial global policy.
// After this runs, the state volume holds the repository config that every
// other action reads.
app.on_install(|rt, _param| {
    rt.start(kopia_job.call()
        .command(["sh", "-c",
            "kopia repository create s3" + s3_args_sh.call() +
            " && " + policy_sh.call()
        ])
    ).terminated().ensure_success();
}, #{});

// ── Connection management ─────────────────────────────────────────────────────

// Re-connects to the existing Kopia repository using the current params.
// Run this after rotating AWS credentials or changing bucket/prefix/endpoint/region.
// Safe to run at any time: it overwrites the stored repository config.
app.on_action("connect", |rt, _param| {
    rt.start(kopia_job.call()
        .command(["sh", "-c",
            "rm -f /kopia-state/repository.config && " +
            "kopia repository connect s3" + s3_args_sh.call()
        ])
    ).terminated().ensure_success();
}, #{ description: "Re-connect to the Kopia repository using current params" });

// ── Required backup actions ───────────────────────────────────────────────────

app.on_action("save-snapshot", |rt, _param| {
    let source = app.external_volume("source");
    rt.start(kopia_job.call()
        .mount("/source", source)
        .command(["kopia", "snapshot", "create", "/source"])
    ).terminated().ensure_success();
}, #{ description: "Upload a volume snapshot to the Kopia repository" });

app.on_action("list-snapshots", |rt, _param| {
    let output = app.external_volume("output");
    rt.start(kopia_job.call()
        .mount("/output", output)
        .command(["sh", "-c",
            "kopia snapshot list --json > /output/snapshots.json"
        ])
    ).terminated().ensure_success();
}, #{ description: "List snapshots in the Kopia repository" });

app.on_action("restore-snapshot", |rt, param| {
    let dest = app.external_volume("destination");
    rt.start(kopia_job.call()
        .env([#{ name: "KOPIA_SNAPSHOT_ID", value: param["snapshot"] }])
        .mount("/destination", dest)
        .command(["sh", "-c",
            "kopia snapshot restore \"$KOPIA_SNAPSHOT_ID\" /destination"
        ])
    ).terminated().ensure_success();
}, #{ description: "Restore a Kopia snapshot to a new volume" });

// ── Maintenance actions ───────────────────────────────────────────────────────

// Quick maintenance: expire snapshots per the retention policy and compact
// indexes.  Runs nightly at a stable time.
app.on_action("maintenance", |rt, _param| {
    rt.start(kopia_job.call()
        .command(["kopia", "maintenance", "run"])
    ).terminated().ensure_success();
}, #{ description: "Run quick maintenance (expire snapshots, compact indexes)" })
    .on_schedule("H 3 * * *");

// Full maintenance: includes garbage collection of unreferenced blobs.  Runs
// weekly.
app.on_action("maintenance-full", |rt, _param| {
    rt.start(kopia_job.call()
        .command(["kopia", "maintenance", "run", "--full"])
    ).terminated().ensure_success();
}, #{ description: "Run full maintenance including garbage collection" })
    .on_schedule("H 4 * * 0");

// Applies the current compression and retention param values to the global
// policy.  Run this manually after changing any policy params.
app.on_action("apply-policy", |rt, _param| {
    rt.start(kopia_job.call()
        .command(["sh", "-c", policy_sh.call()])
    ).terminated().ensure_success();
}, #{ description: "Apply current compression and retention settings to the global policy" });

// ── Shell ─────────────────────────────────────────────────────────────────────

// Drops the operator into a shell with the Kopia repository already connected,
// so they can run arbitrary kopia commands to inspect or repair the repository.
app.on_shell("kopia", |_rt, shell, _param| {
    shell.attach(kopia_job.call()
        .command("sh")
    );
}, #{ description: "Shell with the Kopia repository connected" });
