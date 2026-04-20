# Writing a BSL Script

BSL (BES Seedling Language) is the scripting language used to define and manage applications on a Seedling node. It describes not just what to run, but how to upgrade it, how operators can interact with it, and what happens on first install.

The full language specification is at [`docs/spec/language.md`](spec/language.md). This document is a practical guide to writing a script, with a reference summary and annotated examples. When this document and the spec disagree, the spec wins.

## Language fundamentals

BSL is written in [Rhai](https://rhai.rs). Rhai is a scripting language with Rust-like syntax: `let`, closures `|args| { body }`, string interpolation with backticks, object maps `#{ key: value }`, and arrays `[a, b, c]`. You do not need to know Rust to write BSL.

A script runs in a fresh scope every time it is evaluated. The `app` global is pre-injected; everything else must be defined or computed.

### Names

Most resources take a name. Valid names are 3–63 characters, ASCII alphanumeric with hyphens, must not start with a digit or hyphen, and must not end with a hyphen. Regex: `^[a-zA-Z][a-zA-Z0-9-]{1,60}[a-zA-Z0-9]$`.

### Builders

Most resource methods return a builder: an object you configure by chaining calls, then hand off to something else. The builder methods modify the resource in place and return it for the next call.

```rhai
let svc = app.service("web")   // creates the service, returns builder
    .http(80);                 // specialises to HTTP, returns the HttpService
```

### Static vs. dynamic context

Code at the **top level** of the script is the _static context_: it runs every time the script is evaluated and defines the app's steady state. Resources defined here are _static_.

Code inside action or shell closures is the _dynamic context_: it runs when an operator invokes the action. Resources defined here are _dynamic_ — they are created fresh for each invocation and cleaned up when the action ends.

```rhai
let svc = app.service("web");          // static: always exists
app.on_action("migrate", |rt, _p| {
    let job = app.job();               // dynamic: exists only during this action
    rt.start(job).terminated();
});
```

In the dynamic context, `app.resource_method(name)` returns a _reference_ to an existing static resource by that name, not a new one.

## Quick reference

### Resources

| Method | Returns | Notes |
|---|---|---|
| `app.param(name)` | `Param` | Reads a value set by the control plane |
| `app.service(name)` | Service builder | Network endpoint |
| `app.deployment(name)` | Deployment builder | Long-running container workload |
| `app.job(name?)` | Job builder | One-off container workload; `name` optional in dynamic context |
| `app.volume(name?)` | Volume builder | Persistent directory; `name` optional |
| `app.external_volume(name)` | `ExternalVolume` | Volume provided by the control plane |
| `app.on_action(name, fn, opts?)` | `Action` | Operator-invokable action |
| `app.on_shell(name, fn, opts?)` | `Action` | Interactive shell for operators |
| `app.on_start(fn, opts?)` | `Action` | Startup lifecycle hook |
| `app.on_install(fn, config?)` | `Action` | First-install hook with param schema |

### Service

```rhai
let svc = app.service("name");
let http_svc = svc.http(80);          // specialise; port defaults to 80
let route = http_svc.route("/api");   // HTTP path prefix routing

let ing = svc.ingress("host.example.com", 443)
    .http()                           // terminate HTTPS → HTTP/1.1
    .redirect();                      // redirect port 80 → 443
let ing2 = ing.http2();               // terminate HTTPS → HTTP/2 (h2c)
let ing3 = svc.ingress("h.example.com", 443).tls();  // terminate TLS (non-HTTP)
```

### Deployment / Job (Container + Pod)

Both implement Container and Pod:

```rhai
app.deployment("worker")
    .image("ghcr.io/example/app:v1.2.3")
    .command("serve")                 // override entrypoint
    .arg(["--port", "8080"])          // append args
    .env("WORKERS", "4")
    .env([#{ name: "FOO", value: "bar" }, ...])
    .memory("512m")                   // k / m / g suffixes
    .cpus(0.5)
    .pids_limit(128)
    .workdir("/app")
    .writable_rootfs()                // opt out of read-only root
    .cap_add("NET_BIND_SERVICE")
    .mount("/data", vol)              // bind volume into container
    .mount(svc.port(5432))            // bind service port into network namespace
    .http(8080, http_svc.route("/"))  // expose pod port to HTTP route
    .tcp(5432, db_svc.port(5432))     // expose pod port to TCP service
    .udp(5353, dns_svc)
    .scale(2)                         // fixed replicas (Deployment only)
    .scale(1..8)                      // scalable range (Deployment only)
    .on_update(OnUpdate.Rolling)      // default; or OnUpdate.Replace
    .on_exit(OnExit.Restart);         // default for Deployment; Terminate for Job

app.job("migrate")
    .image("ghcr.io/example/app:v1.2.3")
    .deadline(300);                   // seconds; no deadline = runs indefinitely
```

### Volume

```rhai
let vol = app.volume("data")
    .exported(#{ description: "App data" })  // advertise to control plane
    .readonly()                              // read-only mount
    .tmpfs()                                 // RAM-backed; does not survive reboot
    .write("/config.json", contents);        // pre-populate a file
```

### Parameter

```rhai
let version = app.param("version")
    .kind("text")           // text | email | password | weak-password
    .required(false)
    .default_value("latest")
    .description("Docker image tag to deploy");

if version.is_set() { version.value() } else { "latest" }

version.on_change(|rt, old| {
    // old is the App state at the previous generation
    // use it to stop old resources before starting new ones
});
```

### Runtime (rt)

Available inside action closures:

```rhai
rt.start(resources)             // schedule resources, returns Started
rt.stop(resources, deadline?)   // unschedule and block until terminated
rt.query(resources)             // returns Started without scheduling
rt.warm_certs(resources)        // pre-provision TLS certs for ingresses

// Started methods — all block until state is reached (deadline in seconds):
started.scheduled(deadline?)
started.running(deadline?)
started.ready(deadline?)
started.terminated()            // returns Termination

termination.ensure_success()    // throws if the resource failed
```

### Collections

```rhai
col(val)                         // coerce anything to Collection
col.one()                        // any single resource (or null)
col.only(other)                  // intersection
col.except(other)                // difference
col.select(#{ types: [ResourceType.Deployment], names: ["api", "worker"] })
col.select(#{ name_patterns: ["worker-*"] })
```

### Constants

| Name | Type | Description |
|---|---|---|
| `AVAILABLE_THREADS` | int | Compute threads available |
| `DEFAULT_DEADLINE` | int | Default deadline in seconds |
| `OnUpdate.Rolling` | enum | Start new, stop old (default) |
| `OnUpdate.Replace` | enum | Stop all old first, then start new |
| `OnTerminate.Recreate` | enum | Always recreate terminated containers |
| `OnExit.Restart` | enum | Always restart on exit |
| `OnExit.Terminate` | enum | Stop container on exit |
| `OnExit.RestartOnFailure` | enum | Restart on non-zero exit, terminate otherwise |
| `ResourceType.{Parameter,Service,HttpService,Ingress,Deployment,Job,Volume,ExternalVolume,Action}` | enum | For use with `col.select()` |

## Annotated example

```rhai
// Parameters come from the control plane; operators set them via the UI.
let version = app.param("version")
    .required(true)
    .description("Docker image tag to deploy");

// A closure to build the image reference, called later when needed.
let image = || `ghcr.io/example/myapp:${version.value()}`;

// Services define network endpoints.
// Specialising as .http() enables URL-prefix routing.
let web_svc = app.service("web")
    .ingress("myapp.example.com", 443)
    .http()               // terminate HTTPS, re-emit HTTP/1.1
    .redirect()           // redirect HTTP on port 80 to 443
    .service()            // navigate back from ingress to the service
    .http(80);            // specialise the service for path routing

// A persistent volume, exported so operators and other apps can reference it.
let data = app.volume("data").exported(#{ description: "Application data" });

// A deployment is a long-running container workload.
app.deployment("api")
    .image(image.call())
    .scale(1..4)
    .mount("/data", data)
    .http(8080, web_svc.route("/api"))
    .http(8080, web_svc.route("/"));

// on_start controls how the app is brought up.
// Default (if omitted) is: rt.start(app);
app.on_start(|rt, _param| {
    rt.warm_certs(app).ready();  // pre-provision TLS before routing traffic
    rt.start(app).ready();
});

// Actions are available to operators via the UI or CLI.
// The closure runs when an operator invokes the action.
app.on_action("migrate", |rt, _param| {
    // Jobs defined inside actions are dynamic: created fresh per invocation.
    let job = app.job()
        .image(image.call())
        .command(["migrate", "--run"])
        .mount("/data", data);
    rt.start(job).terminated().ensure_success();
}, #{
    description: "Run database migrations",
});

// Shells give operators an interactive terminal into the running app.
app.on_shell("bash", |_rt, shell, _param| {
    shell.attach(app.job()
        .image(image.call())
        .mount("/data", data)
        .command("bash"));
}, #{
    description: "Bash shell in the app container",
});

// on_install only runs on first install, and can prompt for secrets.
app.on_install(|rt, param| {
    let seed_vol = app.volume();
    seed_vol.write("/seed.json", `{"admin_password":"${param.admin_password}"}`);
    let seed_job = app.job()
        .image(image.call())
        .command(["seed-db"])
        .mount("/seed", seed_vol)
        .mount("/data", data);
    rt.start(seed_job).terminated().ensure_success();
    rt.action(app, "start");
}, #{
    params: #{
        admin_password: #{
            kind: "password",
            description: "Initial administrator password",
        },
    },
});

// on_change fires when a parameter value changes (not on first install).
version.on_change(|rt, old| {
    // old is the app at the previous generation; use it to stop old resources.
    rt.start(app.job()
        .image(image.call())
        .command(["migrate", "--run"])
        .mount("/data", data)
    ).terminated().ensure_success();

    rt.stop(old.select(#{ types: [ResourceType.Deployment] }));
    rt.start(app).ready();
});
```

## Common patterns

**Starting things in order:**
```rhai
rt.start(db).ready();
rt.start(app.select(#{ names: ["api"] })).running();
rt.start(app).ready();
```

**Dynamic job with cleanup:**
```rhai
app.on_action("export", |rt, _p| {
    let tmp = app.volume().tmpfs();
    let job = app.job()
        .image("ghcr.io/example/exporter:latest")
        .mount("/out", tmp);
    rt.start(job).terminated().ensure_success();
    // tmp is automatically cleaned up when the action ends
});
```

**Scheduled action:**
```rhai
app.on_action("vacuum", |rt, _p| {
    rt.start(app.job().image("...").command("vacuum"))
        .terminated()
        .ensure_success();
}, #{ description: "Vacuum the database" })
    .on_schedule("H 3 * * *");  // nightly at a stable time in the 03:xx hour
```

**Multiple environment variables:**
```rhai
deployment.env([
    #{ name: "DB_HOST", value: "localhost" },
    #{ name: "DB_PORT", value: "5432" },
]);
```
