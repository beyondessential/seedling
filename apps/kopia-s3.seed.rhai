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

// ── Helpers ───────────────────────────────────────────────────────────────────

// Environment for every Kopia job.  AWS credentials are passed via env vars;
// the AWS SDK (used by Kopia internally) picks them up automatically.
let kopia_env = || {
    let e = [
        #{ name: "KOPIA_PASSWORD",       value: passphrase.value() },
        #{ name: "AWS_ACCESS_KEY_ID",     value: access_key.value() },
        #{ name: "AWS_SECRET_ACCESS_KEY", value: secret_key.value() },
    ];
    if region.is_set() {
        e + [#{ name: "AWS_REGION", value: region.value() }]
    } else {
        e
    }
};

// Shell fragment that connects to the S3 repository.
// Credentials come from the environment (kopia_env); non-sensitive config
// is inlined.  The image is Alpine-based so busybox sh is available.
let connect_sh = || {
    let cmd = "kopia repository connect s3 --no-check-for-updates" +
        " '--bucket=" + bucket.value() + "'" +
        " '--override-hostname=" + hostname.value() + "'";
    let cmd = if prefix.is_set() {
        cmd + " '--prefix=" + prefix.value() + "'"
    } else { cmd };
    if endpoint.is_set() {
        cmd + " '--endpoint=" + endpoint.value() + "'"
    } else if region.is_set() {
        cmd + " --region=$AWS_REGION"
    } else { cmd }
};

// Shell fragment that creates the S3 repository (used only during install).
let create_sh = || {
    let cmd = "kopia repository create s3" +
        " '--bucket=" + bucket.value() + "'" +
        " '--override-hostname=" + hostname.value() + "'";
    let cmd = if prefix.is_set() {
        cmd + " '--prefix=" + prefix.value() + "'"
    } else { cmd };
    if endpoint.is_set() {
        cmd + " '--endpoint=" + endpoint.value() + "'"
    } else if region.is_set() {
        cmd + " --region=$AWS_REGION"
    } else { cmd }
};

// Shell fragment that writes the global policy from current param values.
let policy_sh = || {
    "kopia policy set --global" +
    " '--compression=" + (if compression.is_set() { compression.value() } else { "zstd-fastest" }) + "'" +
    " '--keep-latest=" + (if retention_latest.is_set() { retention_latest.value() } else { "3" }) + "'" +
    " '--keep-daily=" + (if retention_daily.is_set() { retention_daily.value() } else { "7" }) + "'" +
    " '--keep-weekly=" + (if retention_weekly.is_set() { retention_weekly.value() } else { "4" }) + "'" +
    " '--keep-monthly=" + (if retention_monthly.is_set() { retention_monthly.value() } else { "12" }) + "'" +
    " '--keep-annual=" + (if retention_annual.is_set() { retention_annual.value() } else { "3" }) + "'"
};

// Base job for all Kopia tasks.  Writable rootfs is required so Kopia can
// write its repository config to ~/.config/kopia/.
let kopia_job = || {
    app.job()
        .image(image.call())
        .env(kopia_env.call())
        .writable_rootfs()
};

// ── Install ───────────────────────────────────────────────────────────────────

// Creates the Kopia repository in S3 and applies the initial global policy.
// Run this once after setting all required params.
app.on_install(|rt, _param| {
    rt.start(kopia_job.call()
        .command(["sh", "-c",
            create_sh.call() + " && " + policy_sh.call()
        ])
    ).terminated().ensure_success();
}, #{});

// ── Required backup actions ───────────────────────────────────────────────────

app.on_action("save-snapshot", |rt, _param| {
    let source = app.external_volume("source");
    rt.start(kopia_job.call()
        .mount("/source", source)
        .command(["sh", "-c",
            connect_sh.call() + " && kopia snapshot create /source"
        ])
    ).terminated().ensure_success();
}, #{ description: "Upload a volume snapshot to the Kopia repository" });

app.on_action("list-snapshots", |rt, _param| {
    let output = app.external_volume("output");
    rt.start(kopia_job.call()
        .mount("/output", output)
        .command(["sh", "-c",
            connect_sh.call() + " && kopia snapshot list --json > /output/snapshots.json"
        ])
    ).terminated().ensure_success();
}, #{ description: "List snapshots in the Kopia repository" });

app.on_action("restore-snapshot", |rt, param| {
    let dest = app.external_volume("destination");
    rt.start(kopia_job.call()
        .env([#{ name: "KOPIA_SNAPSHOT_ID", value: param["snapshot"] }])
        .mount("/destination", dest)
        .command(["sh", "-c",
            connect_sh.call() + " && kopia snapshot restore \"$KOPIA_SNAPSHOT_ID\" /destination"
        ])
    ).terminated().ensure_success();
}, #{ description: "Restore a Kopia snapshot to a new volume" });

// ── Maintenance actions ───────────────────────────────────────────────────────

// Quick maintenance: expire snapshots per the retention policy and compact
// indexes.  Runs nightly at a stable time.
app.on_action("maintenance", |rt, _param| {
    rt.start(kopia_job.call()
        .command(["sh", "-c",
            connect_sh.call() + " && kopia maintenance run"
        ])
    ).terminated().ensure_success();
}, #{ description: "Run quick maintenance (expire snapshots, compact indexes)" })
    .on_schedule("H 3 * * *");

// Full maintenance: includes garbage collection of unreferenced blobs.
// Runs weekly.
app.on_action("maintenance-full", |rt, _param| {
    rt.start(kopia_job.call()
        .command(["sh", "-c",
            connect_sh.call() + " && kopia maintenance run --full"
        ])
    ).terminated().ensure_success();
}, #{ description: "Run full maintenance including garbage collection" })
    .on_schedule("H 4 * * 0");

// Applies the current compression and retention param values to the global
// policy.  Run this manually after changing any policy params.
app.on_action("apply-policy", |rt, _param| {
    rt.start(kopia_job.call()
        .command(["sh", "-c",
            connect_sh.call() + " && " + policy_sh.call()
        ])
    ).terminated().ensure_success();
}, #{ description: "Apply current compression and retention settings to the global policy" });

// ── Shell ─────────────────────────────────────────────────────────────────────

// Drops the operator into a shell with the Kopia repository already connected,
// so they can run arbitrary kopia commands to inspect or repair the repository.
app.on_shell("kopia", |_rt, shell, _param| {
    shell.attach(kopia_job.call()
        .command(["sh", "-c",
            connect_sh.call() + " && exec sh"
        ])
    );
}, #{ description: "Shell with the Kopia repository connected" });
