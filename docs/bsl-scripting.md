# Writing a BSL Script

BSL (BES Seedling Language) is the scripting language used to define and manage applications on a Seedling node. It describes not just what to run, but how to upgrade it, how operators can interact with it, and what happens on first install.

The full language specification is at [`docs/spec/language.md`](spec/language.md). This document is a practical guide to writing a script, with a reference summary and annotated examples. When this document and the spec disagree, the spec wins.

## Language fundamentals

BSL is written in [Rhai](https://rhai.rs). Rhai is a scripting language with Rust-like syntax: `let`, closures `|args| { body }`, string interpolation with backticks, object maps `#{ key: value }`, and arrays `[a, b, c]`. You do not need to know Rust to write BSL.

The conventional extension is `.seed.rhai`.

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
| `app.external_service(name)` | `ExternalService` | Endpoint slot; the control plane maps it to a concrete address |
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
let svc = app.service("name")
    .exported(#{ description: "Public API" });  // advertise to the control plane
let http_svc = svc.http(80);          // specialise; port defaults to 80
let route = http_svc.route("/api");   // HTTP path prefix routing

// Ingresses are keyed by (hostname, port); one service can have many.
// .tls(terminate, output) declares both what's terminated at the edge
// and what protocol is handed to the bound Service. Without it, the
// ingress is plain TCP/UDP passthrough.
svc.ingress("host.example.com", 443)
    .tls(Terminate.Https, Output.Http1)   // HTTPS → HTTP/1.1
    .redirect();                           // redirect port 80 → 443
svc.ingress("host.example.com", 8443)
    .tls(Terminate.Https, Output.Http2);   // HTTPS → HTTP/2 (h2c)
svc.ingress("tls.example.com", 443)
    .tls(Terminate.Tls, Output.Tcp);       // TLS → plaintext TCP
svc.ingress("dtls.example.com", 443)
    .tls(Terminate.Dtls, Output.Udp);      // DTLS → plaintext UDP

// HttpService.ingress() delegates to the wrapped Service, so chains
// that flow through .http() can declare ingresses without backing out.
// The resulting Ingress is bound to the Service. Throws on external services.
http_svc.ingress("api.example.com", 443).tls(Terminate.Https, Output.Http1);

// External services are slots; the operator binds them to a concrete endpoint.
let db = app.external_service("upstream-db");
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
    .stop_signal("SIGINT")            // signal sent on stop (default SIGTERM)
    .stop_timeout(30)                 // seconds before SIGKILL (default 90)
    .mount("/data", vol)              // bind volume into container
    .mount(svc.port(5432))            // bind service port into network namespace
    .http(8080, http_svc.route("/"))  // expose pod port to HTTP route
    .tcp(5432, db_svc.port(5432))     // expose pod port to TCP service
    .udp(5353, dns_svc)
    .scale(2)                         // fixed replicas (Deployment only)
    .scale(1..8)                      // scalable range (Deployment only)
    .on_update(OnUpdate.Rolling)      // default; or OnUpdate.Replace
    .on_exit(OnExit.Restart)          // default for Deployment; Terminate for Job
    .healthcheck(#{                   // see "Healthchecks" below — Deployment only
        kind: "command",
        cmd: ["curl", "-fsS", "http://localhost:8080/healthz"],
        interval: 10,
    });

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
    .kind("text")           // text | multiline | email | password | weak-password | random
    .required(false)        // (action params can also use kind "volume" — see below)
    .default_value("latest")
    .secret(false)          // password/weak-password imply secret=true; random does NOT
    .description("Docker image tag to deploy");

version.value()             // "latest" when no operator value is stored
version.is_set()            // false while only the default is in effect

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
rt.restart(deployment)          // rotate a Deployment's instances per on_update
rt.signal(target, "SIGHUP")     // deliver a POSIX signal to PID 1 of every running instance
rt.warm_certs(resources)        // pre-provision TLS certs for ingresses
rt.warm_images(resources)       // pre-pull container images without starting them

// Started methods — all block until state is reached (deadline in seconds):
started.scheduled(deadline?)
started.running(deadline?)
started.ready(deadline?)
started.terminated(deadline?)   // six-hour deadline, returns Termination
started.terminated_eventually() // no deadline, returns Termination
started.ready_eventually()      // no deadline, returns Started

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
| `AVAILABLE_MEMORY` | int | Memory available to the application, in bytes |
| `CPU_ARCHITECTURE` | string | CPU architecture of the node (e.g. `x86_64`, `aarch64`) |
| `HOST_HAS_IPV4` | bool | Whether the node has working IPv4 egress |
| `HOST_HAS_IPV6` | bool | Whether the node has working IPv6 egress |
| `NAT64_ACTIVE` | bool | Whether the node itself is providing NAT64 translation |
| `HAS_SNAPSHOTS` | bool | Whether volume storage supports copy-on-write snapshots |
| `NODE_NAME` | string | Identifier of the node running the application |
| `TIMEZONE` | string | IANA timezone of the host (e.g. `Pacific/Auckland`); `UTC` when unknown |

### Enum constants

- `OnUpdate.Rolling`: Start new, stop old (default)
- `OnUpdate.Replace`: Stop all old first, then start new
- `OnTerminate.Recreate`: Always recreate terminated containers
- `OnExit.Restart`: Always restart on exit
- `OnExit.Terminate`: Stop container on exit
- `OnExit.RestartOnFailure`: Restart on non-zero exit, terminate otherwise
- `Terminate.Tls`: Edge terminates TLS over TCP
- `Terminate.Dtls`: Edge terminates DTLS over UDP
- `Terminate.Https`: Edge terminates HTTPS (HTTP/1.1, HTTP/2, HTTP/3)
- `Output.Tcp`: Hand plaintext TCP to the Service
- `Output.Udp`: Hand plaintext UDP to the Service
- `Output.Http1`: Hand plaintext HTTP/1.1 to the Service
- `Output.Http2`: Hand plaintext HTTP/2 (h2c) to the Service
- `ResourceType.{Parameter,Service,HttpService,Ingress,Deployment,Job,Volume,ExternalVolume,Action}`: For use with `col.select()`

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
let web_svc = app.service("web").http(80);

// Ingresses route traffic from outside to a service
web_svc.ingress("myapp.example.com", 443)
    .tls(Terminate.Https, Output.Http1)  // terminate HTTPS, re-emit HTTP/1.1
    .redirect();                         // redirect HTTP on port 80 to 443

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
// `params` declares a validated schema; extra keys are passed through as-is.
app.on_action("migrate", |rt, param| {
    // Jobs defined inside actions are dynamic: created fresh per invocation.
    let job = app.job()
        .image(image.call())
        .command(["migrate", "--run", "--target", param.target])
        .mount("/data", data);
    rt.start(job).terminated().ensure_success();
}, #{
    description: "Run database migrations",
    params: #{
        target: #{
            kind: "text",
            required: true,
            default_value: "latest",
            description: "Target schema revision to migrate to",
        },
    },
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

// on_install only runs on first install, and can prompt for secrets. Param
// keys follow the BSL name rules (lowercase alphanumeric and hyphens), so
// access them via `param["admin-password"]` rather than `param.admin_password`.
app.on_install(|rt, param| {
    let seed_vol = app.volume();
    seed_vol.write("/seed.json", `{"adminPassword":"${param["admin-password"]}"}`);
    let seed_job = app.job()
        .image(image.call())
        .command(["seed-db"])
        .mount("/seed", seed_vol)
        .mount("/data", data);
    rt.start(seed_job).terminated().ensure_success();
    rt.action(app, "start");
}, #{
    params: #{
        "admin-password": #{
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

## Healthchecks

A Deployment can declare a healthcheck so seedling knows when its container is actually ready to serve, and what to do when it stops being ready.

```rhai
app.deployment("api")
    .image("ghcr.io/example/api:v1")
    .http(8080, web.route("/"))
    .healthcheck(#{
        kind: "command",
        cmd: ["curl", "-fsS", "http://localhost:8080/healthz"],
        interval: 10,         // seconds between checks (default 30)
        timeout: 3,           // seconds before a check is considered failed (default 30)
        retries: 3,           // consecutive failures before transitioning to unhealthy (default 3)
        start_period: 15,     // seconds of grace after start before failures count (default 0)
        on_failure: "replace",// "replace" (default) or "monitor"
    });
```

Healthchecks are only valid on Deployments — calling `.healthcheck(...)` on a Job is a BSL error.

**What the platform does with the result:**

- A backend pod is only added to the service routing pool once it has been observed healthy at least once. Pods still in `start_period`, or that have never been observed healthy, do not receive traffic. Once in the pool, an unhealthy pod is dropped only if a sibling is still healthy; if not, it stays in the pool (degraded) and seedling files a `service_degraded` fault so the operator can see it.
- A pod with no declared healthcheck is treated as healthy as soon as it's running.

**`on_failure` policies:**

- **`"replace"` (default).** When an instance has been unhealthy past its grace window, seedling spawns a fresh replacement *alongside* it. The unhealthy original keeps serving (possibly partial) traffic until the replacement is observed healthy; then traffic shifts to the replacement and the original is retired. If the replacement also fails to become healthy, seedling stops the cycle, leaves the original running in degraded mode, and files a `health_check_replace_failed` fault. This is reset when the operator pushes a new generation of the script.
- **`"monitor"`.** No automatic replacement. The check still gates routing (so unhealthy pods don't receive traffic when a healthy sibling exists), but seedling does not spawn replacements. Recovery is operator-driven. Use this for stateful workloads where replacement is risky.

**Probe kinds:**

- **`kind: "command"`.** Requires `cmd`. The command is run inside the container; exit code zero means healthy. `cmd` may be a single string (run through a shell) or an array (executed directly).
- `kind: "http"`, `kind: "tcp"`, and `kind: "grpc"` are reserved names for future expansion.

**Choosing timings.** `start_period + retries × interval` is the grace window before seedling considers the workload unhealthy. Set `start_period` to your typical cold-start time, and pick `interval` and `retries` so that one transient failure doesn't trigger a swap.

## Stop semantics: `stop_signal` and `stop_timeout`

By default seedling stops a container with `SIGTERM` and waits up to 90 seconds before sending `SIGKILL`. Override these per container when the workload's shutdown semantics require it:

```rhai
app.deployment("postgres")
    .image("postgres:18")
    .stop_signal("SIGINT")    // postgres maps SIGINT → fast shutdown
    .stop_timeout(30);        // hard kill after 30s
```

`stop_signal` accepts canonical (`"SIGTERM"`) or bare (`"TERM"`) forms; unknown signal names are a script-evaluation error. `stop_timeout` is in whole seconds and must be positive. The defaults match systemd's `TimeoutStopSec`.

A common pairing: PostgreSQL with `SIGINT` (fast shutdown) — the default `SIGTERM` triggers smart shutdown which waits for clients to disconnect and frequently exceeds the stop deadline.

## Sending signals: `rt.signal`

When an action needs to nudge a running container without restarting it — reload config, rotate logs, dump state — use `rt.signal`:

```rhai
app.on_action("reload", |rt, _p| {
    rt.signal(app.select(#{ names: ["postgres"] }), "SIGHUP");
}, #{ description: "Ask postgres to reload its config" });
```

Behaviour:

- Targets PID 1 of every running container instance in the selected Collection.
- Containers in transient states (starting, terminated) are silently skipped — safe to use against a deployment with multiple replicas.
- Non-container resources in the collection are ignored.
- An empty selection is a script error (usually a sign the selector matched nothing).
- The call is at-most-once across replays: if the runtime restarts mid-action and replays the closure, a previously-delivered signal is not re-sent.

Common signals: `SIGHUP` (reload config — postgres, nginx), `SIGUSR1` (log rotation), `SIGINT`/`SIGTERM` for cooperative shutdown when the deployment's declared `stop_signal` isn't right for this one-off invocation.

## Action params with `kind: "volume"`

Action and shell param schemas accept a special `kind: "volume"` that asks the operator to pick a site volume at invocation time. The runtime resolves the operator's choice into an operation-scoped binding and delivers the binding's name in `param["<name>_volume"]` (the same convention the backup actions use). The closure consumes it via `app.external_volume(...)`:

```rhai
app.on_action("dump", |rt, param| {
    let dest = app.external_volume(param["destination_volume"]);
    let job = app.job()
        .image(image.call())
        .command(["pg_dump", "--file=/dump/db.sql", "--format=custom"])
        .mount("/dump", dest);
    rt.start(job).terminated().ensure_success();
}, #{
    description: "Dump the database to a chosen volume",
    params: #{
        destination: #{
            kind: "volume",
            description: "Site volume to write the dump into",
        },
    },
});
```

The operator picks the volume from the action invocation dialog (web UI) or via `seedling-ctl apps action invoke`. The binding lives only for the operation; nothing in the script needs a stable external_volume name.

`kind: "volume"` is valid only on action and shell schemas. `app.param(...)` and `on_install` requirements reject it because their bindings outlive any single operation; for those, declare an `app.external_volume(name)` and have the operator wire it up via the UI's external volume mappings.

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
