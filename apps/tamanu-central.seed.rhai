// Tamanu Central server.
//
// Runs the Tamanu central API, scheduled tasks, FHIR workers, and a frontend,
// plus an optional patient portal. Connects to PostgreSQL via the unix socket
// exported by the seedling-managed `postgres` app: the operator binds this
// app's `pg-socket` external volume to that socket. The same socket is used
// to provision the `tamanu` database and the read-only `raw` / `reporting`
// users at install time.
//
// Routing model:
//   - `/api/*` and `/v1/*` → API workers (port 3000)
//   - `/*` → frontend (port 3000)
//   - patient portal (when enabled) is a separate hostname / ingress
//
// Version upgrades:
//   - The frontend is rotated first (rolling) so cached HTML keeps working.
//   - All API + worker deployments are stopped, the database migration job
//     runs against the new image, and they come back up on the new tag.

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
    .default_value("UTC")
    .description("IANA timezone, e.g. Pacific/Auckland");

let country_name = app.param("country-name")
    .required(false)
    .default_value("Utopia")
    .description("Country name for localisation");

let country_alpha2 = app.param("country-alpha2")
    .required(false)
    .default_value("UT")
    .description("ISO 3166-1 alpha-2 country code");

let country_alpha3 = app.param("country-alpha3")
    .required(false)
    .default_value("UTO")
    .description("ISO 3166-1 alpha-3 country code");

let auth_secret = app.param("auth-secret")
    .kind("password")
    .required(true)
    .description("JWT signing secret used by the auth subsystem");

let refresh_secret = app.param("refresh-secret")
    .kind("password")
    .required(true)
    .description("Refresh-token signing secret used by the auth subsystem");

let db_name = app.param("db-name")
    .required(false)
    .default_value("tamanu")
    .description("PostgreSQL database name");

let db_user = app.param("db-user")
    .required(false)
    .default_value("tamanu")
    .description("PostgreSQL role the central server connects as");

let db_password = app.param("db-password")
    .kind("password")
    .required(true)
    .description("PostgreSQL password for the central server's role");

let raw_password = app.param("raw-password")
    .kind("password")
    .required(true)
    .description("Password for the 'raw' read-only reporting role");

let reporting_password = app.param("reporting-password")
    .kind("password")
    .required(true)
    .description("Password for the 'reporting' read-only reporting role");

let api_replicas = app.param("api-replicas")
    .required(false)
    .default_value("2")
    .description("Number of central API replicas");

let frontend_replicas = app.param("frontend-replicas")
    .required(false)
    .default_value("2")
    .description("Number of frontend replicas");

let patient_portal_fqdn = app.param("patient-portal-fqdn")
    .required(false)
    .description("Public FQDN for the patient portal; if set, the patient portal subsystem is enabled");

let extra_config = app.param("extra-config")
    .kind("multiline")
    .required(false)
    .description("Additional fields appended into production.json5 (advanced; merged at the top level)");

// ── External resources ────────────────────────────────────────────────────────

// Postgres unix socket. The operator maps this to the `socket` volume
// exported by the postgres app via the external-volume mapping table.
let pg_socket = app.external_volume("pg-socket");

// ── Helpers ───────────────────────────────────────────────────────────────────

let server_image = || `ghcr.io/beyondessential/tamanu-central:${version.value()}`;
let frontend_image = || `ghcr.io/beyondessential/tamanu-frontend:${version.value()}`;
let portal_image = || `ghcr.io/beyondessential/tamanu-patient-portal:${version.value()}`;

let canonical = || if canonical_url.is_set() {
    canonical_url.value()
} else {
    `https://${public_hostname.value()}`
};

let portal_on = || patient_portal_fqdn.is_set();

// JSON-string-encode a value with the few escapes JSON5 requires.
let json_str = |s| {
    let escaped = s.replace("\\", "\\\\").replace("\"", "\\\"")
                   .replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t");
    `"${escaped}"`
};

// production.json5: tamanu's main config. Operator-tunable params land here.
let production_json = || {
    let portal_block = if portal_on.call() {
        `  patientPortal: {
    portalUrl: ${json_str.call("https://" + patient_portal_fqdn.value())},
  },
`
    } else { "" };

    let extra = if extra_config.is_set() {
        `  // Operator-supplied overrides:
${extra_config.value()}
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
      country: {
        name: ${json_str.call(country_name.value())},
        "alpha-2": ${json_str.call(country_alpha2.value())},
        "alpha-3": ${json_str.call(country_alpha3.value())},
      },
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
${portal_block}${extra}}
`
};

// local.json5: deployment-local connection details. Tamanu reads this
// alongside production.json5 (later loads override earlier ones).
let local_json = || `{
  // Generated by Seedling. Regenerate with the "reconfigure" action.
  db: {
    name: ${json_str.call(db_name.value())},
    verbose: false,
    username: ${json_str.call(db_user.value())},
    password: ${json_str.call(db_password.value())},
    host: "/var/run/postgresql",
    port: null,
    migrateOnStartup: false,
    reportSchemas: {
      connections: {
        raw: {
          username: "raw",
          password: ${json_str.call(raw_password.value())},
        },
        reporting: {
          username: "reporting",
          password: ${json_str.call(reporting_password.value())},
        },
      },
    },
  },
}
`;

let parse_int_param = |p| {
    let v = parse_int(p.value());
    if v < 1 { throw `${p.value()} is not a positive integer` }
    v
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
    .scale(parse_int_param.call(api_replicas))
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

// FHIR workers: refresh and resolve queues run as separate processes so
// either can be scaled or restarted without affecting the other.
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

// Frontend: serves the React app. The bundled env.js tells the client which
// API to talk to; that's served from the same hostname so there's no CORS or
// URL configuration to thread through here.
app.deployment("frontend")
    .image(frontend_image.call())
    .scale(parse_int_param.call(frontend_replicas))
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

// Set up the central database, role, and read-only reporting roles. Idempotent:
// CREATE statements are guarded so re-running the action is harmless. Connects
// over the local socket as the postgres superuser (trust auth on local).
let provision_db = |rt| {
    let staging = app.volume().tmpfs();
    let dbn = db_name.value();
    let dbu = db_user.value();
    let escape = |s| s.replace("'", "''");
    staging.write("/setup.sql", `
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${escape.call(dbu)}') THEN
    EXECUTE format('CREATE ROLE %I LOGIN PASSWORD %L', '${escape.call(dbu)}', '${escape.call(db_password.value())}');
  ELSE
    EXECUTE format('ALTER ROLE %I WITH LOGIN PASSWORD %L', '${escape.call(dbu)}', '${escape.call(db_password.value())}');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'raw') THEN
    EXECUTE format('CREATE ROLE raw LOGIN PASSWORD %L', '${escape.call(raw_password.value())}');
  ELSE
    EXECUTE format('ALTER ROLE raw WITH LOGIN PASSWORD %L', '${escape.call(raw_password.value())}');
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'reporting') THEN
    EXECUTE format('CREATE ROLE reporting LOGIN PASSWORD %L', '${escape.call(reporting_password.value())}');
  ELSE
    EXECUTE format('ALTER ROLE reporting WITH LOGIN PASSWORD %L', '${escape.call(reporting_password.value())}');
  END IF;
END $$;
`);
    staging.write("/createdb.sql", `SELECT 'CREATE DATABASE "${escape.call(dbn)}" OWNER "${escape.call(dbu)}"' WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '${escape.call(dbn)}')\\gexec
`);

    rt.start(app.job()
        .image(server_image.call())
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

// ── Install ───────────────────────────────────────────────────────────────────

app.on_install(|rt, _param| {
    copy_image_defaults.call(rt);
    render_config.call(rt);
    provision_db.call(rt);
    migrate_db.call(rt);

    rt.warm_certs(app).ready();
    rt.start(app).ready(300);
}, #{});

// ── Maintenance actions ───────────────────────────────────────────────────────

app.on_action("reconfigure", |rt, _param| {
    render_config.call(rt);
    rt.restart(app.deployment("api"));
    rt.restart(app.deployment("tasks"));
    rt.restart(app.deployment("fhir-refresh"));
    rt.restart(app.deployment("fhir-resolve"));
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

app.on_action("restart-api", |rt, _param| {
    rt.restart(app.deployment("api"));
}, #{ description: "Restart the API workers" });

app.on_action("restart-frontend", |rt, _param| {
    rt.restart(app.deployment("frontend"));
}, #{ description: "Restart the frontend" });

app.on_action("restart-workers", |rt, _param| {
    rt.restart(app.deployment("tasks"));
    rt.restart(app.deployment("fhir-refresh"));
    rt.restart(app.deployment("fhir-resolve"));
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
    rt.restart(app.deployment("fhir-refresh"));
    rt.restart(app.deployment("fhir-resolve"));
    if portal_on.call() {
        rt.restart(app.deployment("patient-portal"));
    }
};

canonical_url.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
timezone.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
country_name.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
country_alpha2.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
country_alpha3.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
auth_secret.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
refresh_secret.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });
extra_config.on_change(|rt, _old| { render_config.call(rt); restart_servers.call(rt); });

// DB credential changes: re-provision (so the postgres-side passwords get
// updated to match) and re-render the local config that holds them.
db_password.on_change(|rt, _old| {
    provision_db.call(rt);
    render_config.call(rt);
    restart_servers.call(rt);
});
raw_password.on_change(|rt, _old| {
    provision_db.call(rt);
    render_config.call(rt);
    restart_servers.call(rt);
});
reporting_password.on_change(|rt, _old| {
    provision_db.call(rt);
    render_config.call(rt);
    restart_servers.call(rt);
});

// Replica count changes: the deployment definition is already updated by the
// time on_change fires, so just push the new desired state.
api_replicas.on_change(|rt, _old| { rt.start(app.deployment("api")); });
frontend_replicas.on_change(|rt, _old| { rt.start(app.deployment("frontend")); });
