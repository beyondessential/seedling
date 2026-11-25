// Demo: paired-consumer.
//
// The half of the paired demo that uses what another app offers: two external
// services and an external volume, slots the definition names and an operator
// fills by mapping them to paired-provider's exported services and volume, or
// to a site service of the operator's own. Deployments mount the external
// services' ports and the external volume, and a page on an HTTPS ingress
// shows what they read through each, refreshed every 10 seconds. Two actions
// write into the external volume and resolve an external volume the
// definition never declares. Not a production definition.
//
// Installed with nothing mapped, the page still answers: the services part
// reports that nothing answered, and the volume part keeps its placeholder,
// because the runtime does not start the reader while its external volume is
// unmapped and holds an external_volume_not_mapped fault instead.
//
// The hostname must resolve to the host, through /etc/hosts or curl's
// `--resolve <name>:443:<host address>`, and its certificate comes from the
// proxy's internal CA, so pass `-k` to curl unless the client trusts it.
//
// Try on a dev VM, with paired-provider installed from its folder:
//   seedling-ctl apps create paired-consumer demos/paired-consumer
//   seedling-ctl apps install paired-consumer
//   seedling-ctl services external declared   # upstream and echo, unmapped
//   seedling-ctl volumes external declared    # shared, unmapped
//   seedling-ctl apps show paired-consumer    # the external_volume_not_mapped fault
//
// Map the slots to paired-provider, the volume read-only:
//   seedling-ctl services external map paired-consumer upstream paired-provider/api
//   seedling-ctl services external map paired-consumer echo paired-provider/echo
//   seedling-ctl volumes external map --read-only paired-consumer shared paired-provider/shared
//   seedling-ctl services external list
//   seedling-ctl volumes external list
//   curl -k https://paired-consumer.demo.internal/
//   seedling-ctl apps params set paired-provider message="something new"   # the page follows
//
// Write into the provider's volume, refused while the mapping is read-only:
//   seedling-ctl apps action paired-consumer leave-note note="hello, provider"
//   seedling-ctl volumes external remap paired-consumer shared paired-provider/shared
//   seedling-ctl apps action paired-consumer leave-note note="hello, provider"
//
// Point upstream at an arbitrary endpoint through a site service, then back:
//   seedling-ctl services site create demo-upstream --endpoint 80/http=example.com:80
//   seedling-ctl services external remap paired-consumer upstream _site/demo-upstream
//   seedling-ctl services external remap paired-consumer upstream paired-provider/api
//   seedling-ctl services site delete demo-upstream
//
// Map a name the definition never declares, and read it from an action:
//   seedling-ctl volumes site create-managed demo-scratch
//   seedling-ctl volumes external map paired-consumer scratch _site/demo-scratch
//   seedling-ctl apps action paired-consumer peek volume=scratch
//   seedling-ctl apps logs paired-consumer
//   seedling-ctl volumes external unmap paired-consumer scratch
//
// Unmapping a declared slot is refused while the definition declares it; an
// operator retargets it with remap instead.

app.description("Demo: uses paired-provider's exported services and volume through external services, an external volume and service mounts");
app.canopy_type("seedling-demo");

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("paired-consumer.demo.internal")
    .description("Hostname the consumer's page answers on");

let socat = "docker.io/alpine/socat:1.8.1.3";

// External services: slots the operator maps to another app's service or to a
// site service. The description is what operators see listed against the slot.
let upstream = app.external_service("upstream")
    .description("An HTTP server on port 80; map it to paired-provider/api");
let echo = app.external_service("echo")
    .description("A TCP echo server on port 7000; map it to paired-provider/echo");

// An external service takes the HTTP view, routes, redirects and balancing a
// service does. No ingress can be declared on an external service, so nothing
// serves these settings; they are here so the definition exercises them.
upstream.balance(#{ policy: "first", try_duration: "2s" });
let upstream_http = upstream.http(80);
upstream_http.route("/v1").redirect("/v2", 308);

// An external volume: storage the operator supplies by mapping it, here to
// paired-provider's exported volume.
let provider_volume = app.external_volume("shared")
    .description("paired-provider's shared volume; map it to paired-provider/shared");

// The page volume is the consumer's own, held in memory: its declared files
// are rewritten each time it is created, and the deployments below overwrite
// two of them while they run.
let page = app.volume("page")
    .tmpfs()
    .write("/index.html", `<!doctype html>
<meta charset="utf-8">
<title>paired-consumer</title>
<h1>paired-consumer</h1>
<p>What this app read from paired-provider, refreshed every 10 seconds.</p>
<h2>Through the external volume</h2>
<iframe src="/volume.txt" width="100%" height="320"></iframe>
<h2>Through the mounted external services</h2>
<iframe src="/services.txt" width="100%" height="480"></iframe>
`)
    .write("/volume.txt", "Nothing read yet: the reader starts once the external volume `shared` is mapped.\n")
    .write("/services.txt", "Nothing read yet: the caller has not finished its first round.\n");

let site = app.service("site").http(80);

site.ingress(hostname.value(), 443)
    .tls(Terminate.Https, Output.Http1)
    .redirect();

// Busybox httpd ignores SIGTERM as PID 1, so a shell runs it and exits on the
// signal. Alpine's busybox leaves httpd out; the busybox image has it.
app.deployment("web")
    .image("docker.io/library/busybox:1.38.0-musl")
    .command(["/bin/sh", "-c", "trap 'exit 0' TERM INT; httpd -f -p 8080 -h /www & wait"])
    .mount("/www", page)
    .http(8080, site.route("/"));

// Service mounts: each external service port is reachable inside this pod at
// the hostname `localmount`, on the service port, and reaches whatever the
// slot is mapped to. Unmapped, a connection to it gets no answer.
app.deployment("caller")
    .image(socat)
    .command(["/bin/sh", "-c", `
trap 'exit 0' TERM INT
while true; do
  {
    echo "== upstream: GET http://localmount:80/ =="
    wget -q -T 3 -O - http://localmount:80/ || echo "(no answer: is upstream mapped?)"
    echo
    echo "== echo: a line sent to localmount:7000 =="
    echo "hello through the mounted echo port" | socat -t 2 - TCP6:localmount:7000,connect-timeout=3 || echo "(no answer: is echo mapped?)"
    echo
    echo "read at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > /www/services.txt.tmp 2>&1
  mv /www/services.txt.tmp /www/services.txt
  sleep 10 &
  wait
done
`])
    .mount(upstream_http.port(80))
    .mount(echo.port(7000))
    .mount("/www", page);

// Mounting the external volume is what holds this deployment back while the
// slot is unmapped. Mapped read-only, it still reads the files and connects to
// the provider's socket, since connecting writes nothing to the volume.
app.deployment("reader")
    .image(socat)
    .command(["/bin/sh", "-c", `
trap 'exit 0' TERM INT
while true; do
  {
    echo "== shared/README.txt, declared by paired-provider's definition =="
    cat /shared/README.txt
    echo
    echo "== shared/status.txt, rewritten by paired-provider every 10 seconds =="
    cat /shared/status.txt
    echo
    echo "== shared/echo.sock, paired-provider's Unix socket =="
    echo "hello through the shared socket" | socat -t 2 - UNIX-CONNECT:/shared/echo.sock || echo "(no answer from the socket)"
    echo
    echo "== shared/notes/, written by the leave-note action =="
    cat /shared/notes/*.txt 2>/dev/null || echo "(none yet)"
    echo
    echo "read at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > /www/volume.txt.tmp 2>&1
  mv /www/volume.txt.tmp /www/volume.txt
  sleep 10 &
  wait
done
`])
    .mount("/shared", provider_volume)
    .mount("/www", page);

// In an action, `app.external_volume("shared")` refers to the static slot.
// Writing to it through its mapping throws while the mapping is read-only.
app.on_action("leave-note", |rt, p| {
    rt.write(app.external_volume("shared"), "/notes/paired-consumer.txt", `${p.note}\n`);
}, #{
    description: "Write a note into paired-provider's volume through the shared slot; needs a read-write mapping",
    params: #{
        note: #{
            kind: "text",
            default_value: "hello from paired-consumer",
            description: "The note's text",
        },
    },
});

// A name the definition does not declare resolves against the host's mappings
// when the action runs, so an operator can map one just for it. The job lists
// what it finds in the app's log.
app.on_action("peek", |rt, p| {
    let job = app.job()
        .image("docker.io/library/alpine:3.24.4")
        .command(["ls", "-lR", "/peek"])
        .mount("/peek", app.external_volume(p.volume));
    rt.start(job).terminated("2m").ensure_success();
}, #{
    description: "List the contents of an external volume, declared or only mapped, into the app's log",
    params: #{
        volume: #{
            kind: "text",
            default_value: "shared",
            description: "External volume name: shared, or any name mapped for this app",
        },
    },
});
