// Demo: hello world.
//
// The smallest app that serves something: one Deployment running a
// request-echo server, one HTTP service, and one HTTPS ingress on a hostname
// param. Not a production definition.

app.description("Demo: the smallest Seedling app, an echo server behind one HTTPS ingress");
app.canopy_type("seedling-demo");

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("hello-world.demo.internal")
    .description("Hostname the echo server answers on");

let web = app.service("web").http(80);

web.ingress(hostname.value(), 443)
    .tls(Terminate.Https, Output.Http1)
    .redirect();

app.deployment("echo")
    .image("docker.io/traefik/whoami:v1.12.0")
    .arg(["--port", "8080", "--name", "hello-world"])
    .http(8080, web.route("/"));
