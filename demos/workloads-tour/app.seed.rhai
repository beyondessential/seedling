// Demo: workloads tour.
//
// Every workload and storage feature in one app. Deployments show fixed and
// ranged scale, rolling and replacing updates, recreation on termination,
// command health checks that replace or only monitor, and the four priority
// levels. Containers show commands and arguments, environment, resource
// limits, capabilities, a writable root, users, working directories, stop
// signals and timeouts, and the three exit strategies. Pods show HTTP, TCP and
// UDP bindings, service mounts and start order. A Job runs past its deadline.
// Volumes come named, anonymous, read-only, tmpfs and external, with declared
// file writes, and the host constants size and shape what runs.
//
// Browse https://workloads-tour.demo.internal/report for what the containers
// see from inside. Not a production definition.

app.description("Demo: deployments, jobs, containers, pods and volumes, every feature of each");
app.canopy_type("seedling-demo");

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("workloads-tour.demo.internal")
    .description("Hostname the tour answers on");

// Most deployments read this into their environment, so changing it changes
// their definitions and sets each one's update strategy going:
// `seedling-ctl apps params set workloads-tour revision=2`.
let revision = app.param("revision")
    .kind("text")
    .default_value("1")
    .description("Bump to roll the deployments and watch each update strategy");

let alpine = "docker.io/library/alpine:3.24.4";

// CPU_ARCHITECTURE: these images are published for some architectures only.
// On any other, including one this script does not recognise, the workloads
// that need them are left out rather than failing to pull.
let whoami_arches = ["x86_64", "aarch64", "arm"];
let socat_arches = ["x86_64", "aarch64", "arm"];

// AVAILABLE_THREADS and AVAILABLE_MEMORY size the frontend's scale ceiling and
// the inspector's limits from the host the app is evaluated on.
let frontend_max = if AVAILABLE_THREADS < 2 { 2 } else if AVAILABLE_THREADS > 6 { 6 } else { AVAILABLE_THREADS };
let inspector_memory = AVAILABLE_MEMORY / 64;
if inspector_memory < std::parse_quantity("32Mi") { inspector_memory = std::parse_quantity("32Mi"); }
if inspector_memory > std::parse_quantity("128Mi") { inspector_memory = std::parse_quantity("128Mi"); }
let inspector_cpus = if AVAILABLE_THREADS >= 4 { 1.0 } else { 0.5 };

// HOST_HAS_IPV4 with NAT64_ACTIVE says whether this host itself carries the
// pods' traffic to IPv4 destinations, since pod networks are IPv6 only.
let ipv4_destinations = if HOST_HAS_IPV4 && NAT64_ACTIVE {
    "reachable through this host's NAT64"
} else {
    "reachable only where the network provides NAT64 and DNS64"
};
let ipv6_destinations = if HOST_HAS_IPV6 { "reachable" } else { "unreachable: no IPv6 egress" };

let web = app.service("web").http(80);
web.ingress(hostname.value(), 443)
    .tls(Terminate.Https, Output.Http1)
    .redirect();

// A plain service carrying one TCP and one UDP port. Nothing outside the app
// reaches it; the inspector mounts it, and an operator can forward to it with
// `seedling-ctl apps forward workloads-tour echo 7000` (add `--proto udp` for
// 7001) and talk to it with `nc`.
let echo = app.service("echo");

// A minimal HTTP server: busybox nc hands each connection to this script.
// `POST .../health` with a status code in the body sets what the instance's
// health check sees; `GET .../health` answers with it. Each instance holds its
// own, so the reply names the instance that took the request.
let handler = `read -r method path _
length=0
while read -r line; do
    line=$(printf '%s' "$line" | tr -d '\r')
    [ -z "$line" ] && break
    case "$line" in
        [Cc]ontent-[Ll]ength:*) length=$(printf '%s' "$line" | cut -d: -f2 | tr -d ' ') ;;
    esac
done
body=
[ "$length" -gt 0 ] 2>/dev/null && body=$(head -c "$length")

respond() {
    printf 'HTTP/1.1 %s Demo\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: %s\r\nConnection: close\r\n\r\n%s\n' \
        "$1" "$(( $(printf '%s\n' "$2" | wc -c) ))" "$2"
}

health=$(cat /tmp/health)
case "$method $path" in
    "POST "*/health)
        case "$body" in
            [2-5][0-9][0-9])
                echo "$body" > /tmp/health
                respond 200 "$(hostname) now answers its health check with $body" ;;
            *) respond 400 "POST a status code from 200 to 599" ;;
        esac ;;
    "GET "*/health) respond "$health" "$(hostname) health $health" ;;
    "GET /report"*) respond 200 "$(sh /tmp/report.sh 2>&1)" ;;
    "GET /share"*) respond 200 "$(ls -la /share 2>&1)" ;;
    *) respond 200 "$(hostname) revision $REVISION health $health ($method $path)" ;;
esac`;

// The shell stays as the main process so that it can act on SIGTERM; a bare
// nc as the main process would ignore it and wait out the stop timeout.
// `setup` runs first, in the same shell.
let serve = |port, setup| `${setup}
echo 200 > /tmp/health
trap 'exit 0' TERM
nc -lk -p ${port} -e sh -c ${std::quote_shell(handler)} &
wait $!`;

// Named volume, read-only: the declared file writes are applied once, when
// the runtime creates the volume, and any container mounting it gets it
// read-only. A later definition that changes the text does not rewrite it.
// The runtime creates each written file 0640 and its parent folders as
// needed; the report lists them.
let site = app.volume("site")
    .readonly()
    .write("/motd.txt", "Written by the runtime when it created this volume, and never again.\n")
    .write("/conf/build.txt", `First evaluated on ${NODE_NAME} (${CPU_ARCHITECTURE}), ${AVAILABLE_THREADS} threads, ${AVAILABLE_MEMORY} bytes.` + "\n");

// Named volume, writable and persistent. HAS_SNAPSHOTS says whether a backup
// of it can be taken from a copy-on-write snapshot without stopping writers.
let state = app.volume("state").description(if HAS_SNAPSHOTS {
    "Prepared state; the host can snapshot it"
} else {
    "Prepared state; the host cannot snapshot it, so stop writers to back it up"
});

// Named tmpfs volume: held in memory, so its contents do not survive a host
// restart, and its declared writes are applied again each time the runtime
// creates its directory, which a restart of the runtime alone does not do.
let cache = app.volume("cache")
    .tmpfs()
    .write("/seeded.txt", "Seeded into memory-backed storage.\n");

// The inspector writes this into its own /tmp when it starts, so the report
// always matches the definition the instance runs.
let report = `section() { printf '\n== %s\n' "$1"; }

echo "workloads-tour inspector, instance $(hostname)"

section "identity: container.user, container.workdir"
id
echo "working directory: $(pwd)"

section "environment: container.env and the host constants"
env | grep -v -e '^PATH=' -e '^HOME=' -e '^SHLVL=' -e '^PWD=' -e '^HOSTNAME=' -e '^container=' | sort

section "limits: container.memory, container.cpus, container.pids_limit"
for f in memory.max cpu.max pids.max; do
    printf '%-11s %s\n' "$f" "$(cat /sys/fs/cgroup/$f 2>/dev/null || echo unavailable)"
done

section "privileges: container.cap_add, container.writable_rootfs"
grep -e '^CapEff' -e '^NoNewPrivs' /proc/self/status
probe=$(mktemp)
if chown 65534:65534 "$probe" 2>/dev/null; then
    echo "chown: allowed, as the CHOWN capability was granted"
else
    echo "chown: refused"
fi
rm -f "$probe"
if touch /rootfs-probe 2>/dev/null; then
    echo "root filesystem: writable"
else
    echo "root filesystem: read-only, with /tmp writable"
fi

section "mounts: volumes and their storage options"
grep -e ' /srv ' -e ' /state ' -e ' /cache ' -e ' /tmp ' /proc/mounts | cut -d' ' -f2-4

section "declared file writes in the site and cache volumes"
ls -lR /srv /cache
cat /srv/motd.txt /srv/conf/build.txt /cache/seeded.txt

section "state volume, written by the prepare job before this instance started"
cat /state/prepared 2>/dev/null || echo "nothing yet"

section "service mounts: pod.mount, reached at localmount"
for proto in tcp udp; do
    if [ "$proto" = tcp ]; then port=7000; flag=; else port=7001; flag=-u; fi
    out=$(echo ping | nc $flag -w 1 localmount $port 2>&1)
    echo "$proto $port:"
    if [ -n "$out" ]; then echo "$out"; else echo "no answer"; fi
done
`;

// External volume: the app names it and an operator supplies the storage.
// Only share-reader mounts it, and share-reader runs no instance until an
// operator scales it up, so an unmapped share holds no fault. Map one with
// `seedling-ctl volumes site create-managed demo-share` and
// `seedling-ctl apps volumes attach workloads-tour site-share _site/demo-share`,
// then scale share-reader to 1 and browse /share. Scaling it up unmapped
// files `external_volume_not_mapped` and starts nothing.
let share = app.external_volume("site-share")
    .description("Any site volume; share-reader lists it at /share");

// A static Job holds at most one instance, which `seedling-ctl apps show`
// lists with a display name ending in 00000000. It runs once to completion,
// is not restarted once it exits, and its exit 0 is what lets the workloads
// ordered after it start. OnExit.Terminate is a Job's default.
let prepare = app.job("prepare")
    .image(alpine)
    .command(["sh", "-c", "date -u '+prepared at %Y-%m-%d %H:%M:%S UTC' > /state/prepared"])
    .mount("/state", state)
    .deadline("1m");

// Job deadline: the runtime stops the instance 30 seconds after it reaches
// Running, sending the default SIGTERM, which the script logs. See it with
// `seedling-ctl apps logs workloads-tour overrun`.
app.job("overrun")
    .image(alpine)
    .command(["sh", "-c", `trap 'echo "overrun: SIGTERM from the runtime after $n seconds; the deadline is 30"; exit 143' TERM
echo "overrun: started; would run for 600 seconds, past its 30 second deadline"
n=0
while [ "$n" -lt 600 ]; do sleep 1 & wait $!; n=$((n + 1)); done`])
    .on_exit(OnExit.Terminate)
    .deadline("30s");

// The front page. Scale is a range: it starts at the lower bound of 2 and an
// operator may move it within the bounds with
// `seedling-ctl apps scale workloads-tour frontend up`.
//
// Health check, replace: flip one instance with
// `curl -k -X POST https://workloads-tour.demo.internal/health -d 500`; the
// reply names the instance that took it. Three failed checks five seconds
// apart make it unhealthy, and once its grace window has passed
// (start_period + retries x interval, 25 seconds from its start) the runtime
// starts a fresh instance alongside it, routes to the healthy ones, and
// retires it once the fresh one is healthy.
//
// Rolling update: after `apps params set workloads-tour revision=2` or
// `apps restart workloads-tour frontend`, `apps show` lists one instance above
// scale while each fresh instance becomes healthy before a stale one stops.
//
// `arg` with no `command` replaces the image's default arguments and keeps
// its entrypoint, of which alpine has none.
let frontend = app.deployment("frontend")
    .image(alpine)
    .arg(["sh", "-c", serve.call(8080, "")])
    .env("REVISION", revision.value())
    .memory("32Mi")
    .scale(2..=frontend_max)
    .on_update(OnUpdate.Rolling)
    .on_terminate(OnTerminate.Recreate)
    .on_exit(OnExit.Restart)
    .priority(Priority.Critical)
    .healthcheck(#{
        kind: "command",
        cmd: `test "$(cat /tmp/health)" = 200`,
        interval: "5s",
        timeout: "2s",
        retries: 3,
        start_period: "10s",
        on_failure: "replace",
    })
    .http(8080, web)
    .after(prepare);

// Health check, monitor: flip it with
// `curl -k -X POST https://workloads-tour.demo.internal/monitored/health -d 503`.
// It is never replaced. A `health_check_failed` fault appears in
// `seedling-ctl faults --app workloads-tour`, and with no healthy sibling it
// stays in the routing pool in degraded mode. POST 200 to recover it.
// An array `cmd` runs directly, with no shell.
app.deployment("monitored")
    .image(alpine)
    .command(["sh", "-c"])
    .arg(serve.call(8080, ""))
    .env("REVISION", revision.value())
    .scale(1)
    .priority(Priority.Elevated)
    .healthcheck(#{
        kind: "command",
        cmd: ["grep", "-qx", "200", "/tmp/health"],
        interval: "10s",
        timeout: "3s",
        retries: 2,
        start_period: "5s",
        on_failure: "monitor",
    })
    .http(8080, web.route("/monitored"))
    .after(frontend);

// Serves /report, generated per request from inside the container: its user,
// working directory, environment, cgroup limits, capabilities, mounts, the
// declared file writes, and answers from the echo service through its mounts.
// Ordered after both the prepare Job and the frontend Deployment.
app.deployment("inspector")
    .image(alpine)
    .command("sh")
    .arg(["-c", serve.call(8081, `printf '%s' ${std::quote_shell(report)} > /tmp/report.sh`)])
    .env("REVISION", revision.value())
    .env("TZ", TIMEZONE)
    .env([
        #{ name: "NODE_NAME", value: NODE_NAME },
        #{ name: "CPU_ARCHITECTURE", value: CPU_ARCHITECTURE },
        #{ name: "AVAILABLE_THREADS", value: `${AVAILABLE_THREADS}` },
        #{ name: "AVAILABLE_MEMORY", value: `${AVAILABLE_MEMORY}` },
        #{ name: "HOST_HAS_IPV4", value: `${HOST_HAS_IPV4}` },
        #{ name: "HOST_HAS_IPV6", value: `${HOST_HAS_IPV6}` },
        #{ name: "NAT64_ACTIVE", value: `${NAT64_ACTIVE}` },
        #{ name: "HAS_SNAPSHOTS", value: `${HAS_SNAPSHOTS}` },
        #{ name: "IPV4_DESTINATIONS", value: ipv4_destinations },
        #{ name: "IPV6_DESTINATIONS", value: ipv6_destinations },
    ])
    .memory(inspector_memory)
    .cpus(inspector_cpus)
    .pids_limit(64)
    .cap_add("CHOWN")
    .workdir("/state")
    .mount("/srv", site)
    .mount("/state", state)
    .mount("/cache", cache)
    .mount(echo.port(7000))
    .mount(echo.port(7001))
    .priority(Priority.Normal)
    .http(8081, web.route("/report"))
    .after([prepare, frontend]);

// `lower..end` excludes `end`, so this scale is 0 to 1, and starts at 0:
// `seedling-ctl apps scale workloads-tour share-reader to 1` starts it.
app.deployment("share-reader")
    .image(alpine)
    .arg(["sh", "-c", serve.call(8080, "")])
    .mount("/share", share)
    .scale(0..2)
    .http(8080, web.route("/share"));

// Stop signal: the script exits on SIGUSR1 and ignores SIGTERM, so a prompt,
// logged exit when it is stopped (`apps stop-resource workloads-tour
// deployment lifecycle`, or a revision change) shows the runtime sent
// SIGUSR1. stop_timeout bounds the wait before SIGKILL.
//
// Exit strategies: every 10 minutes the script exits with status 0.
// OnExit.RestartOnFailure leaves a clean exit alone, and OnTerminate.Recreate
// then has the runtime recreate the container as a deliberate restart, which
// `seedling-ctl restarts list --app workloads-tour` records and the crash-loop
// count ignores.
//
// writable_rootfs: the script keeps its uptime under /var/lib, outside /tmp.
app.deployment("lifecycle")
    .image(alpine)
    .command("sh")
    .arg("-c")
    .arg(`trap 'echo "lifecycle: caught SIGUSR1, its stop signal; exiting cleanly"; exit 0' USR1
mkdir -p /var/lib/lifecycle
echo "lifecycle: started; exits by itself with status 0 after 600 seconds"
n=0
while [ "$n" -lt 600 ]; do
    sleep 1 & wait $!
    n=$((n + 1))
    echo "$n" > /var/lib/lifecycle/seconds-up
done
echo "lifecycle: 600 seconds up, exiting with status 0"`)
    .env("REVISION", revision.value())
    .writable_rootfs()
    .stop_signal("SIGUSR1")
    .stop_timeout("20s")
    .on_exit(OnExit.RestartOnFailure)
    .on_terminate(OnTerminate.Recreate);

// IDLE_CMD runs a process that does nothing until the container is asked to
// stop, then exits at once rather than waiting out the stop timeout. It runs
// as an unprivileged uid:gid.
app.deployment("idle")
    .image(alpine)
    .command(IDLE_CMD)
    .user("65534:65534")
    .memory(16 * 1024 * 1024)
    .stop_timeout("10s");

// TCP and UDP echo, run as the image's `nobody` user by name; each reply
// starts with the user and instance that sent it. The TCP binding attaches
// service port 7000 to pod port 17000; the UDP binding passes the service,
// which binds the same port number on both sides.
if CPU_ARCHITECTURE in socat_arches {
    app.deployment("echo-server")
        .image("docker.io/alpine/socat:1.8.1.3")
        .command(["sh", "-c", `socat -T 5 UDP6-RECVFROM:7001,fork SYSTEM:"echo udp echo from \$(id -un) on \$(hostname); cat" &
exec socat TCP6-LISTEN:17000,fork,reuseaddr SYSTEM:"echo tcp echo from \$(id -un) on \$(hostname); cat"`])
        .user("nobody")
        .memory("32Mi")
        .cpus(0.25)
        .pids_limit(32)
        .tcp(17000, echo.port(7000))
        .udp(7001, echo);
}

// Replace update: after a revision change or `apps restart workloads-tour
// batch`, both instances stop before either fresh one starts, so /whoami
// answers 502 for a moment. A fixed scale of 2 and the lowest priority, the
// first workload shed under memory pressure; `systemctl show` on its units
// gives the highest OOMScoreAdjust of the app's.
if CPU_ARCHITECTURE in whoami_arches {
    app.deployment("batch")
        .image("docker.io/traefik/whoami:v1.12.0")
        .arg(["--port", "8082", "--name", NODE_NAME])
        .env("REVISION", revision.value())
        .memory("32Mi")
        .cpus(0.25)
        .scale(2)
        .on_update(OnUpdate.Replace)
        .priority(Priority.Low)
        .http(8082, web.route("/whoami"));
}

// Anonymous resources exist only inside an action, for the operation that
// creates them. Each run of this action creates its own instance of the
// anonymous Job, with its own anonymous tmpfs volume seeded by a declared
// write, and the runtime removes both once the run ends. Run it twice with
// `seedling-ctl apps action workloads-tour scratch` and compare the instance
// names in `seedling-ctl apps logs workloads-tour`.
app.on_action("scratch", |rt, _p| {
    let scratch = app.volume()
        .tmpfs()
        .write("/input.txt", "Seeded for this run alone.\n");
    let job = app.job()
        .description("Reads the anonymous volume seeded for this run")
        .image(alpine)
        .command(["sh", "-c", `cat /scratch/input.txt; grep ' /scratch ' /proc/mounts; echo "instance $(hostname)"`])
        .mount("/scratch", scratch)
        .deadline("1m");
    rt.start(job).terminated().ensure_success();
}, #{ description: "Run an anonymous Job against an anonymous tmpfs volume" });
