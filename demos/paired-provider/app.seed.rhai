// Demo: paired-provider.
//
// The half of the paired demo that offers things to another app, the way a
// database app offers its port and its socket directory: two exported
// services, one HTTP and one plain TCP, and an exported volume holding a
// declared file, a file a deployment keeps rewriting, and a Unix socket.
// paired-consumer reaches all of them once an operator maps its external
// service and volume slots here; its header lists the steps. Not a production
// definition.
//
// Try on a dev VM:
//   seedling-ctl apps create paired-provider demos/paired-provider
//   seedling-ctl apps install paired-provider
//   seedling-ctl services exported list   # api with its description, echo without
//   seedling-ctl services app list        # every named service, with its exported flag
//   seedling-ctl volumes exported         # shared, with its description and host path
//   seedling-ctl volumes shell paired-provider/shared

app.description("Demo: exports an HTTP service, a TCP echo service and a volume for paired-consumer to use");
app.canopy_type("seedling-demo");

let message = app.param("message")
    .kind("text")
    .default_value("hello from paired-provider")
    .description("Line the writer puts in shared/status.txt; change it to watch paired-consumer's page follow");

// An exported service is listed to operators with its description, as a
// candidate for other apps' external services. Exporting only lists it: a
// mapping may target any named service an app declares.
let api = app.service("api")
    .exported(#{ description: "Request echo over HTTP on port 80" })
    .http(80);

// Exported with no description: listed all the same.
let echo = app.service("echo").exported();

// An exported volume is listed with its description and host path, and is the
// only kind of app volume another app's external volume may be mapped to.
// The declared file is written once, when the volume is first created.
let shared_volume = app.volume("shared")
    .exported(#{ description: "Files and a Unix socket paired-provider shares" })
    .write("/README.txt", "Written by paired-provider's definition when this volume was created.\n");

// The api service's port 80 is what a client connects to, including a
// consumer that mounts it; the pod listens on 8080.
app.deployment("api")
    .image("docker.io/traefik/whoami:v1.12.0")
    .arg(["--port", "8080", "--name", "paired-provider"])
    .http(8080, api.route("/"));

app.deployment("echo")
    .image("docker.io/alpine/socat:1.8.1.3")
    .arg(["TCP6-LISTEN:7000,fork,reuseaddr", "EXEC:cat"])
    .tcp(7000, echo.port(7000));

// A Unix socket in the exported volume, the way a database leaves its socket
// for clients on the same host. The consumer connects to it through its own
// mount of the volume, without any network path between the two apps.
app.deployment("socket")
    .image("docker.io/alpine/socat:1.8.1.3")
    .arg(["UNIX-LISTEN:/shared/echo.sock,fork,unlink-early", "EXEC:cat"])
    .mount("/shared", shared_volume);

// Rewrites status.txt every 10 seconds, so the consumer's page shows the file
// changing under it. The trap lets the shell, running as PID 1, stop at once.
app.deployment("writer")
    .image("docker.io/library/alpine:3.24.4")
    .command(["/bin/sh", "-c", `
trap 'exit 0' TERM INT
while true; do
  printf '%s\nwritten by paired-provider at %s\n' "$MESSAGE" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > /shared/status.txt.tmp
  mv /shared/status.txt.tmp /shared/status.txt
  sleep 10 &
  wait
done
`])
    .env("MESSAGE", message.value())
    .mount("/shared", shared_volume);
