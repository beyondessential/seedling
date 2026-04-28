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
let pg_socket = app.external_volume("pg-socket");

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
let json_str = |s| {
    let escaped = s.replace("\\", "\\\\").replace("\"", "\\\"")
                   .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t");
    `"${escaped}"`
};

// production.json5: tamanu's main config. Operator-tunable params land here.
// Free-form overrides are not exposed as a script param: operators can edit
// the rendered file directly via a volume-shell on the `config` volume, which
// gives a proper editor instead of a single-line UI textbox.
let production_json = || {
    let portal_block = if portal_on.call() {
        `  patientPortal: {
    portalUrl: ${json_str.call("https://" + patient_portal_fqdn.value())},
  },
`
    } else { "" };

    `{
  // Generated by Seedling. Regenerate with the "reconfigure" action.
  countryTimeZone: ${json_str.call(timezone.value())},
  primaryTimeZone: ${json_str.call(timezone.value())},
  canonicalHostName: ${json_str.call(canonical.call())},
  auth: {
    secret: ${json_str.call(auth_secret.value())},
    refreshToken: {
      secret: ${json_str.call(refresh_secret.value())},
    },
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
  localisation: {
    data: {
      timeZone: ${json_str.call(timezone.value())},
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
${portal_block}}
`
};

// local.json5: deployment-local connection details. The DB connection uses
// the unix socket (host: "/var/run/postgresql"); postgres `local trust` auth
// makes the password field a no-op, so it isn't included.
let local_json = || `{
  // Generated by Seedling. Regenerate with the "reconfigure" action.
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
}
`;

// API replica range: scales from 1 up to half the node's available threads,
// floored at 1 so deployments on a single-thread node still come up.
let api_max_replicas = || {
    let n = AVAILABLE_THREADS / 2;
    if n < 1 { 1 } else { n }
};

// ── Resources ─────────────────────────────────────────────────────────────────

let config_vol = app.volume("config");

// Common env + mounts for every server-image container (api, tasks, fhir-*).
// Tamanu reads /config/production.json5 and /config/local.json5 by default
// when NODE_CONFIG_DIR points at /config.
let configure_server = |c| c
    .env("TZ", timezone.value())
    .env("NODE_CONFIG_DIR", "/config")
    .env("TAMANU_VERSION", version.value())
    .mount("/config", config_vol)
    .mount("/var/run/postgresql", pg_socket);

let web_svc = app.service("web")
    .exported(#{ description: "Tamanu central HTTPS endpoint (HTTP routing)" })
    .http(80);

// Guard with `is_set()` so the script also evaluates cleanly before the
// operator has supplied the hostname (e.g. on first registration / preview):
// `Service.ingress()` validates its hostname argument eagerly, which would
// otherwise throw on the empty default and leave the script unloadable.
if public_hostname.is_set() {
    web_svc.ingress(public_hostname.value(), 443)
        .tls(Terminate.Https, Output.Http1)
        .redirect();
}

// Patient portal: separate ingress on its own hostname when enabled.
let portal_svc = if portal_on.call() {
    let s = app.service("portal-web")
        .exported(#{ description: "Tamanu patient portal HTTPS endpoint" })
        .http(80);
    s.ingress(patient_portal_fqdn.value(), 443)
        .tls(Terminate.Https, Output.Http1)
        .redirect();
    s
} else { () };

// API workers handle /api/* and /v1/* on the central HTTP service.
configure_server.call(app.deployment("api")
    .image(server_image.call())
    .scale(1..api_max_replicas.call())
    .arg(["startApi"])
    .http(3000, web_svc.route("/api"))
    .http(3000, web_svc.route("/v1"))
);

// Scheduled tasks: cron-style internal jobs the central server fires.
configure_server.call(app.deployment("tasks")
    .image(server_image.call())
    .scale(1)
    .arg(["startTasks"])
);

// FHIR workers (refresh + resolve) only run when explicitly enabled. They
// drive the FHIR materialised-resource pipeline, which most deployments don't
// use day-to-day; gating them keeps the default footprint small.
if fhir_on.call() {
    configure_server.call(app.deployment("fhir-refresh")
        .image(server_image.call())
        .scale(1)
        .arg(["startFhirWorker", "--topics=fhir.refresh.allFromUpstream,fhir.refresh.entireResource,fhir.refresh.fromUpstream"])
    );
    configure_server.call(app.deployment("fhir-resolve")
        .image(server_image.call())
        .scale(1)
        .arg(["startFhirWorker", "--topics=fhir.resolver"])
    );
}

// Frontend: serves the React app. The bundled env.js tells the client which
// API to talk to; that's served from the same hostname so there's no CORS or
// URL configuration to thread through here. Fixed at 1 replica — rolling
// restarts handle blue/green rotation without needing a sibling.
app.deployment("frontend")
    .image(frontend_image.call())
    .scale(1)
    .env("TZ", timezone.value())
    .env("TAMANU_VERSION", version.value())
    .http(3000, web_svc.route("/"));

// Patient portal deployment + routing (when enabled).
if portal_on.call() {
    app.deployment("patient-portal")
        .image(portal_image.call())
        .scale(1)
        .env("TZ", timezone.value())
        .env("TAMANU_VERSION", version.value())
        .mount("/config", config_vol)
        .mount("/var/run/postgresql", pg_socket)
        .http(3000, portal_svc.route("/api"))
        .http(3000, portal_svc.route("/v1"))
        .http(3000, portal_svc.route("/"));
}

// ── Helper closures (dynamic) ─────────────────────────────────────────────────

// Locate the default.json5 the image ships and drop a copy into the config
// volume as `default.reference.json5` for operator inspection. Renamed so
// node-config doesn't pick it up as an active config layer (node-config only
// loads files named default / <NODE_ENV> / local / etc., never *.reference.*).
let copy_image_defaults = |rt| {
    rt.start(app.job()
        .image(server_image.call())
        .writable_rootfs()
        .cap_add("DAC_READ_SEARCH")
        .mount("/config", config_vol)
        .command(["sh", "-c", `
set -eu
src=$(find / -path /proc -prune -o -path /sys -prune -o -path /config -prune -o \
    -type f -name 'default.json5' -print 2>/dev/null | head -1)
if [ -z "$src" ]; then
  echo "no default.json5 found in image" >&2
  exit 0
fi
echo "copying $src -> /config/default.reference.json5"
install -m 0640 "$src" /config/default.reference.json5
`])
    ).terminated(120).ensure_success();
};

// Render production.json5 + local.json5 into the config volume via a staging
// tmpfs so the rendered content comes from the script (which has the param
// values) rather than being built up with shell quoting.
let render_config = |rt| {
    let staging = app.volume().tmpfs();
    staging.write("/production.json5", production_json.call());
    staging.write("/local.json5", local_json.call());

    rt.start(app.job()
        .image(server_image.call())
        .writable_rootfs()
        .cap_add("CHOWN")
        .cap_add("DAC_OVERRIDE")
        .cap_add("FOWNER")
        .mount("/staging", staging)
        .mount("/config", config_vol)
        .command(["sh", "-c",
            "install -m 0640 /staging/production.json5 /config/production.json5 && " +
            "install -m 0640 /staging/local.json5 /config/local.json5"
        ])
    ).terminated(60).ensure_success();
};

// Idempotent provisioning: create or refresh the database, the central role,
// and the read-only reporting roles. Connects via the local socket as the
// postgres superuser (`local trust` in the postgres app's pg_hba). No
// passwords are set on the roles — every connection from these apps comes
// in via the socket, where trust auth applies.
let provision_db = |rt| {
    let staging = app.volume().tmpfs();
    let dbn = db_name.value();
    let dbu = db_user.value();
    let escape = |s| s.replace("'", "''");
    staging.write("/setup.sql", `
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${escape.call(dbu)}') THEN
    EXECUTE format('CREATE ROLE %I LOGIN', '${escape.call(dbu)}');
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'raw') THEN
    EXECUTE 'CREATE ROLE raw LOGIN';
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'reporting') THEN
    EXECUTE 'CREATE ROLE reporting LOGIN';
  END IF;
END $$;
`);
    staging.write("/createdb.sql", `SELECT 'CREATE DATABASE "${escape.call(dbn)}" OWNER "${escape.call(dbu)}"' WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '${escape.call(dbn)}')\\gexec
`);

    rt.start(app.job()
        .image(psql_image)
        .writable_rootfs()
        .mount("/var/run/postgresql", pg_socket)
        .mount("/staging", staging)
        .env("PGHOST", "/var/run/postgresql")
        .env("PGUSER", "postgres")
        .command(["sh", "-c",
            "psql -d postgres -v ON_ERROR_STOP=1 -f /staging/setup.sql && " +
            "psql -d postgres -v ON_ERROR_STOP=1 -f /staging/createdb.sql"
        ])
    ).terminated(120).ensure_success();
};

// Run the schema migration against the current database using the current
// image. Tamanu's `migrate` subcommand is idempotent — running it when the
// schema is already at the target revision is a no-op.
let migrate_db = |rt| {
    rt.start(app.job()
        .image(server_image.call())
        .writable_rootfs()
        .env("TZ", timezone.value())
        .env("NODE_CONFIG_DIR", "/config")
        .env("TAMANU_VERSION", version.value())
        .mount("/config", config_vol)
        .mount("/var/run/postgresql", pg_socket)
        .arg(["migrate"])
    ).terminated_eventually().ensure_success();
};

// Provision the initial admin user via Tamanu's `provision` subcommand. Builds
// a minimal provisioning JSON5 file with a single `users` entry and feeds it
// to the central server. `--skip-if-not-needed` makes the command a no-op if
// any user already exists, so this is safe to re-run on subsequent installs.
let provision_admin = |rt, email, display_name, password| {
    let staging = app.volume().tmpfs();
    staging.write("/provision.json5", `{
  users: {
    ${json_str.call(email)}: {
      role: "admin",
      displayName: ${json_str.call(display_name)},
      password: ${json_str.call(password)},
    },
  },
}
`);
    rt.start(app.job()
        .image(server_image.call())
        .writable_rootfs()
        .env("TZ", timezone.value())
        .env("NODE_CONFIG_DIR", "/config")
        .env("TAMANU_VERSION", version.value())
        .mount("/config", config_vol)
        .mount("/var/run/postgresql", pg_socket)
        .mount("/staging", staging)
        .arg(["provision", "/staging/provision.json5", "--skip-if-not-needed"])
    ).terminated_eventually().ensure_success();
};

// ── Install ───────────────────────────────────────────────────────────────────

app.on_install(|rt, param| {
    copy_image_defaults.call(rt);
    render_config.call(rt);
    provision_db.call(rt);
    migrate_db.call(rt);
    provision_admin.call(rt,
        param["admin-email"],
        param["admin-display-name"],
        param["admin-password"]);

    rt.warm_certs(app).ready();
    rt.start(app).ready(300);
}, #{
    params: #{
        "admin-email": #{
            kind: "email",
            required: true,
            description: "Email of the initial admin user",
        },
        "admin-display-name": #{
            kind: "text",
            required: true,
            description: "Display name of the initial admin user",
        },
        "admin-password": #{
            kind: "password",
            required: true,
            description: "Password for the initial admin user",
        },
    },
});

// ── Maintenance actions ───────────────────────────────────────────────────────

app.on_action("reconfigure", |rt, _param| {
    render_config.call(rt);
    rt.restart(app.deployment("api"));
    rt.restart(app.deployment("tasks"));
    if fhir_on.call() {
        rt.restart(app.deployment("fhir-refresh"));
        rt.restart(app.deployment("fhir-resolve"));
    }
    if portal_on.call() {
        rt.restart(app.deployment("patient-portal"));
    }
}, #{ description: "Regenerate production.json5 + local.json5 from current params and restart the API + workers" });

app.on_action("migrate", |rt, _param| {
    migrate_db.call(rt);
}, #{ description: "Run pending schema migrations against the central database" });

app.on_action("provision-db", |rt, _param| {
    provision_db.call(rt);
}, #{ description: "Create or refresh the database, central role, and reporting roles based on current params" });

app.on_action("copy-image-defaults", |rt, _param| {
    copy_image_defaults.call(rt);
}, #{ description: "Copy the image's default.json5 to /config/default.reference.json5 for operator inspection" });

// Pre-pull the container images for a target version into local storage so
// the actual version change can rotate without paying registry-fetch latency
// inside the migration window. The images are pinned to this app until they
// are observed in use, matching what `version.on_change` does at upgrade time.
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
    rt.warm_images(warm).ready(600);
}, #{
    description: "Pre-pull the central, frontend, and (if enabled) patient-portal images for a target version",
    params: #{
        "target-version": #{
            kind: "text",
            required: false,
            description: "Tamanu image tag to warm (e.g. v2.10.0); defaults to the currently configured version",
        },
    },
});

app.on_action("restart-api", |rt, _param| {
    rt.restart(app.deployment("api"));
}, #{ description: "Restart the API workers" });

app.on_action("restart-frontend", |rt, _param| {
    rt.restart(app.deployment("frontend"));
}, #{ description: "Restart the frontend" });

app.on_action("restart-workers", |rt, _param| {
    rt.restart(app.deployment("tasks"));
    if fhir_on.call() {
        rt.restart(app.deployment("fhir-refresh"));
        rt.restart(app.deployment("fhir-resolve"));
    }
}, #{ description: "Restart the tasks and FHIR workers" });

// ── Shells ────────────────────────────────────────────────────────────────────

// Equivalent of the ansible `tamanu-run` script: invoke any subcommand of the
// central image (e.g. `seed`, `report`, `dump`) against the current config
// and database.
app.on_shell("run", |_rt, shell, param| {
    let cmd = if "command" in param && param["command"] != "" {
        [param["command"]]
    } else {
        ["bash"]
    };
    shell.attach(app.job()
        .image(server_image.call())
        .writable_rootfs()
        .env("TZ", timezone.value())
        .env("NODE_CONFIG_DIR", "/config")
        .env("TAMANU_VERSION", version.value())
        .mount("/config", config_vol)
        .mount("/var/run/postgresql", pg_socket)
        .command(cmd));
}, #{
    description: "Run a subcommand inside the central server image (defaults to bash)",
    params: #{
        command: #{
            kind: "text",
            required: false,
            description: "Subcommand to invoke (e.g. migrate, seed, report). Leave blank for an interactive shell.",
        },
    },
});

// ── Version changes ───────────────────────────────────────────────────────────

// API + worker deployments are stopped so the migration has exclusive database
// access; the frontend keeps serving cached pages through the brief outage.
// `rt.start(app)` afterwards rolls the frontend (rolling update) and brings
// the API + workers back up on the new image.
version.on_change(|rt, old| {
    let warm = [
        app.job().image(server_image.call()),
        app.job().image(frontend_image.call()),
    ];
    if portal_on.call() {
        warm.push(app.job().image(portal_image.call()));
    }
    rt.warm_images(warm).ready(600);

    copy_image_defaults.call(rt);
    render_config.call(rt);

    rt.stop(old.select(#{
        types: [ResourceType.Deployment],
        names: ["api", "tasks", "fhir-refresh", "fhir-resolve", "patient-portal"],
    }));

    migrate_db.call(rt);

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

canonical_url.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
timezone.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
auth_secret.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
refresh_secret.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
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
