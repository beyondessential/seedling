# Writing a BSL Script

BSL (BES Seedling Language) is the scripting language used to define and manage applications on a Seedling node. It describes not just what to run, but the steps an upgrade needs, how operators can interact with it, and what has to hold before the app counts as set up.

The full language specification is at [`.workhorse/specs/language/`](../.workhorse/specs/language/). This document is a practical guide to writing a script, with a reference summary and annotated examples. When this document and the spec disagree, the spec wins.

## Language fundamentals

BSL is written in [Rhai](https://rhai.rs). Rhai is a scripting language with Rust-like syntax: `let`, closures `|args| { body }`, string interpolation with backticks, object maps `#{ key: value }`, and arrays `[a, b, c]`. You do not need to know Rust to write BSL.

The conventional extension is `.seed.rhai`. A script can stand alone, or be one part of a [definition bundle](#definition-bundles) that also carries files for it to read.

A script runs in a fresh scope every time it is evaluated. The `app` global is pre-injected; everything else must be defined or computed.

### Replicability

A script is evaluated many times over an app's life, and must declare the same app each time from the same inputs: its bundle, its parameter values, the [host constants](#constants) and the app's seed, which [derived random values](#derived-random-values) come from. So BSL leaves out the parts of Rhai that would let an evaluation differ or stall: there is no `timestamp()`, `sleep()` throws, and a script that contains `import` fails to evaluate. An action that needs the time or the network reaches it through `rt`, for example by running a command in a container with `rt.exec`.

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
| `app.external_service(name)` | `ExternalService` | Endpoint slot; the control plane maps it to a concrete address. In an action closure it refers to the static one and throws if there is none |
| `app.deployment(name)` | Deployment builder | Long-running container workload |
| `app.job(name?)` | Job builder | One-off container workload; `name` optional in dynamic context |
| `app.volume(name?)` | Volume builder | Persistent directory; `name` optional |
| `app.external_volume(name)` | `ExternalVolume` | Volume provided by the control plane |
| `app.on_action(name, fn, opts?)` | `Action` | Operator-invokable action |
| `app.on_shell(name, fn, opts?)` | `Action` | Interactive shell for operators |
| `app.gate(name)` | Gate builder | A step the runtime keeps satisfied; see [Gates](#gates) |
| `app.action(name)` | `Action` | Look up an action handle (dynamic context only) |
| `action.invoke(params?)` | `()` | Run another action's closure inline (dynamic context only) |

### Service

```rhai
let svc = app.service("name")
    .exported(#{ description: "Public API" });  // advertise to the control plane
let http_svc = svc.http(80);          // specialise; port defaults to 80
let route = http_svc.route("/api");   // HTTP path prefix routing

// A route can answer with a redirect instead of being served by a pod.
// The positional forms carry the path tail and query over; the map form
// spells out exactly what carries, using <tail> and <query>.
http_svc.route("/v1/login").redirect("/api/login", 308);
http_svc.route("/old").redirect(#{ to: "https://example.com/new", code: 301 });

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
svc.ingress("dtls.example.com", 5684)
    .tls(Terminate.Dtls, Output.Udp);      // DTLS → plaintext UDP

// Ingresses that terminate TLS or HTTPS share a port when their hostnames
// differ: the proxy routes each connection by the name the client sends,
// so the three above on 443 and 8443 evaluate together. A pass-through
// ingress, and a DTLS ingress, hold their whole port: nothing else can
// use that port on the same transport (a DTLS ingress on 443 would take
// the UDP port HTTP/3 needs). Two ingresses of one app whose claims on a
// port conflict make the definition fail to evaluate.

// HttpService.ingress() delegates to the wrapped Service, so chains
// that flow through .http() can declare ingresses without backing out.
// The resulting Ingress is bound to the Service.
http_svc.ingress("api.example.com", 443).tls(Terminate.Https, Output.Http1);

// External services are slots; the operator maps them to a concrete endpoint.
// A pod reaches one by mounting its ports; binding a pod to one throws.
let db = app.external_service("upstream-db");
app.deployment("api").mount(db.port(5432));
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
    .memory("512Mi")                  // a quantity, or an integer number of bytes
    .cpus(0.5)
    .pids_limit(128)
    .workdir("/app")
    .user("999:999")                  // run as a non-root uid:gid
    .writable_rootfs()                // opt out of read-only root
    .cap_add("NET_BIND_SERVICE")
    .stop_signal("SIGINT")            // signal sent on stop (default SIGTERM)
    .stop_timeout("30s")              // before SIGKILL, in whole seconds (default 90s)
    .mount("/data", vol)              // bind volume into container
    .mount(svc.port(5432))            // bind service port into network namespace
    .http(8080, http_svc.route("/"))  // expose pod port to HTTP route
    .tcp(5432, db_svc.port(5432))     // expose pod port to TCP service
    .udp(5353, dns_svc)
    .scale(2)                         // fixed replicas (Deployment only)
    .scale(1..=8)                     // scalable range (Deployment only)
    .on_update(OnUpdate.Rolling)      // default; or OnUpdate.Replace
    .after(db)                        // wait for a gate, Deployment or Job (or an array of them)
    .priority(Priority.Critical)      // claim on host resources (Deployment only)
    .on_exit(OnExit.Restart)          // default for Deployment; Terminate for Job
    .healthcheck(#{                   // see "Healthchecks" below — Deployment only
        kind: "command",
        cmd: ["curl", "-fsS", "http://localhost:8080/healthz"],
        interval: "10s",
    });

app.job("migrate")
    .image("ghcr.io/example/app:v1.2.3")
    .deadline("5m");                  // whole seconds; no deadline = runs indefinitely
```

### Volume

```rhai
let vol = app.volume("data")
    .exported(#{ description: "App data" })  // advertise to control plane
    .readonly()                              // read-only mount
    .tmpfs()                                 // RAM-backed; does not survive reboot
    .write("/config.json", contents)         // pre-populate a file (string or File)
    .write_dir("/errors", app.dir("pages")); // copy a bundle folder in
```

### Parameter

```rhai
let version = app.param("version")
    .kind("text")           // text (one line) | multiline | email | password | weak-password | random
                            // | boolean | integer | quantity | choice | version | version-semver | purl
                            // | url | url-origin | hostname | ip-address | ip-subnet | ip-range
                            // | duration | regex
    .required(false)        // (action params can also use kind "volume" — see below)
    .default_value("latest")
    .secret(false)          // password/weak-password imply secret=true unless this says false; random does NOT
    .description("Docker image tag to deploy");

version.value()             // "latest" when no operator value is stored
version.parsed()            // the kind's parser result for value(): here the same string
version.is_set()            // false while only the default is in effect

version.validate(|value, all| {
    // throw to reject the proposed value; `value` and each entry of `all`,
    // which maps every declared param that would be set, are parser results
    if !value.starts_with("2.12") { throw "this definition runs 2.12.x"; }
});
```

A kind that takes arguments is given as a kind map, a `name` and the kind's arguments. `integer` and `quantity` take an optional `range`, `choice` takes its `options`, which it requires, and `duration` takes an optional `min`, `max` and `precision` (see [Durations](#durations)):

```rhai
let port = app.param("port")
    .kind(#{ name: "integer", range: 1..=65535 })  // or 1..65536; `1..` has no upper bound
    .default_value(8080);                          // a boolean or integer default is stored as its text
let tier = app.param("tier")
    .kind(#{ name: "choice", options: ["small", "large"] })
    .default_value("small");
let grace = app.param("grace")
    .kind(#{ name: "duration", min: "1s", max: "1h", precision: "s" })
    .default_value("90s");                         // or a Duration, stored as its text
let canary = app.param("canary").kind("boolean").required(false);
let cache = app.param("cache-memory")
    .kind(#{ name: "quantity", range: 64 * 1024 * 1024..=AVAILABLE_MEMORY })
    .default_value("256Mi")
    .description("Memory for the cache, in bytes");

port.parsed()               // 8080, an integer
cache.parsed()              // 268435456, an integer of bytes
canary.is_set() && canary.parsed()  // a boolean, without comparing against "true"
```

A range with no start, such as `..10`, starts at 0, so give the start of a range that takes negative values. `default_value("")` removes a default. A default the kind refuses fails the evaluation once the script has finished, since `kind` and `default_value` can be called in either order. A `boolean` or `choice` parameter cannot be secret, since the definition lists every value it can take; the same goes for action and shell params.

Every set value is first checked against its kind, by the kind's [parser](#parsing-a-kind): `text` and every kind but `multiline` hold a single line with no control characters, `email` needs an address, `password` a strong one, `boolean` exactly `true` or `false`, `integer` a whole number in plain decimal form within its range, `quantity` a [quantity](#quantities) within its range, `choice` one of its options, matched case-sensitively, and `duration` a [duration](#durations) within its bounds. A value its kind refuses is rejected with the parser's reason, and while any value is refused no validator runs. An empty stored value is checked like any other; unset a parameter to clear it.

Operator interfaces offer a toggle for a `boolean` parameter, a number input for an `integer` one, a text input showing the resolved amount for a `quantity` one, and a selection for a `choice` one. Clients setting parameters through `/apps/params/set` may send a JSON boolean for a `boolean` parameter and a JSON integer for an `integer` or `quantity` one; it is stored as its text.

A validator decides whether a proposed value is acceptable. It runs before any definition update or parameter change is stored, against the values the app would hold if the request went through, so it sees the new definition's rules alongside the new values. A rejection refuses the request with `validation_failed` and the thrown value as the reason; nothing changes. A validator runs only while its param is set (use `required` for presence), gets no `rt`, and may not define resources. Call `validate` at most once per param, at the top level.

### Gate

```rhai
app.gate("schema")
    .key(version.value())                // the step reruns whenever this value changes
    .exclusive(app.select(#{ names: ["api", "worker"] }))  // held stopped while the step runs
    .after(db)                           // start order, as for a Deployment or Job
    .check(check_job, #{ interval: "5m" }) // optional: a Job that proves the step still holds
    .run(|rt, last| {                    // the body; takes up to two parameters
        rt.start(app.job().image(image.call()).command(["migrate"]))
            .terminated()
            .ensure_success();
    });
```

See [Gates](#gates) for what each method does.

### Runtime (rt)

Available inside action closures and gate bodies:

```rhai
rt.start(resources)             // schedule resources, returns Started
rt.stop(resources, deadline?)   // unschedule and block until terminated; default deadline is the longest stop timeout + 30s
rt.query(resources)             // returns Started without scheduling
rt.restart(deployment)          // rotate a Deployment's instances per on_update
rt.signal(target, "SIGHUP")     // deliver a POSIX signal to PID 1 of every running instance
rt.warm_certs(resources)        // pre-provision TLS certs for ingresses
rt.warm_images(resources)       // pre-pull container images without starting them
rt.write(volume, path, contents)         // write a file (string or File) into a volume at action time
rt.exec(target, argv, options?)          // run argv inside a running container, returns Executed

// Started methods — all block until state is reached (deadline a duration in whole seconds, not zero):
started.scheduled(deadline?)
started.running(deadline?)
started.ready(deadline?)
started.terminated(deadline?)   // six-hour deadline, returns Termination
started.terminated_eventually() // no deadline, returns Termination
started.ready_eventually()      // no deadline, returns Started
// On what rt.warm_certs or rt.warm_images returned, only ready() and
// ready_eventually() are allowed; the other barriers throw.

termination.ensure_success()    // throws if the resource failed

executed.exit_code()            // int — exit code of the rt.exec command
executed.success()              // bool — true if exit code is 0
executed.ensure_success()       // throws with the exit code if non-zero
```

### Collections

```rhai
col(val)                         // coerce anything to Collection (an Action, like any other non-resource, is empty)
col.one()                        // any single resource (or null)
col.only(other)                  // intersection
col.except(other)                // difference
col.select(#{ types: [ResourceType.Deployment], names: ["api", "worker"] })
col.select(#{ name_patterns: ["worker-*"] })
```

### Constants

| Name | Type | Description |
|---|---|---|
| `AVAILABLE_THREADS` | int | Compute threads available; `1` while undetermined, with a `host_constant_undetermined` fault on `_system` |
| `AVAILABLE_MEMORY` | int | Memory available to the application, in bytes; while undetermined every evaluation fails, with a `host_constant_undetermined` fault on `_system` |
| `CPU_ARCHITECTURE` | string | CPU architecture of the node (e.g. `x86_64`, `aarch64`) |
| `HOST_HAS_IPV4` | bool | Whether the node has working IPv4 egress; `false` while undetermined, with an `ipv4_egress_undetermined` fault on `_system` |
| `HOST_HAS_IPV6` | bool | Whether the node has working IPv6 egress; `false` while undetermined, with an `ipv6_egress_undetermined` fault on `_system` |
| `NAT64_ACTIVE` | bool | Whether the node itself is providing NAT64 translation |
| `HAS_SNAPSHOTS` | bool | Whether volume storage supports copy-on-write snapshots |
| `NODE_NAME` | string | Identifier of the node running the application; while undetermined every evaluation fails, with a `host_constant_undetermined` fault on `_system` |
| `TIMEZONE` | string | IANA timezone of the host (e.g. `Pacific/Auckland`); `UTC` while undetermined, with a `timezone_undetermined` fault on `_system` |
| `IDLE_CMD` | array | A no-op long-running command, `sleep infinity` under a shell that exits on SIGTERM or SIGINT, for containers whose only purpose is to host `rt.exec` calls |

### Enum constants

- `OnUpdate.Rolling`: Start new, stop old (default)
- `OnUpdate.Replace`: Stop all old first, then start new
- `OnTerminate.Recreate`: Always recreate terminated containers
- `OnExit.Restart`: Always restart on exit
- `OnExit.Terminate`: Stop container on exit
- `OnExit.RestartOnFailure`: Restart on non-zero exit, terminate otherwise
- `Priority.Critical`: Among the last workloads shed under memory pressure, and given the largest share of contended CPU and I/O
- `Priority.Elevated`: Favoured over ordinary workloads but yields to `Critical`
- `Priority.Normal`: The default; no preferential treatment
- `Priority.Low`: First shed under memory pressure, and yields contended CPU and I/O to every higher level
- `Terminate.Tls`: Edge terminates TLS over TCP
- `Terminate.Dtls`: Edge terminates DTLS over UDP
- `Terminate.Https`: Edge terminates HTTPS (HTTP/1.1, HTTP/2, HTTP/3)
- `Output.Tcp`: Hand plaintext TCP to the Service
- `Output.Udp`: Hand plaintext UDP to the Service
- `Output.Http1`: Hand plaintext HTTP/1.1 to the Service
- `Output.Http2`: Hand plaintext HTTP/2 (h2c) to the Service
- `ResourceType.{Parameter,Service,Ingress,Deployment,Job,Volume,ExternalVolume,ExternalService,Action}`: For use with `col.select()`

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
let api = app.deployment("api")
    .image(image.call())
    .scale(1..=4)
    .mount("/data", data)
    .http(8080, web_svc.route("/api"))
    .http(8080, web_svc.route("/"));

// A gate is a step the runtime keeps satisfied. This one migrates the schema
// whenever the version changes, with the API held stopped while it runs.
app.gate("schema")
    .key(version.value())
    .exclusive(api)
    .run(|rt, last| {
        let job = app.job()
            .image(image.call())
            .command(["migrate", "--run"])
            .mount("/data", data);
        rt.start(job).terminated().ensure_success();
    });

// Actions are available to operators via the UI or CLI.
// The closure runs when an operator invokes the action.
// `params` declares a validated schema; extra keys are passed through as-is.
// Every handler (`on_action`, `on_shell`, a gate's `run`,
// `validate`) must be a closure: passing a function by name, as
// `handler` or `Fn("handler")`, throws. Wrap it instead: `|rt, param| handler(rt, param)`.
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

// `shell.attach` starts the job, connects the operator's terminal to it, and
// blocks until the attachment ends: the job exits, the operator closes the
// session or stops it, or the connection is lost. It returns a `ShellExit`
// with `exited()`, `exit_code()` (`()` if the job did not exit), `success()`
// and `ensure_success()`. The closure then runs on, with `rt` but without the
// terminal, and the session ends when the closure returns. Dynamic resources
// the closure starts are torn down after the session ends. In a shell,
// `rt.start` places only dynamic resources in the desired state and `rt.stop`
// throws for a resource the app declares.
app.on_shell("check", |rt, shell, _param| {
    shell.attach(app.job().image(image.call()).command("sh")).ensure_success();
    rt.start(app.job().image(image.call()).command(["cleanup"])).terminated().ensure_success();
});
```

A closure's `param` map holds each declared param that has a value as its kind's [parser result](#parsing-a-kind): an `email` param is a map of `local` and `domain`, a `boolean` a boolean, an `integer` or `quantity` an integer, a `version` a `Version` and a `duration` a `Duration`. A declared param left without a value is absent, so test it with `"name" in param`. Params a schema does not declare arrive as they were given. Operators and clients may send a JSON boolean for a `boolean` param and a JSON integer for an `integer` or `quantity` param, as well as its text.

## Standard helpers

Helpers for parsing, checking and encoding values live in the `std` module, and are called by their qualified name, as `std::name(arguments)`. Keeping them in a module means a helper added in a later Seedling version cannot collide with a function or variable your script already defines. `std` is available everywhere a script runs: at the top level, in action and shell closures, in gate bodies and in validators.

Every helper follows the same conventions:

- **It throws on malformed input.** A helper never guesses, coerces or returns a partial result. A validator can call a parsing helper and let it throw, which rejects the value; to turn a failure into a yes-or-no answer elsewhere, catch it with `try`/`catch`.
- **It returns a map for compound text** such as a URL, a plain string, integer or boolean where that is all the value is, and a value of its own type only where that value has methods, such as comparing versions.
- **It is replicable.** The same arguments always give the same result: a helper never reads the clock, the network or the filesystem, and never changes the app. The [derived random value](#derived-random-values) helpers are the one exception: they also read a seed the runtime holds, so the same call gives the same value wherever the seed is the same.

Calling a helper that does not exist throws, like any unknown function. Which helpers exist depends on the Seedling version, so declare the versions a definition supports with the `seedling` field of its [bundle metadata](#definition-bundles).

### Parsing a kind

Every parameter kind has a parser, `std::parse_<kind>` with any `-` written as `_`: `std::parse_text`, `std::parse_multiline`, `std::parse_email`, `std::parse_password`, `std::parse_weak_password`, `std::parse_random`, `std::parse_boolean`, `std::parse_integer`, `std::parse_quantity`, `std::parse_choice`, `std::parse_version`, `std::parse_version_semver`, `std::parse_purl`, `std::parse_url`, `std::parse_url_origin`, `std::parse_hostname`, `std::parse_ip_address`, `std::parse_ip_subnet`, `std::parse_ip_range`, `std::parse_duration` and `std::parse_volume`. A kind's argument follows the value: `std::parse_integer(v)` or `std::parse_integer(v, 1..=65535)`, `std::parse_quantity(v)` or `std::parse_quantity(v, 1..)`, `std::parse_choice(v, ["small", "large"])`, and `std::parse_ip_subnet(v)` or `std::parse_ip_subnet(v, 4)`. A kind with several arguments takes them as a map, as in `std::parse_duration(v, #{ max: "1h" })`. The parser is exactly the check the runtime applies to a value of that kind, for action and shell params and for stored parameters alike, so calling it applies the same rule:

```rhai
let contact = std::parse_email("ops@example.org");
contact.local   // "ops"
contact.domain  // "example.org"

app.param("fallback-contact").validate(|v, all| { std::parse_email(v); });
```

`std::parse_email` returns the parts of the address, split at its first `@`. `std::parse_boolean` returns a boolean, and `std::parse_integer` and `std::parse_quantity` an integer. `std::parse_version` and `std::parse_version_semver` return a version, `std::parse_purl` the components of a Package URL, `std::parse_url` and `std::parse_url_origin` the components of a URL, `std::parse_hostname` the hostname in its ASCII form, the three IP parsers an address, subnet or range, and `std::parse_duration` a `Duration`, all described below. Every other parser returns the value unchanged. `std::parse_volume` checks a site volume's name only; whether the volume exists is checked when the action runs.

### Quantities

A quantity is a number with an optional suffix, as Kubernetes writes resource quantities, read as a whole integer: `std::parse_quantity` and the `quantity` kind read one, and `container.memory` takes one as a number of bytes. The number may have a sign and a decimal point (`-1`, `1.5`, `.5`), and the suffix multiplies it:

- `k`, `M`, `G`, `T`, `P` and `E` by powers of 1000;
- `Ki`, `Mi`, `Gi`, `Ti`, `Pi` and `Ei` by powers of 1024;
- `e` or `E` followed by an integer, such as `1e9`, by that power of ten.

Suffixes are case-sensitive and nothing may stand between the number and its suffix. A quantity that does not come to a whole number, or falls outside the signed 64-bit range, is refused rather than rounded or capped, so `512m`, which Kubernetes reads as milli, is refused with a hint towards `512Mi`. A quantity has no unit: the parameter's description says whether it counts bytes, bits or anything else.

```rhai
std::parse_quantity("1.5Gi")      // 1610612736
std::parse_quantity("2k")         // 2000
std::parse_quantity("512Mi") < AVAILABLE_MEMORY
```

### Versions

There are two version kinds. `version` takes a version of any scheme: a Debian package version, a partial image tag such as `16.4`, or a release tag. `version-semver` takes only a full [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) version, so `2.71` is refused, and adds its parts and requirement matching. Both accept a leading `v` or `V`.

`std::parse_version` returns a `Version`. A value must be non-empty printable ASCII with no spaces, so floating tags such as `latest` and digests are accepted. Versions compare with `<`, `<=`, `>`, `>=`, `==` and `!=`. A version starting with a digit, or with a `v` or `V` before a digit, is _numbered_, and two numbered versions compare using [libversion](https://github.com/repology/libversion/blob/master/doc/ALGORITHM.md)'s ordering, which ignores the separators and leading zeros, puts `alpha`, `beta`, `rc` and `pre` before the release and `patch`, `post` and `pl` after it, and ignores a leading `v` before a digit. Every other version, such as `latest`, `main` or `sha256:244fd47e07d10`, sorts above all numbered ones, and two of them compare by their text, byte by byte. `segments()` splits the version into its runs of digits, as integers, and of letters, as strings; in a version that is not numbered, a run of digits too large for an integer is a string:

```rhai
let pg = std::parse_version("16.4-bookworm");
pg.segments()                                         // [16, 4, "bookworm"]
std::parse_version("1.0rc1") < std::parse_version("1.0")    // true
std::parse_version("v2.0") == std::parse_version("2.0.0")   // true
std::parse_version("99.0") < std::parse_version("latest")   // true
```

`std::parse_version_semver` returns a `SemVer`, with `major()`, `minor()` and `patch()` as integers and `pre_release()` and `build()` as strings, empty when absent. SemVers compare by SemVer precedence, which ignores build metadata. `matches(requirement)` takes the same requirement syntax as the `seedling` field of the [bundle metadata](#definition-bundles), and a pre-release only matches a requirement that names a pre-release of the same version:

```rhai
let release = std::parse_version_semver("v2.71.3");
release.minor()               // 71
release.matches("~2.71")      // true
std::parse_version_semver("2.71.4-rc.1").matches("~2.71")    // false

app.param("version").kind("version-semver").validate(|v, all| {
    if !v.matches("~2.71") { throw "this definition runs 2.71.x"; }
});
```

A version prints exactly as it was written, `v` included, so `` `ghcr.io/org/app:${release}` `` gives back the operator's tag. Comparing a version with anything but a version of the same type throws, including a string or the other version type. Sort a list of versions with a closure: `versions.sort(|a, b| a <= b)`.

### Package URLs

The `purl` kind takes a [Package URL](https://ecma-tc54.github.io/ECMA-427/), a reference to a package in any ecosystem, such as `pkg:npm/%40angular/core@17.0.0` or `pkg:cargo/serde@1.0.200`, for a script to pass on to a workload. Any type is accepted, and no type's own naming rules are applied, so `pkg:pypi/Foo_Bar` keeps its name as written. Input in a form other than the canonical one is read as the [purl specification](https://github.com/package-url/purl-spec/blob/v1.1.1/docs/specification/how-to-parse.md) reads it: `pkg://NPM/foo` is type `npm`, empty qualifiers and `.` subpath segments are dropped. The scheme must be lower-case `pkg:`, and a purl is refused when a qualifier has no `=`, a qualifier key appears twice, a decoded component holds a control character or line break, or a non-empty version is not a valid `version`.

`std::parse_purl` returns a map of the components, percent-decoded. `type` and `name` are always present; `namespace`, `version`, `qualifiers` and `subpath` only when the purl has them. `version` is a `Version`, so it compares like any other. `qualifiers` maps each lower-cased key to its value, except `checksum`, which is an array of its `algorithm:digits` entries, lower-cased and sorted by algorithm:

```rhai
let purl = std::parse_purl("pkg:npm/%40scope/foo@1.2.0?arch=x64&checksum=SHA256:BB,sha1:aa#lib");
// #{ type: "npm", namespace: "@scope", name: "foo", version: 1.2.0,
//    qualifiers: #{ arch: "x64", checksum: ["sha1:aa", "sha256:bb"] }, subpath: "lib" }
purl.version >= std::parse_version("1.0")    // true

app.param("dependency").kind("purl").validate(|v, all| {
    if v.type != "npm" { throw "an npm package is needed"; }
});
```

A parameter's value stays as the operator entered it, so a workload given `dependency.value()` receives their text, not a normalised form.

### URLs and hostnames

The `url` kind takes a URL of any scheme, parsed as a browser parses it under the [URL Standard](https://url.spec.whatwg.org/): `https://example.org/app`, `postgres://app:secret@db.internal:5432/app`, `file:///etc/hosts` and `mailto:ops@example.org` are all accepted. So is a PostgreSQL connection URI for a Unix socket, `postgresql://app@/db?host=/run/postgresql`, which the URL Standard itself refuses; it has no `host`. `std::parse_url` returns a map of its components. `scheme` and `path` are always present; `host`, `port`, `query`, `fragment`, `username` and `password` only when the URL has them, an empty query or fragment counting as none. `port` is an integer, filled from the scheme's default for `http`, `https`, `ws`, `wss` and `ftp` when the URL gives none; port `0` is refused. `host` is in lower case, with an internationalised name in its `xn--` form, whatever the scheme, so `postgres://DB.Example.org` and `https://db.example.org` name the same host. `username` and `password` are percent-decoded; `path`, `query` and `fragment` are left as written, so a script that needs the query's pairs splits them itself:

```rhai
let db = std::parse_url("postgres://app:p%40ss@DB.Internal:5432/app?sslmode=require");
// #{ scheme: "postgres", host: "db.internal", port: 5432, path: "/app",
//    query: "sslmode=require", username: "app", password: "p@ss" }

app.param("database").kind("url").secret(true).validate(|v, all| {
    if v.scheme != "postgres" { throw "a postgres:// URL is needed"; }
});
```

A URL can carry a password, and a `url` parameter is not secret unless the script says so, so mark one that might with `secret(true)`.

The `url-origin` kind takes an `http` or `https` URL naming a host and optionally a port, and nothing else: no userinfo, query or fragment, and no path but a single `/`. `std::parse_url_origin` returns a map of `scheme`, `host` and `port`. The `hostname` kind takes a name an ingress can be declared on, and `std::parse_hostname` returns it in lower case, an internationalised name in its `xn--` form. Both refuse an IP address, and any name a URL would read as one, such as `app.0x1f`, so the host of a URL origin and a hostname can each be given to `service.ingress` as they are:

```rhai
let public = std::parse_url_origin("https://Bücher.example:8443/");
// #{ scheme: "https", host: "xn--bcher-kva.example", port: 8443 }
std::parse_hostname("App.Example.org")    // "app.example.org"
```

### IP addresses, subnets and ranges

The `ip-address` kind takes an IPv4 address in dotted decimal, such as `192.0.2.1`, or an IPv6 address, such as `2001:db8::1`. A dotted number with a leading zero, a zone such as `%eth0`, brackets, and an IPv4-mapped IPv6 address such as `::ffff:192.0.2.1` are refused; write that last one as the IPv4 address. The `ip-subnet` kind takes an address and a prefix length, such as `10.0.0.0/8`, or a bare address, which stands for itself alone (`/32` or `/128`). A subnet with host bits set, such as `10.0.0.5/24`, is refused, and the error names the subnet it lies in. The `ip-range` kind takes two addresses of one family joined by `-` with no spaces, such as `10.0.0.100-10.0.0.199`, a subnet, or a single address.

Each takes an optional `family`, `4` or `6`, which refuses the other family: `kind(#{ name: "ip-range", family: 4 })`, and `std::parse_ip_range(v, 4)`.

`std::parse_ip_address` returns an `IpAddress`, `std::parse_ip_subnet` an `IpSubnet` and `std::parse_ip_range` an `IpRange`. Each has `family()`, `4` or `6`, and prints in a canonical form: an IPv6 address in lower case and shortened, a subnet always with its prefix length, and a range always as `first-last`, whichever way it was written. That form parses back to an equal value, so it can be handed to software with a stricter parser of its own.

- An `IpAddress` has `to_subnet()`, the subnet of just that address, `to_subnet(prefix_len)`, the subnet of that size the address lies in, and `to_range()`. Addresses of one family compare with `<`, `<=`, `>`, `>=`, `==` and `!=` in numeric order. An IPv4 and an IPv6 address are never equal, and ordering one against the other throws.
- An `IpSubnet` has `address()`, its first address, `prefix_len()` and `to_range()`.
- An `IpRange` has `start()` and `end()`.
- Subnets and ranges compare only with `==` and `!=`. They have `contains(other)`, which is true when every address of `other` lies within, and `overlaps(other)`, which is true when at least one does. `other` is an address, subnet or range, never a string, so parse an operator's text first. Both are `false` across families.

Comparing an IP value with any value of another type throws, including a string or another of the three IP types. An IP value passed to `invoke` is taken as its canonical text:

```rhai
let lan = std::parse_ip_subnet("10.0.0.0/24");
lan.contains(std::parse_ip_address("10.0.0.7"))          // true
lan.overlaps(std::parse_ip_range("10.0.0.250-10.0.1.9"))  // true
std::parse_ip_address("10.0.0.7").to_subnet(16)          // 10.0.0.0/16
std::parse_ip_range("10.0.0.0/30")                       // 10.0.0.0-10.0.0.3

app.param("allowed").kind("ip-subnet");
app.param("pool").kind(#{ name: "ip-range", family: 4 }).validate(|v, all| {
    if "allowed" in all && !all.allowed.contains(v) {
        throw "the address pool lies outside the allowed subnet";
    }
});
```

### Durations

The `duration` kind takes a length of time in [jiff's friendly format](https://docs.rs/jiff/0.2/jiff/fmt/friendly/index.html#grammar), in compact or spelled-out form: `90s`, `1h30m`, `1h 30m`, `1 hour, 30 minutes` and `01:30:00` are all accepted. Units run from nanoseconds to weeks, a day counting as 24 hours and a week as 7 days. Only the last unit may carry a fraction, from hours down, as in `1.5h`. A sign, an `ago`, a month or year unit, and a length beyond about 292 years are refused; write about a month as `30d`.

`std::parse_duration` returns a `Duration`. A `duration` kind's optional `min` and `max` bound it, and `precision`, one of `d`, `h`, `m`, `s`, `ms`, `us` and `ns`, requires a whole number of that unit. Give `precision: "s"` to a parameter that feeds an option taking whole seconds, so a fraction is refused when the operator enters it rather than when the definition evaluates.

A `Duration` converts to a string from days down, as `std::parse_duration("90m").to_string()` gives `1h 30m` and `2w` gives `14d`; zero is `0s`. Durations compare with `<`, `<=`, `>`, `>=`, `==` and `!=`, but comparing one with a string or a number throws. A `Duration` passed to `invoke` is taken as its text. `+` and `-` combine two durations, and `*` and `/` take an integer, dividing toward zero; a negative result, an overflow or a division by zero throws. `as_seconds()` and `as_millis()` return an integer and throw unless the duration is a whole number of that unit, and `as_seconds_f()` and `as_millis_f()` return a floating-point number:

```rhai
let d = std::parse_duration("1m 30s");
d.as_seconds()                                          // 90
d * 2 > std::parse_duration("3m")                       // false: they are equal
std::parse_duration("1500ms").as_seconds_f()            // 1.5
std::parse_duration("2h", #{ max: "1h" })               // throws: must be at most 1h
```

Every option that takes a length of time, such as `stop_timeout`, `deadline`, a healthcheck's or a gate check's `interval`, or a barrier deadline, takes a `Duration` or a string `std::parse_duration` accepts. A number is refused, so write `"30s"`, not `30`. An option documented in whole seconds also refuses a fraction of a second:

```rhai
app.deployment("db").stop_timeout("2m");
app.deployment("api").stop_timeout(grace.parsed() * 2);
```

### Regular expressions

`std::regex(pattern)` compiles a pattern in the [Rust `regex` crate's syntax](https://docs.rs/regex/1/regex/#syntax) and returns a `Regex`. Searches run in time linear in the text, with no backtracking, so the syntax has no backreferences or look-around. A search finds a match anywhere in the text; anchor the pattern with `^` and `$` to match the whole of it.

```rhai
let kv = std::regex("(?<key>[a-z_]+)=(?<value>[^;]*)");

kv.is_match("a=1")                       // true
kv.find("x; a=1")                        // #{ text: "a=1", start: 3, end: 6 }
kv.find_all("a=1;b=2").map(|m| m.text)   // ["a=1", "b=2"]
kv.captures("a=1").named.value.text      // "1"
kv.replace_all("a=1;b=2", "$value=$key") // "1=a;2=b"
kv.replace_all("a=1", |c| c.named.key.text.to_upper())  // "A"
std::regex(",\\s*").split("a, b,c", 2)   // ["a", "b,c"]
```

Positions count characters, as `sub_string` does. `captures` gives `groups`, every group in order with the whole match first and `()` for a group that took no part, and `named`, the named groups by name. `find` and `captures` give `()` when nothing matches. In a replacement string, `$1`, `$name` and `${name}` insert a group and `$$` a `$`; a reference to a group the pattern does not have, or a stray `$`, throws, whether or not anything matched. A replacement closure is called with each match's captures and returns the text to insert as it is.

A pattern written as a string literal is checked when the definition is evaluated, so `std::regex("(")` fails the evaluation even in an action that never runs. The `regex` param kind takes a pattern from an operator: `std::parse_regex` and `param.parsed()` return the `Regex`, and a malformed pattern is refused when the value is set. Compiling counts toward a per-evaluation budget of 32 MiB of compiled pattern (`--script-max-regex-compiled-size`); a pattern too big for what is left throws, and uses the rest of the budget up.

### Derived random values

A script that needs a password, a token or an identifier draws it from a seed the runtime holds, rather than from a random source, so the value is the same on every evaluation:

```rhai
let alphanumeric = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
let db_password = std::random_string("db-password", 32, alphanumeric);

app.deployment("database")
    .image("docker.io/library/postgres:17")
    .env("POSTGRES_PASSWORD", db_password);
app.volume("config");

app.on_action("write-client-config", |rt, p| {
    // The captured value: the one the database container was given.
    rt.write(app.volume("config"), "/db.env", `DB_PASSWORD=${db_password}\n`);
});
```

| helper | returns |
| --- | --- |
| `std::random_bytes(label, n)` | a Blob of `n` bytes |
| `std::random_string(label, length, alphabet)` | `length` characters, each drawn evenly from `alphabet`, which holds at least two distinct characters |
| `std::uuid_v4(label)` | a version 4 UUID, hyphenated and lowercase |

The label names the value, and is any non-empty string. A value depends only on the seed, the helper, the label and the other arguments, so adding, removing or reordering calls leaves every other value as it was, and no two apps derive the same values.

Which seed a call derives from depends on where it runs:

- **At the top level and in validators,** from the app's seed, created when the app is registered and kept until it is deregistered.
- **Inside an action, shell or gate body closure,** from the operation's seed, created when the operation starts and kept for every pass and replay of it, including after a daemon restart. An action invoked from another derives from its caller's.

Pitfalls:

- **The same call gives a different value inside a closure than at the top level.** A closure that needs a value the app's workloads also hold, like the password above, captures the top-level value rather than calling the helper again.
- **An operation's values differ in the next operation.** Anything that must outlast the operation is derived at the top level, or written into a volume by the operation that derived it.
- **Every argument is part of the value.** Changing a length or an alphabet gives a different value, not a longer or shorter one; two uses that need different secrets need different labels.
- **App-seeded values change when the seed does:** when an operator rotates it, when the app is deregistered and registered again, and when an app is moved to another host, which registers it there afresh. Data written with the old values keeps them, so a database whose password came from the seed needs its password changed to match.
- **Derived values are withheld like secret parameters.** Any string that holds one reads as `null` when an operator describes the app, and it is masked as `<redacted>` in breadcrumbs, errors and faults. A short value is masked wherever it appears, even by chance.

#### Password hashes

A service that stores a password hash rather than the password, such as a basic-auth user list, takes one from a password hash helper. Its salt is derived from the seed under the label, so the hash is the same on every evaluation:

```rhai
let admin_password = std::random_string("admin-password", 24, alphanumeric);
let admin_hash = std::password_argon2id("admin-password", admin_password);
```

| helper | returns |
| --- | --- |
| `std::password_argon2id(label, password)` | an Argon2id hash, as a PHC string (`$argon2id$v=19$m=19456,t=2,p=1$...`) |
| `std::password_bcrypt(label, password)` | a bcrypt hash (`$2b$12$...`) |

The work factors are fixed, so a hash is cheap enough to compute on every evaluation, and one evaluation computes at most 16 hashes unless the operator raises the limit. Both throw on an empty password, which most often means a value that was never set. `std::password_bcrypt` also throws on a password longer than 72 bytes or holding a zero byte, because bcrypt would silently ignore everything from that point on. Inside a closure the operation's seed gives a different hash each operation; every one of them verifies the same password. A password hash is withheld like any other derived value.

### Digests and encodings

These helpers turn bytes into text and back, digest them, authenticate them under a key, and name them with a UUID. Each one that takes bytes accepts a string (its UTF-8 bytes), a Blob, a [bundle file](#definition-bundles) (its contents) or a bundle directory (every file's path and contents, in a fixed order).

| helper | returns |
| --- | --- |
| `std::encode_hex(data)` / `std::decode_hex(text)` | lowercase hexadecimal / a Blob |
| `std::encode_base64(data)` / `std::decode_base64(text)` | base64 with padding / a Blob |
| `std::encode_base64url(data)` / `std::decode_base64url(text)` | URL-safe base64 without padding / a Blob |
| `std::encode_utf8(text)` / `std::decode_utf8(data)` | a Blob / a string |
| `std::encode_percent(data)` / `std::decode_percent(text)` | percent-encoding of every byte but `A`-`Z`, `a`-`z`, `0`-`9`, `-`, `.`, `_` and `~`, with uppercase digits / a Blob |
| `std::digest_sha256(data)`, `std::digest_sha512(data)` | a 32 or 64 byte Blob |
| `std::digest_blake2b(data)`, `std::digest_blake3(data)` | a 64 or 32 byte Blob |
| `std::hmac_sha256(key, message)`, `std::hmac_sha512(key, message)` | a 32 or 64 byte Blob |
| `std::uuid_v5(namespace, name)` | a version 5 UUID, hyphenated and lowercase |

```rhai
// A basic-auth header.
let auth = "Basic " + std::encode_base64("admin:" + admin_password);

// A password placed in a URL, which may hold `@`, `/` or `:`.
let db_url = "postgres://app:" + std::encode_percent(db_password) + "@db/app";

// An environment value that changes when the config file does, restarting the workload with it.
let config_rev = std::encode_hex(std::digest_sha256(app.file("nginx.conf")));
let configs_rev = std::encode_hex(std::digest_blake3(app.dir("config")));

// An identifier that is the same for the same name in every app, on every host.
let tenant_id = std::uuid_v5("6ba7b811-9dad-11d1-80b4-00c04fd430c8", "https://example.org/tenant/a");
```

Decoders are strict: whitespace, wrong padding, a stray character or invalid UTF-8 throws rather than being skipped or repaired. To read bytes as text with invalid sequences replaced instead, use the Blob's own `as_string()`, which substitutes U+FFFD for each one.

`std::decode_percent` is the exception: it takes a URL component as a URL carries it, such as a path or query that `std::parse_url` leaves encoded, so any character other than `%` stands for its own UTF-8 bytes, and `+` stays `+`. A `%` that does not begin two hexadecimal digits still throws.

A result computed from a secret parameter or a derived value is withheld like the value itself, so the header above reads as `null` when an operator describes the app.

### Quoting

`std::quote_shell(words)` quotes a string so the shell reads it as one word holding exactly that string, wherever it is placed in a command. Given an array of strings, it quotes each and joins them with spaces, one word per element, and an empty array gives an empty string. A string of only ASCII letters, digits and `_ - . / : , + @ %` is returned as it is; anything else is put in single quotes, with each `'` in it written as `'\''`. A string holding a NUL character throws, since no shell word can hold one.

```rhai
let backup = app.job("backup")
    .image("docker.io/library/alpine:3.22")
    .command(["sh", "-c",
        "tar -czf /backup/" + std::quote_shell(name) + ".tgz -- " + std::quote_shell(paths)
        + " && sync"]);
```

Quoting stops the shell from changing a value, not the program from reading it: a value beginning with `-` still reaches the program as an option, so put `--` before values that might, as above.

A quoted result is withheld like any other helper's when an argument holds a secret parameter or a derived value.

### Structured formats

These helpers write a value as JSON, YAML, TOML or env-file text, such as a configuration file to [write into a volume](#volume), and read one back. Each decoder takes bytes as the encoding helpers do, so a bundle file decodes directly.

| helper | returns |
| --- | --- |
| `std::encode_json(value)` / `std::decode_json(text)` | indented JSON / the value it holds |
| `std::encode_yaml(value)` / `std::decode_yaml(text)` | block-style YAML / the value it holds |
| `std::encode_toml(map)` / `std::decode_toml(text)` | a TOML 1.0 document / the map it holds |
| `std::encode_env(map)` / `std::decode_env(text)` | `KEY='value'` lines / the map they hold |

```rhai
let settings = #{ server: #{ port: 8080, hosts: ["a.internal", "b.internal"] }, debug: false };
let toml = std::encode_toml(settings);
// debug = false
//
// [server]
// hosts = ["a.internal", "b.internal"]
// port = 8080

let defaults = std::decode_yaml(app.file("defaults.yaml"));
```

Every encoder writes map keys in sorted order and one fixed layout, so the same value always gives the same text. A value the format cannot hold throws, naming where it sits and why, rather than being dropped or converted: `()` in TOML or an env file, an infinite number in JSON, an array or nested map in an env file, and a Blob anywhere (encode it with one of the encodings first). A version is written as its text, and a regex as its pattern. YAML quotes any string an older YAML reader could take for something else, such as `no`, `on` or `0755`. An env file single-quotes every value, so a value holding a single quote or a line break throws.

Decoders are strict: text that is not a document of the format throws, naming the line it stops being one, and so does a key given twice or an integer too large for BSL. YAML is read under the YAML 1.2 core schema, so `yes` and `on` are strings, and a document holding more than one document, a custom tag or a non-string key throws. TOML date-times come back as strings. `std::decode_env` reads only blank lines, `#` comments and `KEY=value` entries with a bare or single-quoted value; double quotes, escapes, `$` expansion and `export` throw, because the readers of env files disagree on them.

Rhai's own `to_json` on a map throws, pointing at `std::encode_json`, which handles every value the same way the other encoders do.

An encoding of a value holding a secret is withheld like the secret, and each string decoded from a secret is withheld too.

## Gates

An app comes up from its resource graph alone: the runtime starts every static resource and keeps it running. Some changes need a step the graph cannot express, such as migrating a database before a new version serves. A _gate_ is such a step. The runtime keeps each gate satisfied by running its body whenever the gate's _key_ moves from the key of its last success, and holds back the workloads that depend on it until it is.

```rhai
let db = app.deployment("db").image(db_image.call());
let api = app.deployment("api").image(app_image.call()).after(db);

let schema_check = app.job("schema-check")
    .image(app_image.call())
    .command(["migrate", "--check"])
    .deadline("1m");

app.gate("schema")
    .key(version.value())
    .after(db)
    .exclusive(api)
    .check(schema_check)
    .run(|rt, last| {
        // `last` is the app as it stood at the gate's last success,
        // or `()` when there has been none.
        let from = if last == () { "none" } else { last.param("version").value() };
        rt.start(app.job()
            .image(app_image.call())
            .command(["migrate", "--from", from])
        ).terminated().ensure_success();
    });
```

`app.gate(name)` declares a gate and returns its builder; calling it again with the same name returns the same gate. Gates are declared in the static context only, and a gate is not a resource: `rt.start(gate)` throws.

- **`.key(value)`** sets the value that decides whether the gate is satisfied. The gate runs again whenever its key differs from the key of its last success. A key is a unit, boolean, integer, finite float, string, or an array or map of those, and two keys are the same when they are the same JSON value (map members compared in any order, and an integer never equal to a float). A gate with no `key` has the unit as its key, so it runs once and again only when its check fails.
- **`.run(|rt, last| ...)`** sets the body, taking up to two parameters. It may be set once. A gate without a body goes straight to its check.
- **`.check(job)` or `.check(job, #{ interval: duration })`** sets a named Job with a `deadline` as the gate's check. A check passes when the Job's instance exits 0; a failure, or the deadline, makes the gate unsatisfied until a run succeeds. The runtime runs the check after each run, before it starts a dependant workload that has no running instance, and every `interval` if one is given (whole seconds, at least 1s), whatever the gate's state, while no run of it is in progress; the interval counts from the last check, across restarts of the daemon. A check is how a gate notices a change made outside Seedling, such as a database restored from an older backup. A Job can be the check of only one gate, and the runtime starts it for nothing else. A gate with a check and no body waits on a condition alone, such as an external database becoming reachable.
- **`.exclusive(selection)`** names the Deployments and Jobs the runtime stops before the body runs, and keeps stopped until the gate is satisfied. A selection holding a dynamic resource throws.
- **`.after(target)`** orders the gate after a gate, a named Deployment, a named Job, or an array of these. `after` is also a method of Deployments and Jobs. The runtime starts no instance of a dependant workload, and runs no dependant gate, until each target is ready: a gate is ready when satisfied, a Deployment when one instance is Ready, a Job when its latest instance exited 0. A running instance is never stopped because a target is not ready. The evaluation fails where `after` calls form a cycle, and where a gate's exclusive selection holds one of its own targets or the Job of its check.

A workload ordered directly after a gate takes the gate's key as part of its configuration, so a running Deployment ordered after a gate is replaced by its update strategy once the gate is satisfied for a new key.

### Keys use the effective value

A key is evaluated with the app's definition and parameter values together, so `version.value()` is the version in effect: the stored value, the default, a value derived from another parameter, or one a new definition brought. A gate keyed on it moves whichever of those changed it, with no hook to write per way of changing. Parameters, definition updates and `apps update` initiate no operation: they write a generation, and each gate whose key moved is unsatisfied until it runs.

A key can carry a secret, such as a password a gate rotates: the runtime stores only a digest of a key, computed under the host's secret key, never the key itself.

### `last`

`last` is the app as it stood at the generation of the gate's last success: that definition, evaluated with that generation's parameter values, read-only. Use it to see where the gate is coming from, for example to upgrade a database from the release it was last migrated to rather than from the previous generation. A version that moved twice while a migration kept failing is still migrated from the release the database holds.

`last` is `()` when the runtime holds no success for the gate. That means the gate has not run, not that the data is new: volumes and external services kept across an uninstall still hold what they held. `last.file(...)` and `last.dir(...)` read that generation's bundle.

### How gates run

- A gate is _satisfied_ when its last success is for its current key and nothing has undone it since. A run starts by invalidating the last success, so a migration that fails part way leaves the gate unsatisfied and its exclusive selection stopped.
- The runtime pulls the images a run needs, stops the exclusive selection, waits for it to terminate, and runs the body. A selection that has not stopped within ten minutes fails the run, which is retried with back-off. The body directs the gate's own exclusive selection and the workloads ordered after the gate; `rt.start` in a body does not wait for the gate.
- The gate's check then runs. A run succeeds when the body returns and the check, if any, passes.
- What a body throws is recorded as how its run ended, and shows in the `gate_failed` fault and under `apps show`. Only a digest of the key is stored, but a thrown message is stored as it is, so keep secrets such as a rotated password out of it.
- A failed run is retried after a delay that starts at 30 seconds, doubles with each failure, and is capped at one hour. The runtime never gives up: it files a `gate_failed` fault and keeps retrying at the cap. A change of key makes the next run due at once, and a success resets the delay.
- Cancelling a gate run pauses the gate for its key. It stays paused until an operator runs it or its key moves.
- When the runtime cannot tell whether a gate is satisfied, because its record is unreadable or its check could not be run, the gate is neither satisfied nor unsatisfied: nothing runs, no dependant starts, and nothing is stopped.
- An app is _set up_, and leaves the `installing` status, once every gate it declares has been satisfied at the same time. An app with no gates is set up as soon as it is installed.
- Gates run one at a time with every other lifecycle operation on the host, in declaration order. A gate no longer declared keeps its record, so declaring it again under the same name resumes from it.

Operators see each gate's state under `apps show` and can run one with `seedling-ctl apps gate run <app> <gate>`, whether or not it is satisfied, for example to repeat a migration after restoring a database the check cannot see. A new app has no stored `last`, so installing an app runs each of its gates once, as for the first time.

## Healthchecks

A Deployment can declare a healthcheck so seedling knows when its container is actually ready to serve, and what to do when it stops being ready.

```rhai
app.deployment("api")
    .image("ghcr.io/example/api:v1")
    .http(8080, web.route("/"))
    .healthcheck(#{
        kind: "command",
        cmd: ["curl", "-fsS", "http://localhost:8080/healthz"],
        interval: "10s",      // between checks, whole seconds, at least 1s (default 30s)
        timeout: "3s",        // before a check is considered failed, whole seconds, at least 1s (default 30s)
        retries: 3,           // consecutive failures before transitioning to unhealthy (default 3)
        start_period: "15s",  // grace after start before failures count, whole seconds (default 0s)
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
    .stop_timeout("30s");     // hard kill after 30s
```

`stop_signal` accepts canonical (`"SIGTERM"`) or bare (`"TERM"`) forms; unknown signal names are a script-evaluation error. `stop_timeout` is a [duration](#durations) in whole seconds, at least 1s. The defaults match systemd's `TimeoutStopSec`.

A common pairing: PostgreSQL with `SIGINT` (fast shutdown) — the default `SIGTERM` triggers smart shutdown which waits for clients to disconnect and frequently exceeds the stop deadline.

## Resource priority

`deployment.priority(level)` declares how important a Deployment's workload is
when the host runs short of memory, CPU or I/O. It is an expression of intent:
the runtime decides how each level is realised, and the same definition carries
across platforms whose mechanisms differ.

```rhai
app.deployment("api")
    .image("ghcr.io/example/api:v1")
    .priority(Priority.Critical);   // last to be shed; largest share of contention

app.deployment("reports")
    .image("ghcr.io/example/reports:v1")
    .priority(Priority.Low);        // first to give way
```

Priority is declared on Deployments only. Calling it on a Job is an evaluation
error, and every workload that is not a Deployment runs at `Priority.Normal`.

There is a second lever the definition does not control: each installed app
carries an **app priority** (`high`, `normal`, `low`) that an operator sets with
`ctl apps priority <app> <level>` or from the web interface. The relative
standing of apps sharing a host is an operational decision, not one any single
definition can make for itself.

The two compose **app-major**: the app comparison decides first, and the
declared level only orders workloads belonging to equally-ranked apps. A
`Priority.Critical` Deployment in a `low`-priority app is therefore shed before a
`Priority.Normal` Deployment in a `high`-priority app. Both levers are relative —
they decide how contention is resolved, not how much a workload may use when
capacity is free. For hard per-container ceilings use `container.memory` and
`container.cpus`.

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

## Writing files at action time: `rt.write`

Static `Volume.write(...)` declares files that the runtime writes into the volume when it creates it: once for a named volume, whose existing contents are never overwritten, and each time a tmpfs volume's directory is created. `rt.write(volume, path, contents)` is its action-time sibling: a one-shot write performed during the action, useful for seeding parameter-derived configuration, dropping a marker file, or staging input for a follow-up command:

```rhai
let cfg = app.volume("config");

app.on_action("regenerate-config", |rt, _p| {
    rt.write(cfg, "/app.conf", `host=${host.value()}\nport=${port.value()}\n`);
    rt.restart(app.deployment("api"));
}, #{ description: "Regenerate /app.conf from current params and restart the API" });
```

Behaviour:

- Target may be a named or anonymous `Volume`, or an `ExternalVolume` that resolves to storage, through an operation-scoped `kind: "volume"` param or the host's external volume mappings.
- The call throws for an `ExternalVolume` that resolves to nothing, for one whose binding or mapping is read-only, and for a `Volume` declared read-only.
- Path validation matches static `Volume.write`: absolute, no `..`, must not be the volume root.
- The call is at-most-once across replays.
- Unlike static `Volume.write`, which lands when the volume is created, `rt.write` lands once, when it is called. For tmpfs volumes the file is wiped at the next container start; that's allowed and is up to the script author to reason about.
- Calling `rt.write` outside an action closure is a script error.

## Reporting to Canopy: `app.canopy_type`

`app.canopy_type(type)` declares what the app is to Canopy:

```rhai
app.canopy_type("seedling-demo");
```

Seedling then reports the app to Canopy as an application of that type, keyed by the app's name, with its status as health and its faults as detail. A type is lowercase letters and digits in hyphen-separated segments, beginning with a letter; anything else throws. It is declared at the top level only, never inside an action. An app that does not call it is not reported as an application. A type is also what lets Seedling ask Canopy for the certificates of the app's ingress hostnames (see [Deploying](deploying.md#certificates-from-canopy)). See [Deploying](deploying.md#reaching-canopy-through-bestool) for what reporting needs on the host.

## Definition bundles

A definition is a *bundle*: a folder holding the script and any files it needs, such as error pages or seed data. A lone script pushed with `seedling-ctl` becomes a bundle holding that file under its own name, with a `seedling.toml` naming it as the script.

```
web/
├── seedling.toml        # optional
├── app.seed.rhai
└── pages/
    ├── 500.html
    └── logo.png
```

`seedling.toml` is optional and has four optional fields:

```toml
seedling = ">=0.13, <0.15"          # Seedling versions this definition supports; `||` separates alternatives
script = ["lib.rhai", "app.seed.rhai"] # script files, concatenated in order; default ["app.seed.rhai"]

[metadata]                           # free-form values for the script; Seedling does not interpret them
version = "2.12.0"

[include]                            # files and folders copied in from elsewhere in the repository
"lib.rhai" = "../common/lib.rhai"
"pages" = "../common/pages"
```

A definition for a Seedling version the host is not running is refused when it is installed. An error in any script file is reported against that file and its own line number.

`app.metadata()` returns the `[metadata]` table as a map, or an empty map when there is none; a key it does not hold reads as `()`. Strings, integers, floats, booleans, arrays and tables arrive as their BSL counterparts, and dates and times as strings. A float that is not finite (`inf`, `nan`) is refused when the definition is installed. The table is also shown with the definition's provenance, so a definition can carry the release it was published for and default to it:

```rhai
let published = app.metadata().version;           // () if the definition declares none
let version = app.param("version").kind("version-semver");
if published != () {
    version.default_value(published);
    let major = std::parse_version_semver(published).major();
    version.validate(|value, all| {
        if value.major() != major { throw `this definition runs ${major}.x`; }
    });
}
```

Inside a gate's body, `last.metadata()` reads the table of the definition at the gate's last success.

Every other file is a *sidecar* the script can read:

```rhai
let pages = app.volume("error-pages")
    .write_dir("/", app.dir("pages"));            // every file under pages/, byte for byte

app.volume("seed").write("/seed.json", app.file("seed.json"));

let banner = app.file("banner.txt").text();       // throws if the file is not UTF-8
```

- `app.file(path)` returns a `File`; `app.dir(path)` returns a `Directory` of every file beneath `path`. Paths are relative to the bundle root and cannot escape it; a missing file, or a folder with nothing in it, throws.
- Both read the bundle the app was evaluated from, in any context.
- `volume.write` and `rt.write` accept a `File` as well as a string. `volume.write_dir(path, dir)` writes a whole directory, and may target the volume root.

Bundles are limited to regular files and folders, 2 MiB in total. The [bundle-tour demo](../demos/bundle-tour/) uses every field of `seedling.toml`, sidecar files and includes. See [Deploying](deploying.md#app-definitions) for pushing a folder and publishing a bundle to a registry.

### Sharing files between definitions

Sibling definitions in one repository share helpers and sidecars through `[include]`. Each key is where the file or folder lands in the bundle; each value is where `seedling-ctl` copies it from, relative to the definition folder. A shared script joins the definition by being listed in `script`, ahead of the files that use it, so it runs in the same scope as they do.

`seedling-ctl` copies included files in when it reads the folder, so the host only ever sees a self-contained bundle; the host refuses a bundle whose `[include]` names a path it does not hold, as a folder published without `seedling-ctl definitions pack` would. Sources are confined:

- to the repository the folder sits in (the nearest folder above holding `.git` or `.jj`), or to the definition folder itself outside a repository;
- for a GitHub URL, to that repository at the same commit;
- never through a symlink, and never into `.git` or `.jj`.

The [bundle-tour](../demos/bundle-tour/) and [bundle-share](../demos/bundle-share/) demos include the same helper script and pages from `demos/shared/`.

An included folder is read like a definition folder, honouring its own `.seedignore`, and may not hold a `seedling.toml`: only the definition folder's own is read. A copy already in the folder must match its source; where the source cannot be reached, as when pushing a folder written by `apps export`, the copy is used as it stands.

## Running a command inside a container: `rt.exec`

`rt.exec(target, argv, options?)` runs a command in an existing running container without paying the cost of starting a new one for each invocation. Useful for sequences of commands that share a container's filesystem, or for poking a management endpoint:

```rhai
app.on_action("seed-db", |rt, _p| {
    let host = app.job()
        .image("ghcr.io/example/dbtools:1.0")
        .command(IDLE_CMD);
    let started = rt.start(host);

    rt.exec(started, ["create-schema"]).ensure_success();
    rt.exec(started, ["load-fixtures"], #{
        env: #{ FIXTURE_SET: "smoke" },
    }).ensure_success();

    rt.stop(started);
});
```

The returned `Executed` exposes:

- `executed.exit_code()` → int
- `executed.success()` → bool (true iff exit code is 0)
- `executed.ensure_success()` → throws with the exit code if non-zero

Discarding the return value is fine if the script doesn't care about the result; the command runs either way.

`rt.exec` rules:

- The target must resolve to exactly one running container. The accepted forms are a `Started` (returned by an earlier `rt.start`), a named `Deployment`, or a named `Job`.
- Anonymous `Deployment` / `Job` handles must be passed via the `Started` returned by `rt.start` — `rt.exec(app.job()...)` directly is a script error because there is no instance yet to target.
- `Deployment.scale(N)` with `N > 1` (or any range whose upper bound is `> 1`) is rejected without consulting the registry. Use a `Job` or split the work into a single-replica deployment when you need exec.
- `argv` must be a non-empty array of strings.
- `options.env` is a string-to-string map of env-var overrides layered on top of the container's own environment. Keys must be POSIX env names (`[A-Za-z_][A-Za-z0-9_]*`). Other option keys are reserved.
- Stdout and stderr go to the container's log stream; they are not captured by the script.
- The call is at-most-once across replays: the recorded exit code is recovered from the action log on replay rather than re-running the command.

The `IDLE_CMD` constant is provided as a sugar for the "container as exec host" pattern: it runs `sleep infinity` under a shell that exits on SIGTERM or SIGINT, so the container starts, does nothing, stays alive while the action runs `rt.exec` against it, and stops at once. The image needs a POSIX shell and a `sleep` that accepts `infinity`.

## Action params with `kind: "volume"`

Action and shell param schemas accept a special `kind: "volume"` that asks the operator to pick a site volume at invocation time. The runtime resolves the operator's choice into an operation-scoped binding and delivers the binding's name in `param["<name>_volume"]`. The closure consumes it via `app.external_volume(...)`:

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

`kind: "volume"` is valid only on action and shell schemas. `app.param(...)` rejects it because its binding outlives any single operation; for it, declare an `app.external_volume(name)` and have the operator wire it up via the UI's external volume mappings.

## Invoking other actions

`Action.invoke(params?)` runs another action's closure inline, in the calling closure's runtime. Get the handle either by capturing the return of `app.on_action(...)` or by looking it up by name with `app.action(name)`. Both produce the same `Action` value. `app.action(name)` is only valid in dynamic context — it throws when called from the static top-level.

`params` follows the called action's declared schema: required fields must be present, defaults are applied to absent optional fields, reserved-suffix keys (`*_volume`, `*_filename`) are rejected, undeclared keys pass through as given, and `kind: "volume"` references must resolve to a real site volume. A `boolean` or `integer` param may be given as a boolean or integer as well as its text, and a `Version` or `SemVer` is taken as its text, so a closure can pass on the param map it received. Validation happens before the closure runs, and errors throw with the field name. Operator-invoked actions get the same validation, so a sub-action invocation behaves exactly like an operator dispatching the action manually.

```rhai
let migrate = app.on_action("migrate", |rt, p| {
    rt.start(app.job()
        .image(image.call())
        .command(["migrate", "--batch-size", p["batch-size"]])
    ).terminated().ensure_success();
}, #{
    params: #{
        "batch-size": #{ kind: "text", default_value: "1000" },
    },
});

app.on_action("seed", |rt, _p| {
    // Captured handle — same as the return of on_action above.
    migrate.invoke();
    // Or by name, in dynamic context only.
    app.action("migrate").invoke(#{ "batch-size": "100" });
});

app.on_action("reseed", |rt, _p| {
    rt.start(app.job().image(seed_image).command(["seed-db"]))
        .terminated()
        .ensure_success();
    migrate.invoke();
});
```

The method is named `invoke` rather than `call` because the scripting engine reserves `obj.call(...)` for function-pointer dispatch.

A few things invoke does *not* do:

- It does not start a new operation. The called closure shares the caller's `rt`, operation id, and action log; nothing in `apps history` shows it as a separate top-level entry, but the runtime records a `SubActionInvoked` log entry with the action name and validated params so the nested chain is inspectable in the operation's history.
- It does not allow recursion. Calling an action that is already on the active call stack — directly (`bar` invoking `bar`) or transitively (`foo` → `bar` → `foo`) — throws before the closure runs. The error names the chain.

For code reuse that does *not* need a param schema or a history entry, extract a top-level Rhai closure and `.call(rt, ...)` it directly. The two patterns coexist: `Action.invoke()` for "a peer action's closure with its own validation surface", a plain Rhai closure for "shared steps that aren't actions in their own right".

> **Note on `rt.start(app)`**: this schedules every static resource the App owns and (with `.ready()`) blocks until they're all up. It does *not* invoke any custom action closure. Outside a gate body it leaves out the resources a gate holds and the workloads ordered after a gate that is not yet satisfied; the runtime starts those itself once the gate is satisfied.

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
