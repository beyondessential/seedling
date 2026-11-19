// Tamanu Central server.
//
// Runs the Tamanu central API, scheduled tasks, an optional FHIR refresh +
// resolve worker pair, and a frontend, plus an optional patient portal. The
// central server connects to PostgreSQL via the unix socket exported by the
// seedling-managed `postgres` app: the operator binds this app's `pg-socket`
// external volume to that socket. Postgres `local trust` authentication makes
// passwords unnecessary for connections via the socket.
//
// Routing model:
//   - `/api/*` and `/v1/*` → API workers (port 3000)
//   - `/*` → frontend (port 3000)
//   - patient portal (when enabled) is a separate hostname / ingress
//
// Scaling: API scales 1..(AVAILABLE_THREADS / 2); the frontend is fixed at 1
// (rolling restarts handle blue/green rotation by themselves).
//
// Gates:
//   - `config` writes local.json5 into the config volume whenever its content
//     changes. The parts that read it are ordered after it, so a change
//     restarts them with the new content.
//   - `database` provisions the database and roles, and checks they exist.
//   - `migrate` runs the schema migration whenever the version moves. The
//     API and workers are stopped for the run and come back up on the new tag
//     once it succeeds. The frontends keep serving cached pages through the
//     brief outage and roll to the new tag afterwards.
//   - `certificates` warms the TLS certificates of the declared ingresses.
//
// A new install starts without an admin user: the operator runs the
// `provision-admin` action once the server is up.

app.description(`
Tamanu central server: where all the data lives, and the core of the Tamanu system.

Facility servers, mobile clients, and API clients all talk to it to synchronise and
query patient and clinical data. This is also where emails and other communications
are sent from, and the home of the patient portal integration.
`);

// ── Params ────────────────────────────────────────────────────────────────────

let version = app.param("version")
    .required(true)
    .default_value("v2.53.3")
    .description("Tamanu image tag (e.g. v2.10.0); applied to central, frontend, and patient-portal images");

let public_hostname = app.param("public-hostname")
    .required(false)
    .description("Public FQDN for the central server, served over HTTPS. Leave unset to declare no ingress, so a site ingress carries the traffic instead");

let canonical_url = app.param("canonical-url")
    .required(false)
    .description("URL the central server advertises in API responses; defaults to https://<public-hostname>. Required when public-hostname is unset");

let timezone = app.param("timezone")
    .required(false)
    .default_value(TIMEZONE)
    .description("IANA timezone, e.g. Pacific/Auckland; defaults to the host's local timezone");

let auth_secret = app.param("auth-secret")
    .kind("random")
    .secret(true)
    .required(true)
    .description("JWT signing secret used by the auth subsystem; click the dice icon to generate a fresh 32-byte hex value");

let refresh_secret = app.param("refresh-secret")
    .kind("random")
    .secret(true)
    .required(true)
    .description("Refresh-token signing secret used by the auth subsystem; click the dice icon to generate a fresh 32-byte hex value");

// A database or role name reaches SQL identifiers and a shell string, so it is
// limited to what needs no quoting.
let sql_name = |v| {
    if v.len() == 0 { throw "must not be empty"; }
    for c in v.chars() {
        if !((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '_') {
            throw `must be lower-case letters, digits and underscores, not "${v}"`;
        }
    }
};

let db_name = app.param("db-name")
    .required(false)
    .default_value("tamanu")
    .description("PostgreSQL database name")
    .validate(|v, all| sql_name.call(v));

let db_user = app.param("db-user")
    .required(false)
    .default_value("tamanu")
    .description("PostgreSQL role the central server connects as (uses local-socket trust auth, no password)")
    .validate(|v, all| sql_name.call(v));

let fhir_enabled = app.param("fhir-enabled")
    .required(false)
    .default_value("false")
    .description("Whether to run the FHIR refresh + resolve workers (set to 'true' to enable)");

let patient_portal_fqdn = app.param("patient-portal-fqdn")
    .required(false)
    .description("Public FQDN for the patient portal; if set, the patient portal subsystem is enabled");

// ── External resources ────────────────────────────────────────────────────────

// Postgres unix socket. The operator maps this to the `socket` volume
// exported by the postgres app via the external-volume mapping table.
let pg_socket = app.external_volume("pg-socket")
    .description("PostgreSQL unix socket — operator maps to the postgres app's exported socket volume");

// ── Helpers ───────────────────────────────────────────────────────────────────

let server_image = || `ghcr.io/beyondessential/tamanu-central:${version.value()}`;
let frontend_image = || `ghcr.io/beyondessential/tamanu-frontend:${version.value()}`;
let portal_image = || `ghcr.io/beyondessential/tamanu-patient-portal:${version.value()}`;

// Postgres-client image used for the one-shot DB provisioning job. The tamanu
// images don't ship with psql, so we run psql out of an alpine-based postgres
// image. Trust auth on the local socket means no password handling is needed.
let psql_image = "docker.io/library/postgres:18-alpine";

// With no public hostname there is no HTTPS ingress to derive an advertised
// URL from, so `canonical-url` has to supply one. Checked here rather than at
// the top level: this closure only runs when the config is rendered, by which
// point params are supplied, whereas a top-level throw would fire during first
// registration and leave the script unloadable.
let canonical = || {
    if !canonical_url.is_set() && !public_hostname.is_set() {
        throw "set canonical-url when public-hostname is unset: the central server must advertise a URL"
    }
    if canonical_url.is_set() {
        canonical_url.value()
    } else {
        `https://${public_hostname.value()}`
    }
};

let portal_on = || patient_portal_fqdn.is_set();
let fhir_on = || fhir_enabled.value() == "true";

// JSON-string-encode a value with the few escapes JSON5 requires.
// Rhai's `String.replace(from, to)` mutates the receiver in place and
// returns `()`, so each pass is a statement on `s`, not a chain.
// Backslash must come first so escapes inserted by later passes (e.g. the
// literal `\n` substituted in for a newline) aren't re-escaped.
let json_str = |s| {
    s.replace("\\", "\\\\");
    s.replace("\"", "\\\"");
    s.replace("\n", "\\n");
    s.replace("\r", "\\r");
    s.replace("\t", "\\t");
    `"${s}"`
};

// Mutate-in-place: see the comment on `json_str` for why a chain doesn't work.
let sql_escape = |s| { s.replace("'", "''"); s };

// Put your config edits here, changes will persist. Also see:
// - default.reference.json5 (a copy of the defaults for the current version)
// - local.json5 (configs generated by Seedling, should not be edited).
const INITIAL_CONFIG = `{
  auth: {
    reportNoUserError: true,
    useHardcodedPermissions: false,
  },
  sync: {
    lookupTable: {
      enabled: true,
    },
  },
  db: {
    reportSchemas: {
      enabled: true,
    },
  },
  disk: {
    freeSpaceRequired: {
      gigabytesForUploadingDocuments: 0.1,
    },
    diskPath: "/config",
  },
  notifications: {
    certificates: {
      labTestCategoryIds: ['labTestCategory-COVID', 'labTestCategory-COVIDRAT'],
    },
  },
}
`;

let local_json5 = || `{
    db: {
        name: ${json_str.call(db_name.value())},
        verbose: false,
        username: ${json_str.call(db_user.value())},
        host: "/var/run/postgresql",
        port: null,
        migrateOnStartup: false,
        reportSchemas: {
            connections: {
                raw: {
                    username: "raw",
                },
                reporting: {
                    username: "reporting",
                },
            },
        },
    },
    countryTimeZone: ${json_str.call(timezone.value())},
    primaryTimeZone: ${json_str.call(timezone.value())},
    canonicalHostName: ${json_str.call(canonical.call())},
    auth: {
        secret: ${json_str.call(auth_secret.value())},
        refreshToken: {
            secret: ${json_str.call(refresh_secret.value())},
        },
    },
    ${if portal_on.call() {
    `patientPortal: {
        portalUrl: ${json_str.call("https://" + patient_portal_fqdn.value())},
    },`
    } else { "" }}
}`;

// API replica range: scales from 1 up to half the node's available threads,
// floored at 1 so deployments on a single-thread node still come up.
let api_max_replicas = || {
    let n = AVAILABLE_THREADS / 2;
    if n < 1 { 1 } else { n }
};

// ── Resources ─────────────────────────────────────────────────────────────────

let config_vol = app.volume("config").description("Tamanu configuration files");
config_vol.write("/production.json5", INITIAL_CONFIG);

// local.json5 is generated, and the volume is only seeded when it is created,
// so the rendered content is the key of a gate the parts that read it come
// after.
let config_gate = app.gate("config")
    // The rendered config needs an advertised URL. Until one is set the key
    // is the unit value, so evaluating the definition does not fail on it and
    // the gate's run says what is missing.
    .key(if canonical_url.is_set() || public_hostname.is_set() {
        local_json5.call()
    } else {
        ()
    })
    .run(|rt| {
        rt.write(config_vol, "/local.json5", local_json5.call());
    });

// Common options for every server-image container (api, tasks, fhir-*).
let configure_server = |c| c
    .image(server_image.call())
    .env("TZ", timezone.value())
    .mount("/config", config_vol)
    .mount("/var/run/postgresql", pg_socket);

// An unset hostname is a supported deployment: the app declares no ingress and
// the exported `web` service is fronted by a site ingress instead.
let web_svc = app.service("web").exported().http(80);
if public_hostname.is_set() {
    web_svc.ingress(public_hostname.value(), 443)
        .tls(Terminate.Https, Output.Http1)
        .redirect();
}

let portal_svc = if portal_on.call() {
    let s = app.service("portal-web").exported().http(80);
    s.ingress(patient_portal_fqdn.value(), 443)
        .tls(Terminate.Https, Output.Http1)
        .redirect();
    s
} else { () };

configure_server.call(app.deployment("api"))
    .description("API servers")
    .scale(1..=api_max_replicas.call())
    .arg(["startApi"])
    .after(config_gate)
    .http(3000, web_svc.route("/api"))
    .http(3000, web_svc.route("/v1"));

configure_server.call(app.deployment("tasks"))
    .description("Scheduled tasks worker")
    .on_update(OnUpdate.Replace)
    .arg(["startTasks"])
    .after(config_gate);

// FHIR workers (refresh + resolve) only run when explicitly enabled.
if fhir_on.call() {
    configure_server.call(app.deployment("fhir-refresh"))
        .description("FHIR materialised-resource refresh worker")
        .on_update(OnUpdate.Replace)
        .arg(["startFhirWorker", "--topics=fhir.refresh.allFromUpstream,fhir.refresh.entireResource,fhir.refresh.fromUpstream"])
        .after(config_gate);
    configure_server.call(app.deployment("fhir-resolve"))
        .description("FHIR resolver worker")
        .on_update(OnUpdate.Replace)
        .arg(["startFhirWorker", "--topics=fhir.resolver"])
        .after(config_gate);
}

let configure_frontend = |c| c
    .image(frontend_image.call())
    .cap_add("NET_BIND_SERVICE");

configure_frontend.call(app.deployment("frontend"))
    .description("Practitioner / Admin frontend")
    .http(3000, web_svc.route("/"));

if portal_on.call() {
    configure_frontend.call(app.deployment("patient-portal"))
        .description("Patient-facing portal (frontend)")
        .after(config_gate)
        .http(3000, portal_svc.route("/"));
}

// ── Helper closures (dynamic) ─────────────────────────────────────────────────

let start_intermediate = |rt| {
    let builder = configure_server.call(app.job())
        .description("Common job to do server-context tasks in actions")
        .cap_add("DAC_READ_SEARCH")
        .cap_add("CHOWN")
        .cap_add("DAC_OVERRIDE")
        .cap_add("FOWNER")
        .command(IDLE_CMD);

    let inter = rt.start(builder);
    inter.running(60);
    inter
};

// Drops a copy of the default.json5 into the config volume as
// `default.reference.json5` for operator convenience. The filename
// is so node-config doesn't pick it up as an active config layer.
let copy_default_config = |rt, inter| {
    rt.exec(inter, [
        "install", "-v", "-m", "0640",
        "config/default.json5",
        "/config/default.reference.json5"
    ]).ensure_success();
};

let upgrade_db = |rt, inter| {
    rt.exec(inter, ["node", "dist", "upgrade"]).ensure_success();
};

// Idempotent provisioning: create or refresh the database, the central role,
// and the read-only reporting roles. Connects via the local socket as the
// postgres superuser (`local trust` in the postgres app's pg_hba). No
// passwords are set on the roles — every connection from these apps comes
// in via the socket, where trust auth applies.
let setup_db = |rt| {
    let staging = app.volume().tmpfs().description("DB provisioning SQL, creates roles and the database");
    let dbn = db_name.value();
    let dbu = db_user.value();
    staging.write("/setup.sql", `
      DO $$ BEGIN
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${sql_escape.call(dbu)}') THEN
          EXECUTE format('CREATE ROLE %I LOGIN SUPERUSER', '${sql_escape.call(dbu)}');
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'raw') THEN
          EXECUTE 'CREATE ROLE raw LOGIN';
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'reporting') THEN
          EXECUTE 'CREATE ROLE reporting LOGIN';
        END IF;
      END $$;
    `);
    staging.write("/createdb.sql", `CREATE DATABASE "${sql_escape.call(dbn)}" OWNER "${sql_escape.call(dbu)}";`);

    let psql = rt.start(app.job()
        .description("DB provisioning")
        .image(psql_image)
        .writable_rootfs()
        .mount("/var/run/postgresql", pg_socket)
        .mount("/staging", staging)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command(IDLE_CMD));
    psql.running();
    rt.exec(psql, ["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-f", "/staging/setup.sql"]).ensure_success();
    rt.exec(psql, ["psql", "-d", "postgres", "-v", "ON_ERROR_STOP=1", "-f", "/staging/createdb.sql"]).exit_code();
    rt.stop(psql);
};

app.on_action("warm-images", |rt, param| {
    let target = if "target-version" in param && param["target-version"] != "" {
        param["target-version"]
    } else {
        version.value()
    };
    let warm = [
        app.job().image(`ghcr.io/beyondessential/tamanu-central:${target}`),
        app.job().image(`ghcr.io/beyondessential/tamanu-frontend:${target}`),
    ];
    if portal_on.call() {
        warm.push(app.job().image(`ghcr.io/beyondessential/tamanu-patient-portal:${target}`));
    }
    rt.warm_images(warm).ready();
}, #{
    description: "Pre-pull the images for a target version",
    params: #{
        "target-version": #{
            kind: "text",
            required: false,
            description: "Tamanu image tag to warm; defaults to the currently configured version",
        },
    },
});

// ── Gates ─────────────────────────────────────────────────────────────────────

let database_check = app.job("database-check")
    .description("Check that the Tamanu database exists on the postgres server")
    .image(psql_image)
    .writable_rootfs()
    .mount("/var/run/postgresql", pg_socket)
    .env("PGHOST", "/var/run/postgresql")
    .env("PGUSER", "postgres")
    .command(["sh", "-c",
        `psql -d postgres -tA -c "SELECT 1 FROM pg_database WHERE datname = '${sql_escape.call(db_name.value())}'" | grep -qx 1`
    ])
    .deadline(30);

// The server waits on a database the operator's postgres fulfils, so this gate
// runs until it is reachable and holds the migration back until then.
let database_gate = app.gate("database")
    .key([db_name.value(), db_user.value()])
    .check(database_check)
    .run(|rt| { setup_db.call(rt); });

// Everything that connects to the database is stopped while it migrates. The
// frontends do not, and roll once the migration has succeeded.
let db_workers = ["api", "tasks"];
let frontends = ["frontend"];
if fhir_on.call() {
    db_workers.push("fhir-refresh");
    db_workers.push("fhir-resolve");
}
if portal_on.call() {
    frontends.push("patient-portal");
}

let migrate_gate = app.gate("migrate")
    .key([version.value(), db_name.value()])
    .after([config_gate, database_gate])
    .exclusive(app.select(#{
        types: [ResourceType.Deployment],
        names: db_workers,
    }))
    .run(|rt| {
        // the intermediate runs the new server image, which holds the
        // migrations
        let inter = start_intermediate.call(rt);
        copy_default_config.call(rt, inter);
        upgrade_db.call(rt, inter);
        rt.stop(inter);
    });

for name in db_workers + frontends {
    app.deployment(name).after(migrate_gate);
}

// Nothing waits on this gate: a certificate that cannot be issued faults the
// app without holding the server back.
let hostnames = [];
if public_hostname.is_set() { hostnames.push(public_hostname.value()); }
if portal_on.call() { hostnames.push(patient_portal_fqdn.value()); }
app.gate("certificates")
    .key(hostnames)
    .run(|rt| { rt.warm_certs(app).ready(); });

app.on_action("provision-admin", |rt, param| {
    rt.start(configure_server.call(app.job())
        .description("Provision the initial admin user via Tamanu's `provision` subcommand")
        .mount("/staging", {
            let vol = app.volume().tmpfs().description("Initial admin provisioning");
            vol.write("/provision.json5", `{
                users: {
                    ${json_str.call(param["email"])}: {
                        role: "admin",
                        displayName: ${json_str.call(param["name"])},
                        password: ${json_str.call(param["password"])},
                    },
                },
            }`);
            vol
        })
        .arg(["provision", "/staging/provision.json5", "--skip-if-not-needed"])
    ).terminated_eventually().ensure_success();
}, #{
    description: "Provision the initial admin user, if the database has none",
    params: #{
        email: #{
            kind: "email",
            required: true,
            default_value: "admin@tamanu.app",
            description: "Email of the initial admin user",
        },
        name: #{
            kind: "text",
            required: true,
            default_value: "Superadmin",
            description: "Display name of the initial admin user",
        },
        password: #{
            kind: "password",
            required: true,
            description: "Password for the initial admin user",
        },
    },
});

app.on_action("reconfigure", |rt, _param| {
    rt.write(config_vol, "/local.json5", local_json5.call());
    rt.restart(app.deployment("api"));
    rt.restart(app.deployment("tasks"));
    if fhir_on.call() {
        rt.restart(app.deployment("fhir-refresh"));
        rt.restart(app.deployment("fhir-resolve"));
    }
}, #{ description: "Regenerate local.json5 from current params and restart the API + workers" });

app.on_shell("bash", |_rt, shell, _param| {
    shell.attach(configure_server.call(app.job()).command("bash"));
}, #{
    description: "Run a bash inside the central server image",
});

app.on_shell("node", |_rt, shell, _param| {
    shell.attach(configure_server.call(app.job()).arg(["shell"]));
}, #{
    description: "Run a bash inside the central server image",
});
