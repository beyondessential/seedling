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
// Version upgrades:
//   - All API + worker deployments are stopped, the database migration job
//     runs against the new image, and they come back up on the new tag.
//   - The frontend keeps serving cached pages through the brief outage and
//     rolls to the new tag as part of `rt.start(app)` afterwards.

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
    .required(true)
    .description("Public FQDN for the central server, served over HTTPS");

let canonical_url = app.param("canonical-url")
    .required(false)
    .description("URL the central server advertises in API responses; defaults to https://<public-hostname>");

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

let db_name = app.param("db-name")
    .required(false)
    .default_value("tamanu")
    .description("PostgreSQL database name");

let db_user = app.param("db-user")
    .required(false)
    .default_value("tamanu")
    .description("PostgreSQL role the central server connects as (uses local-socket trust auth, no password)");

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

let canonical = || if canonical_url.is_set() {
    canonical_url.value()
} else {
    `https://${public_hostname.value()}`
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

// Common options for every server-image container (api, tasks, fhir-*).
let configure_server = |c| c
    .image(server_image.call())
    .env("TZ", timezone.value())
    .mount("/config", config_vol)
    .mount("/var/run/postgresql", pg_socket);

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
    .scale(1..api_max_replicas.call())
    .arg(["startApi"])
    .http(3000, web_svc.route("/api"))
    .http(3000, web_svc.route("/v1"));

configure_server.call(app.deployment("tasks"))
    .description("Scheduled tasks worker")
    .on_update(OnUpdate.Replace)
    .arg(["startTasks"]);

// FHIR workers (refresh + resolve) only run when explicitly enabled.
if fhir_on.call() {
    configure_server.call(app.deployment("fhir-refresh"))
        .description("FHIR materialised-resource refresh worker")
        .on_update(OnUpdate.Replace)
        .arg(["startFhirWorker", "--topics=fhir.refresh.allFromUpstream,fhir.refresh.entireResource,fhir.refresh.fromUpstream"]);
    configure_server.call(app.deployment("fhir-resolve"))
        .description("FHIR resolver worker")
        .on_update(OnUpdate.Replace)
        .arg(["startFhirWorker", "--topics=fhir.resolver"]);
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
    // Mutate-in-place: see the comment on `json_str` for why a chain doesn't work.
    let escape = |s| { s.replace("'", "''"); s };
    staging.write("/setup.sql", `
      DO $$ BEGIN
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${escape.call(dbu)}') THEN
          EXECUTE format('CREATE ROLE %I LOGIN SUPERUSER', '${escape.call(dbu)}');
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'raw') THEN
          EXECUTE 'CREATE ROLE raw LOGIN';
        END IF;
        IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'reporting') THEN
          EXECUTE 'CREATE ROLE reporting LOGIN';
        END IF;
      END $$;
    `);
    staging.write("/createdb.sql", `CREATE DATABASE "${escape.call(dbn)}" OWNER "${escape.call(dbu)}";`);

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

let warmup = app.on_action("warm-images", |rt, param| {
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

app.on_install(|rt, params| {
    // what
    rt.warm_certs(app);
    warmup.invoke();

    setup_db.call(rt);

    let inter = start_intermediate.call(rt);
    copy_default_config.call(rt, inter);
    rt.write(app.volume("config"), "/local.json5", local_json5.call());
    upgrade_db.call(rt, inter);

    rt.start(configure_server.call(app.job())
        .description("Provision the initial admin user via Tamanu's `provision` subcommand")
        .mount("/staging", {
            let vol = app.volume().tmpfs().description("Initial admin provisioning");
            vol.write("/provision.json5", `{
                users: {
                    ${json_str.call(params["email"])}: {
                        role: "admin",
                        displayName: ${json_str.call(params["name"])},
                        password: ${json_str.call(params["password"])},
                    },
                },
            }`);
            vol
        })
        .arg(["provision", "/staging/provision.json5", "--skip-if-not-needed"])
    ).terminated_eventually().ensure_success();

    // then seedling will do normal bringup
}, #{
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
    rt.write(app.volume("config"), "/config/local.json5", local_json5.call());
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

// ── Version changes ───────────────────────────────────────────────────────────

version.on_change(|rt, old| {
    // warming images keeps the rest of the upgrade fast and also gates on
    // all the images' availability *before* we start taking things down.
    warmup.invoke();

    // start intermediate in the new server image early
    let inter = start_intermediate.call(rt);

    // bring down everything non-essential
    let old_tasks = rt.stop(old.select(#{
        types: [ResourceType.Deployment],
        names: ["tasks", "fhir-refresh", "fhir-resolve"],
    }));

    // swap the frontends.
    // we could hard-swap the traffic over, but it's more gracious
    // to get the new running, tell the old to stop gracefully,
    // and only then hard-kill the old. the frontends are caddy,
    // which does a soft-down on first INT, then hard on second.
    let oldfront = old.select(#{
        types: [ResourceType.Deployment],
        names: ["frontend", "patient-portal"],
    });
    let newfront = rt.start(app.select(#{
        types: [ResourceType.Deployment],
        names: ["frontend", "patient-portal"],
    }));
    newfront.running();
    rt.signal(oldfront, "SIGINT");
    newfront.ready();
    rt.signal(oldfront, "SIGINT");
    rt.stop(oldfront);

    // in future we'll want to do this instead of stopping everything:
    // rt.stop(old.select(#{ types: [ResourceType.Deployment] }));
    // rt.write(app.volume("config"), "/config/local.json5", local_json5.call(#{
    //     readonly: true
    // }));
    // rt.start(old.deployment("api"));

    // stop everything that's currently touching the database
    rt.stop(old);

    // mid-upgrade tasks
    copy_default_config.call(rt, inter);
    rt.write(app.volume("config"), "/config/local.json5", local_json5.call());
    upgrade_db.call(rt, inter);

    //

    rt.start(app).ready(300);
});

// ── Config parameter changes ──────────────────────────────────────────────────
// production.json5 and local.json5 are loaded at process startup. Re-render
// on any change and restart the API + workers (the frontend doesn't read these).

let restart_servers = |rt| {
    rt.restart(app.deployment("api"));
    rt.restart(app.deployment("tasks"));
    if fhir_on.call() {
        rt.restart(app.deployment("fhir-refresh"));
        rt.restart(app.deployment("fhir-resolve"));
    }
    if portal_on.call() {
        rt.restart(app.deployment("patient-portal"));
    }
};

// Toggling the FHIR workers: bring them up or tear them down to match the new
// param value. Re-render config first so any related side-channel state is
// consistent, though FHIR workers don't currently read FHIR-specific config.
fhir_enabled.on_change(|rt, old| {
    if fhir_on.call() {
        rt.start(app.select(#{
            types: [ResourceType.Deployment],
            names: ["fhir-refresh", "fhir-resolve"],
        })).ready();
    } else {
        rt.stop(old.select(#{
            types: [ResourceType.Deployment],
            names: ["fhir-refresh", "fhir-resolve"],
        }));
    }
});
