// PostgreSQL database service.
//
// Runs an official PostgreSQL container with tuning derived from node resources
// and a handful of operator-facing params. The unix socket is exposed through
// an exported app volume so other apps can mount it, and the database TCP port
// is published through an exported service.
//
// Version upgrades:
//   - Minor bumps (e.g. 16.2 → 16.3) trigger a rolling restart.
//   - Major bumps (e.g. 15 → 17) run pg_upgrade via docker.io/tianon/postgres-upgrade
//     through each intermediate major version. Data for major N lives at
//     /data/pgN within the external data volume; old data directories are kept
//     after upgrade for rollback.
//
// The shell actions include a `bestool-psql` shell backed by binaries fetched
// from the upstream GitHub release at install time (and via a standalone
// action) into an app volume.

// ── Params ────────────────────────────────────────────────────────────────────

let version = app.param("version")
    .required(true)
    .description("PostgreSQL image tag (docker.io/library/postgres:<tag>), e.g. 18.3");

let password = app.param("password")
    .kind("password")
    .required(true)
    .description("Password for the 'postgres' superuser; change to rotate");

let bestool_psql_version = app.param("bestool-psql-version")
    .required(false)
    .default_value("1.4.2")
    .description("bestool-psql release version to fetch from github.com/beyondessential/bestool");

let max_connections = app.param("max-connections")
    .required(false)
    .default_value("100")
    .description("max_connections setting; affects per-connection memory sizing");

let db_type = app.param("db-type")
    .required(false)
    .default_value("mixed")
    .description("Workload profile for tuning: web, oltp, dw, mixed, desktop");

let extra_config = app.param("extra-config")
    .kind("multiline")
    .required(false)
    .description("Extra postgresql.conf lines appended to the generated config");

// ── Images ────────────────────────────────────────────────────────────────────

let pg_image = || `docker.io/library/postgres:${version.value()}`;
let fetch_image = "docker.io/library/debian:bookworm-slim";

let postgres_uid = 999;

// ── Helpers ───────────────────────────────────────────────────────────────────

let parse_major = |v| {
    let head = v.split(".")[0].split("-")[0];
    if head == "" { throw `cannot extract major version from "${v}"` }
    parse_int(head)
};

let pg_major = || parse_major.call(version.value());

// Compute an integer minimum without a helper: Rhai has `min`/`max` free fns.

// pgtune-style defaults. Input: AVAILABLE_MEMORY (bytes), AVAILABLE_THREADS,
// max_connections, db_type. All output values are integers except
// random_page_cost, which is rendered as a literal string.
let compute_tuning = || {
    let mem_mb = AVAILABLE_MEMORY / 1048576;
    let cpu = AVAILABLE_THREADS;
    let conns = parse_int(max_connections.value());
    let dtype = db_type.value();

    let shared_mb = mem_mb / 4;
    if shared_mb < 128 { shared_mb = 128 }
    if shared_mb > 8192 { shared_mb = 8192 }

    let effective_cache_mb = mem_mb * 3 / 4;
    if effective_cache_mb < 256 { effective_cache_mb = 256 }

    let maint_cap = if dtype == "dw" { 4096 } else { 2048 };
    let maintenance_mb = mem_mb / 16;
    if maintenance_mb > maint_cap { maintenance_mb = maint_cap }
    if maintenance_mb < 64 { maintenance_mb = 64 }

    let parallel_per_gather = if dtype == "dw" { 4 } else { 2 };
    if cpu < 4 { parallel_per_gather = 1 }

    let work_divisor = conns * 3;
    let work_mem_kb = (mem_mb - shared_mb) * 1024 / work_divisor;
    if dtype == "dw" { work_mem_kb = work_mem_kb / 2 }
    if dtype == "desktop" { work_mem_kb = work_mem_kb / 2 }
    if work_mem_kb < 64 { work_mem_kb = 64 }

    let wal_buffers_mb = shared_mb * 3 / 100;
    if wal_buffers_mb > 16 { wal_buffers_mb = 16 }
    if wal_buffers_mb < 1 { wal_buffers_mb = 1 }

    let random_page_cost = "1.1";
    let effective_io_concurrency = 200;

    let max_workers = cpu;
    if max_workers < 2 { max_workers = 2 }

    let checkpoint_target = if dtype == "dw" { "0.9" } else { "0.9" };
    let default_stats_target = if dtype == "dw" { 500 } else { 100 };

    let max_wal_size_gb = if dtype == "dw" { 16 } else { 4 };
    let min_wal_size_gb = if dtype == "dw" { 4 } else { 1 };

    #{
        max_connections: conns,
        shared_buffers_mb: shared_mb,
        effective_cache_mb: effective_cache_mb,
        maintenance_mb: maintenance_mb,
        work_mem_kb: work_mem_kb,
        wal_buffers_mb: wal_buffers_mb,
        random_page_cost: random_page_cost,
        effective_io_concurrency: effective_io_concurrency,
        max_worker_processes: max_workers,
        max_parallel_workers: max_workers,
        max_parallel_workers_per_gather: parallel_per_gather,
        max_parallel_maintenance_workers: parallel_per_gather,
        checkpoint_completion_target: checkpoint_target,
        default_statistics_target: default_stats_target,
        max_wal_size_gb: max_wal_size_gb,
        min_wal_size_gb: min_wal_size_gb,
    }
};

let pg_conf_content = || {
    let t = compute_tuning.call();
    let extra = if extra_config.is_set() { extra_config.value() } else { "" };
    `# Generated by Seedling. Regenerate with the "reconfigure" action.

listen_addresses = '*'
port = 5432
unix_socket_directories = '/var/run/postgresql'
max_connections = ${t.max_connections}

shared_buffers = ${t.shared_buffers_mb}MB
effective_cache_size = ${t.effective_cache_mb}MB
maintenance_work_mem = ${t.maintenance_mb}MB
work_mem = ${t.work_mem_kb}kB

wal_buffers = ${t.wal_buffers_mb}MB
max_wal_size = ${t.max_wal_size_gb}GB
min_wal_size = ${t.min_wal_size_gb}GB
checkpoint_completion_target = ${t.checkpoint_completion_target}

default_statistics_target = ${t.default_statistics_target}
random_page_cost = ${t.random_page_cost}
effective_io_concurrency = ${t.effective_io_concurrency}

max_worker_processes = ${t.max_worker_processes}
max_parallel_workers_per_gather = ${t.max_parallel_workers_per_gather}
max_parallel_workers = ${t.max_parallel_workers}
max_parallel_maintenance_workers = ${t.max_parallel_maintenance_workers}

password_encryption = scram-sha-256

logging_collector = off
log_destination = 'stderr'
log_line_prefix = '%t [%p] %q%u@%d '
log_timezone = 'UTC'
log_checkpoints = on
log_min_duration_statement = 1000

${extra}
`
};

// Trust on the local unix socket (access controlled by the exported socket
// volume mount), scram-sha-256 for TCP.
let pg_hba_content = || {
    `# Generated by Seedling. Regenerate with the "reconfigure" action.
# TYPE  DATABASE  USER  ADDRESS       METHOD
local   all       all                 trust
host    all       all   127.0.0.1/32  scram-sha-256
host    all       all   ::1/128       scram-sha-256
host    all       all   0.0.0.0/0     scram-sha-256
host    all       all   ::/0          scram-sha-256
`
};

// ── Resources ─────────────────────────────────────────────────────────────────

let data = app.external_volume("data");

let socket = app.volume("socket")
    .exported(#{ description: "PostgreSQL unix socket directory (/var/run/postgresql)" });

let config_vol = app.volume("config");

let bin = app.volume("bin");

let pg_svc = app.service("postgres")
    .exported(#{ description: "PostgreSQL wire protocol (TCP 5432)" });

// Worst-case PID budget: one backend per connection plus background workers,
// some margin for pg_upgrade tooling.
let pids_budget = || {
    let conns = parse_int(max_connections.value());
    conns + 100
};

app.deployment("postgres")
    .image(pg_image.call())
    .scale(1)
    .on_update(OnUpdate.Replace)
    .cap_add("CHOWN")
    .cap_add("DAC_OVERRIDE")
    .cap_add("FOWNER")
    .cap_add("SETUID")
    .cap_add("SETGID")
    .pids_limit(pids_budget.call())
    // PGDATA is the only password-agnostic env the entrypoint needs. The
    // cluster is initdb'd by a dedicated install job (see on_install) so no
    // POSTGRES_PASSWORD* / POSTGRES_INITDB_ARGS leak into the long-running
    // deployment's spec — the entrypoint sees PG_VERSION and skips init.
    .env("PGDATA", `/data/pg${pg_major.call()}`)
    .mount("/data", data)
    .mount("/var/run/postgresql", socket)
    .mount("/config", config_vol)
    .arg([
        "postgres",
        "-c", "config_file=/config/postgresql.conf",
        "-c", "hba_file=/config/pg_hba.conf",
    ])
    .tcp(5432, pg_svc);

// ── Helper closures (dynamic) ─────────────────────────────────────────────────

// Render current postgresql.conf + pg_hba.conf into the config volume.
// Staged via an anonymous tmpfs so the rendered content comes from the script
// (which has the param values) rather than being built with shell quoting.
let render_config = |rt| {
    let staging = app.volume().tmpfs();
    staging.write("/postgresql.conf", pg_conf_content.call());
    staging.write("/pg_hba.conf", pg_hba_content.call());

    // chown to postgres because seedling writes the staging files as root
    // with mode 0640 (no per-file ownership/mode in volume.write yet), and
    // the postgres process inside the deployment container runs as uid 999.
    rt.start(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .cap_add("CHOWN")
        .cap_add("DAC_OVERRIDE")
        .cap_add("FOWNER")
        .mount("/staging", staging)
        .mount("/config", config_vol)
        .command(["sh", "-c",
            `install -m 0640 -o ${postgres_uid} -g ${postgres_uid} /staging/postgresql.conf /config/postgresql.conf && ` +
            `install -m 0640 -o ${postgres_uid} -g ${postgres_uid} /staging/pg_hba.conf /config/pg_hba.conf`
        ])
    ).terminated(60).ensure_success();
};

// Fetch the bestool-psql release archive from GitHub and deposit the
// `bestool-psql` and `bestool-psql-audit` binaries at /app/bin/bin/ inside
// the `bin` app volume. The gnu/glibc tarball is selected to match the
// Debian-family postgres image the shell jobs run inside.
let build_bestool_psql = |rt| {
    rt.start(app.job()
        .image(fetch_image)
        .writable_rootfs()
        // apt's privilege-drop dance (setuid to _apt for HTTP fetches,
        // setgroups, etc.) needs the standard set of POSIX capabilities
        // that seedling drops by default per actuate.container.hardening.
        .cap_add("CHOWN")
        .cap_add("DAC_OVERRIDE")
        .cap_add("FOWNER")
        .cap_add("SETUID")
        .cap_add("SETGID")
        .env("BESTOOL_PSQL_VERSION", bestool_psql_version.value())
        .mount("/app/bin", bin)
        .command(["sh", "-c", `
set -eu
apt-get update -qq
apt-get install -y --no-install-recommends curl ca-certificates zstd
arch=$(uname -m)
case "$arch" in
  x86_64) target=x86_64-unknown-linux-gnu ;;
  aarch64) target=aarch64-unknown-linux-gnu ;;
  *) echo "unsupported architecture: $arch" >&2; exit 1 ;;
esac
v=$BESTOOL_PSQL_VERSION
url="https://github.com/beyondessential/bestool/releases/download/bestool-psql-v$v/bestool-psql-$v-$target.tar.zst"
mkdir -p /app/bin/bin
cd /tmp
curl -fL --retry 3 -o bp.tar.zst "$url"
tar --zstd -xf bp.tar.zst
install -m 0755 bestool-psql /app/bin/bin/bestool-psql
install -m 0755 bestool-psql-audit /app/bin/bin/bestool-psql-audit
`])
    ).terminated(600).ensure_success();
};

// Initdb the data directory for the given major version if it doesn't yet
// exist. The password is staged into a RAM-backed tmpfs volume for this one
// invocation — the postgres cluster catalogues hold the hash afterwards, so
// the password never lands in a persistent volume.
let initdb_if_missing = |rt, major| {
    let pwfile = app.volume().tmpfs();
    pwfile.write("/pwfile", password.value());

    rt.start(app.job()
        .image(`docker.io/library/postgres:${major}`)
        .writable_rootfs()
        .cap_add("CHOWN")
        .cap_add("DAC_OVERRIDE")
        .cap_add("FOWNER")
        .cap_add("SETUID")
        .cap_add("SETGID")
        .mount("/data", data)
        .mount("/pw", pwfile)
        .command(["sh", "-c", `
set -e
if [ ! -f /data/pg${major}/PG_VERSION ]; then
  mkdir -p /data/pg${major}
  chown ${postgres_uid}:${postgres_uid} /data/pg${major}
  chmod 0700 /data/pg${major}
  # The pwfile is written by seedling as root with mode 0640 (volume.write
  # has no way to specify per-file ownership/mode yet), so postgres can't
  # read it after gosu drops privileges. Make it world-readable for this
  # one-shot job; the file lives only on the per-instance tmpfs volume
  # /run/seedling/tmpfs-volumes/<id> which is removed when the job exits.
  chmod 0644 /pw/pwfile
  gosu postgres initdb -D /data/pg${major} -U postgres \
    --auth-local=trust --auth-host=scram-sha-256 --data-checksums \
    --pwfile=/pw/pwfile
fi
`])
    ).terminated(900).ensure_success();
};

// Reload postgresql.conf without restarting.
let reload_server = |rt| {
    rt.start(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command(["psql", "-d", "postgres", "-c", "SELECT pg_reload_conf();"])
    ).terminated(30).ensure_success();
};

// ── Install ───────────────────────────────────────────────────────────────────

app.on_install(|rt, _param| {
    initdb_if_missing.call(rt, pg_major.call());
    render_config.call(rt);
    build_bestool_psql.call(rt);

    // Generous deadline: a fresh postgres startup that needs to validate
    // config and warm caches takes longer than the 30s default, especially
    // on first install where the image was just pulled.
    rt.start(app).ready(120);
}, #{});

// ── Maintenance actions ───────────────────────────────────────────────────────

app.on_action("reconfigure", |rt, _param| {
    render_config.call(rt);
    reload_server.call(rt);
}, #{ description: "Regenerate postgresql.conf and pg_hba.conf from current params and reload" });

app.on_action("reload", |rt, _param| {
    reload_server.call(rt);
}, #{ description: "Reload postgresql.conf without restarting the server" });

app.on_action("restart", |rt, _param| {
    rt.restart(app.deployment("postgres"));
}, #{ description: "Restart the PostgreSQL server" });

app.on_action("build-bestool-psql", |rt, _param| {
    build_bestool_psql.call(rt);
}, #{ description: "Re-fetch bestool-psql binaries into the app bin volume" });

app.on_action("status", |rt, _param| {
    rt.start(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .env("PGHOST", "/var/run/postgresql")
        .command(["pg_isready", "-U", "postgres"])
    ).terminated(30).ensure_success();
}, #{ description: "Run pg_isready against the local instance" });

app.on_action("vacuum", |rt, _param| {
    rt.start(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command(["vacuumdb", "--all", "--analyze", "--jobs", "2"])
    ).terminated_eventually().ensure_success();
}, #{ description: "VACUUM ANALYZE all databases" })
    .on_schedule("H 4 * * *");

// pg_dumpall of the whole cluster into the given output volume. Structured
// with the reserved `_volume` / `_filename` params so it plays nicely with
// future backup-app wiring; operators can also invoke it against any external
// volume they pass in.
app.on_action("dump", |rt, param| {
    let output = app.external_volume(param["output_volume"]);
    let filename = if "output_filename" in param { param["output_filename"] } else { "postgres.sql" };
    rt.start(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .mount("/output", output)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command(["sh", "-c", `pg_dumpall --clean --if-exists > '/output/${filename}'`])
    ).terminated_eventually().ensure_success();
}, #{ description: "pg_dumpall the cluster into an external volume" });

app.on_action("create-database", |rt, param| {
    let dbname = param["name"];
    let owner = if "owner" in param { param["owner"] } else { "postgres" };
    rt.start(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command(["createdb", "--owner", owner, dbname])
    ).terminated(60).ensure_success();
}, #{
    description: "Create a new database with the given name and optional owner",
    params: #{
        name: #{ kind: "text", required: true, description: "Database name" },
        owner: #{ kind: "text", required: false, description: "Role that owns the database (defaults to postgres)" },
    },
});

// ── Shells ────────────────────────────────────────────────────────────────────

app.on_shell("bash", |_rt, shell, _param| {
    shell.attach(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .mount("/data", data)
        .mount("/config", config_vol)
        .mount("/app/bin", bin)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command("bash"));
}, #{ description: "Bash shell in the PostgreSQL container with socket and data mounted" });

app.on_shell("psql", |_rt, shell, _param| {
    shell.attach(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command(["psql", "-d", "postgres"]));
}, #{ description: "psql shell connected to the local instance over the unix socket" });

app.on_shell("bestool-psql", |_rt, shell, param| {
    let default_url = "postgresql://postgres@/postgres?host=/var/run/postgresql";
    let db_url = if "url" in param { param["url"] } else { default_url };
    shell.attach(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .mount("/app/bin", bin)
        .command(["/app/bin/bin/bestool-psql", db_url]));
}, #{
    description: "bestool-psql shell; defaults to the local unix socket",
    params: #{
        url: #{
            kind: "text",
            required: false,
            description: "Database URL (defaults to the local unix socket as postgres)",
        },
    },
});

// ── Version changes ───────────────────────────────────────────────────────────

// Minor bumps: regenerate config (in case initdb-time settings shift between
// patchlevels) and restart via the Replace update strategy.
// Major bumps: run pg_upgrade through tianon/postgres-upgrade for each
// intermediate major version, using /data/pgN subdirectories. The new data
// directory is initdb'd first by the new major's postgres image; pg_upgrade
// then hardlink-copies relation files across (no --link: safer, at the cost
// of roughly doubling disk usage during the upgrade).
version.on_change(|rt, old| {
    let old_major = parse_major.call(old.param("version").value());
    let new_major = parse_major.call(version.value());

    if new_major < old_major {
        throw `refusing to downgrade from PostgreSQL ${old_major} to ${new_major}`;
    }

    // Prefetch every image this change will need before touching the running
    // cluster. Also serves as a "does this version exist" check: if the new
    // postgres tag (or an intermediate tianon upgrade image, on a major bump)
    // is not pullable, the barrier throws and the old server keeps running.
    let warm_jobs = [app.job().image(pg_image.call())];
    let scan = old_major;
    while scan < new_major {
        let next_scan = scan + 1;
        warm_jobs.push(app.job().image(`docker.io/library/postgres:${next_scan}`));
        warm_jobs.push(app.job().image(`docker.io/tianon/postgres-upgrade:${scan}-to-${next_scan}`));
        scan = next_scan;
    }
    rt.warm_images(warm_jobs).ready(600);

    render_config.call(rt);

    if new_major == old_major {
        // Minor bump: Replace update strategy stops the old before starting
        // the new, which is what we want on a shared data dir.
        rt.stop(old.select(#{ types: [ResourceType.Deployment], names: ["postgres"] }));
        rt.start(app).ready();
    } else {
        // Major upgrade path. Stop the old server first.
        rt.stop(old.select(#{ types: [ResourceType.Deployment], names: ["postgres"] }));

        let cur = old_major;
        while cur < new_major {
            let next = cur + 1;

            // Initdb the new cluster inside the external data volume.
            // pg_upgrade replaces the roles/passwords from the old cluster
            // immediately after, so the pwfile passed in here is superseded
            // before the cluster ever serves traffic.
            initdb_if_missing.call(rt, next);

            // pg_upgrade in the tianon image, which ships both major binaries.
            // Workdir must be writable: pg_upgrade writes log files there.
            rt.start(app.job()
                .image(`docker.io/tianon/postgres-upgrade:${cur}-to-${next}`)
                .writable_rootfs()
                .cap_add("CHOWN")
                .cap_add("DAC_OVERRIDE")
                .cap_add("FOWNER")
                .cap_add("SETUID")
                .cap_add("SETGID")
                .mount("/data", data)
                .workdir("/tmp")
                .command(["sh", "-c", `
set -e
chown -R ${postgres_uid}:${postgres_uid} /data/pg${cur} /data/pg${next}
cd /tmp
gosu postgres pg_upgrade \
  --old-bindir=/usr/lib/postgresql/${cur}/bin \
  --new-bindir=/usr/lib/postgresql/${next}/bin \
  --old-datadir=/data/pg${cur} \
  --new-datadir=/data/pg${next}
`])
            ).terminated(7200).ensure_success();

            cur = next;
        }

        // Bring up the new-major deployment (PGDATA picks up /data/pg<new>
        // from the env in the current AppDef).
        rt.start(app).ready();

        // Post-upgrade analyze is highly recommended; fire-and-forget-ish.
        // Any operator-relevant failure surfaces through the normal action log.
        rt.start(app.job()
            .image(pg_image.call())
            .writable_rootfs()
            .mount("/var/run/postgresql", socket)
            .env("PGHOST", "/var/run/postgresql")
            .env("PGUSER", "postgres")
            .command(["vacuumdb", "--all", "--analyze-in-stages", "--jobs", "2"])
        ).terminated_eventually();
    }
});

// ── Tuning parameter changes ──────────────────────────────────────────────────
// Every tuning param touches a setting that either requires a restart (e.g.
// max_connections, shared_buffers via AVAILABLE_MEMORY) or might (extra-config
// is opaque). Restart rather than reload to be safe.

max_connections.on_change(|rt, _old| {
    render_config.call(rt);
    rt.restart(app.deployment("postgres"));
});

db_type.on_change(|rt, _old| {
    render_config.call(rt);
    rt.restart(app.deployment("postgres"));
});

extra_config.on_change(|rt, _old| {
    render_config.call(rt);
    rt.restart(app.deployment("postgres"));
});

// Re-fetch the bestool-psql binaries on version change. No deployment
// restart needed: the binaries live in an app volume and are only invoked
// from on-demand shell jobs.
bestool_psql_version.on_change(|rt, _old| {
    build_bestool_psql.call(rt);
});

// ── Password rotation ────────────────────────────────────────────────────────
// Apply the new password via ALTER USER against the running cluster. The SQL
// is staged into a RAM-backed tmpfs so the password isn't written to any
// persistent volume. No deployment restart is needed since the password isn't
// referenced by the deployment spec.
password.on_change(|rt, _old| {
    let escaped = password.value().replace("'", "''");
    let staging = app.volume().tmpfs();
    staging.write("/alter.sql", `ALTER USER postgres WITH PASSWORD '${escaped}';`);

    rt.start(app.job()
        .image(pg_image.call())
        .writable_rootfs()
        .mount("/var/run/postgresql", socket)
        .mount("/staging", staging)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command(["psql", "-d", "postgres", "-f", "/staging/alter.sql"])
    ).terminated(60).ensure_success();
});
