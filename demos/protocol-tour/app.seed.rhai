// Demo: protocol-tour.
//
// One ingress for each way an ingress can take traffic in and hand it on:
// TCP passed through untouched, TLS terminated to TCP, DTLS terminated to
// UDP, and HTTPS terminated to HTTP/1.1 and to cleartext HTTP/2. Behind them
// sit a TCP and a UDP echo server and a request-echo server. It also shows
// how ingresses are identified, which of them can share a port, the HTTPS
// redirect, and a service left without an ingress of its own for an operator
// to attach a site ingress to. Not a production definition.
//
// Every hostname is a param, so two installs can share hostnames or differ.
// Installing the demo a second time under another app name with the same
// hostnames leaves the second install's ingresses out of the proxy, with an
// `ingress_conflict` fault against it:
//   seedling-ctl apps create protocol-tour demos/protocol-tour
//   seedling-ctl apps install protocol-tour
//   seedling-ctl apps create protocol-tour-two demos/protocol-tour
//   seedling-ctl apps install protocol-tour-two
//   seedling-ctl faults --app protocol-tour-two
// Giving the second install different hostnames clears every conflict but
// the TCP and DTLS ones: those ingresses hold their whole port, whatever
// hostname they name.
//
// The hostnames must resolve to the host, for example through /etc/hosts.
// They are `.internal` names, so the proxy's internal CA issues every
// certificate here without any operator setup; clients that do not trust
// that CA need `-k` (curl) or must skip verification (openssl), and
//   seedling-ctl tls hostnames --app protocol-tour
// shows each hostname's certificate. A hostname is held in lower case, so
// TLS.protocol-tour.demo.internal names the same ingress as the default.

app.description("Demo: one ingress per termination, shared ports, the HTTPS redirect, and a site ingress target");
app.canopy_type("seedling-demo");

let tcp_hostname = app.param("tcp-hostname")
    .kind("hostname")
    .default_value("tcp.protocol-tour.demo.internal")
    .description("Hostname of the TCP pass-through ingress");

let tls_hostname = app.param("tls-hostname")
    .kind("hostname")
    .default_value("tls.protocol-tour.demo.internal")
    .description("Hostname of the TLS-terminating ingress");

let dtls_hostname = app.param("dtls-hostname")
    .kind("hostname")
    .default_value("dtls.protocol-tour.demo.internal")
    .description("Hostname of the DTLS-terminating ingress");

let https_hostname = app.param("https-hostname")
    .kind("hostname")
    .default_value("https.protocol-tour.demo.internal")
    .description("Hostname of the HTTPS ingresses that hand on HTTP/1.1");

let h2_hostname = app.param("h2-hostname")
    .kind("hostname")
    .default_value("h2.protocol-tour.demo.internal")
    .description("Hostname of the HTTPS ingress that hands on cleartext HTTP/2");

let whoami = "docker.io/traefik/whoami:v1.12.0";
let socat = "docker.io/alpine/socat:1.8.1.3";

// The TCP echo service. A pass-through or TLS-terminating ingress delivers to
// the service port with its own number, so each ingress below has a port of
// its own here, and the echo server binds both.
let tcp_echo = app.service("tcp-echo");

// TCP passed through: no tls() call, so the proxy forwards the connection as
// it arrives and never looks inside it. It reads no hostname from the
// traffic, so this ingress holds port 7 alone and the hostname only names
// it. Only TCP is forwarded, never UDP.
//   nc tcp.protocol-tour.demo.internal 7
tcp_echo.ingress(tcp_hostname.value(), 7)
    .description("TCP passed through to the echo server");

// TLS terminated, plaintext TCP handed on. The proxy finds this ingress by
// the name the client sends (SNI), which is what lets it share 443 with the
// HTTPS ingresses below; a handshake with no SNI, or one naming no ingress
// on the port, is refused.
//   openssl s_client -quiet -connect tls.protocol-tour.demo.internal:443 -servername tls.protocol-tour.demo.internal
tcp_echo.ingress(tls_hostname.value(), 443)
    .tls(Terminate.Tls, Output.Tcp)
    .description("TLS terminated, plaintext TCP to the echo server");

// DTLS terminated, plaintext datagrams handed on, each session on a UDP flow
// of its own so the echo server tells clients apart by source port. A DTLS
// ingress holds its UDP port alone and presents its own hostname's
// certificate whatever SNI the client sends, so on 443 it would take the UDP
// port the HTTPS ingresses need for HTTP/3; it sits on 5684 instead.
// Datagrams that are not DTLS are dropped without a reply.
//   openssl s_client -dtls -quiet -connect dtls.protocol-tour.demo.internal:5684
let udp_echo = app.service("udp-echo");
udp_echo.ingress(dtls_hostname.value(), 5684)
    .tls(Terminate.Dtls, Output.Udp)
    .description("DTLS terminated, plaintext UDP to the echo server");

// HTTPS terminated. Clients may speak HTTP/1.1 or HTTP/2 over TLS, or HTTP/3
// over UDP on the same port; the request-echo server prints the protocol it
// was handed on its request line, HTTP/1.1 or HTTP/2.0, whatever the client
// spoke.
//   curl -k https://https.protocol-tour.demo.internal/
//   curl -k --http3-only https://https.protocol-tour.demo.internal/
//   curl -k https://h2.protocol-tour.demo.internal/
let web = app.service("web").http(80);

// .redirect() answers plaintext HTTP on port 80 with a 307 to this ingress,
// except ACME challenges the proxy answers itself.
//   curl -i http://https.protocol-tour.demo.internal/path?q   # Location: https://https.protocol-tour.demo.internal/path?q
web.ingress(https_hostname.value(), 443)
    .tls(Terminate.Https, Output.Http1)
    .redirect()
    .description("HTTPS terminated, HTTP/1.1 to the request-echo server");

// An ingress is named by its hostname and port, here
// https.protocol-tour.demo.internal:8443, so the same hostname on another
// port is a second ingress, served on its own. Its redirect listens on 8080
// with 308, and its Location carries :8443, the ingress's own port.
//   curl -i http://https.protocol-tour.demo.internal:8080/   # 308 to https://https.protocol-tour.demo.internal:8443/
// The order of tls() and redirect() does not matter.
web.ingress(https_hostname.value(), 8443)
    .redirect(8080, 308)
    .tls(Terminate.Https, Output.Http1)
    .description("The HTTP/1.1 ingress again, on its own port");

// Declaring the same hostname and port twice throws and leaves the first
// ingress as it was. Hostnames are compared in lower case, so the upper-case
// spelling is the same ingress. A definition that did not catch the error
// would fail to evaluate.
let duplicate_refused = false;
try {
    web.ingress(https_hostname.value().to_upper(), 443);
} catch {
    duplicate_refused = true;
}
if !duplicate_refused {
    throw "declaring an ingress whose hostname and port are taken should have thrown";
}

// HTTPS terminated, cleartext HTTP/2 handed on, from the same service as the
// HTTP/1.1 ingresses. Three TLS-terminating ingresses now share TCP port
// 443, told apart by SNI, and two plaintext redirects share port 80, told
// apart by Host. Two ingresses of one app making claims that conflict (two
// on one hostname and port, or a pass-through ingress on a TLS port) would
// fail the definition instead.
web.ingress(h2_hostname.value(), 443)
    .tls(Terminate.Https, Output.Http2)
    .redirect(80, 301)
    .description("HTTPS terminated, cleartext HTTP/2 to the request-echo server");

// A service with no ingress of its own, exported so operators find it when
// they list exported services. An operator gives it a hostname by attaching a
// site ingress to it:
//   seedling-ctl ingresses site create tour-site --hostname site.protocol-tour.demo.internal --tls internal
//   seedling-ctl ingresses site attach tour-site --port 443 --protocol http --to protocol-tour/site-target
//   seedling-ctl ingresses site attach tour-site --port 7007 --protocol tcp --to protocol-tour/site-target
//   seedling-ctl ingresses site attach tour-site --port 7007 --protocol udp --to protocol-tour/site-target
// An `http` or `http2` attachment reaches the request-echo server through the
// service's routes. A `tcp` or `udp` attachment reaches the service port of
// the first binding of its transport, looking through the deployments by
// name: port 7, served by the TCP echo server for `tcp` and by the UDP echo
// server for `udp`. A site ingress on the same hostname and port as one of
// this app's ingresses conflicts with it, and both are left out.
let site_target = app.service("site-target")
    .exported(#{ description: "Attach a site ingress here: HTTP to the request-echo server, TCP and UDP to the echo servers" });

// Pods hold only IPv6 addresses, so the echo servers listen on IPv6. Each
// connection or datagram peer gets a process of its own, which exits after
// ten minutes idle.
app.deployment("socat-tcp")
    .image(socat)
    .arg(["-T", "600", "TCP6-LISTEN:9000,fork,reuseaddr", "PIPE"])
    .tcp(9000, tcp_echo.port(7))
    .tcp(9000, tcp_echo.port(443))
    .tcp(9000, site_target.port(7));

app.deployment("socat-udp")
    .image(socat)
    .arg(["-T", "600", "UDP6-LISTEN:9001,fork,reuseaddr", "PIPE"])
    .udp(9001, udp_echo.port(5684))
    .udp(9001, site_target.port(7));

app.deployment("whoami")
    .image(whoami)
    .arg(["--port", "8080", "--name", "protocol-tour"])
    .http(8080, web)
    .http(8080, site_target.http(80));

// The runtime obtains a certificate for every TLS-terminating ingress on its
// own. This action starts obtaining them and waits until each is served, as
// a step that must not go ahead without them would; the pass-through ingress
// needs none and is skipped.
//   seedling-ctl apps action protocol-tour warm-certs
app.on_action("warm-certs", |rt, _params| {
    rt.warm_certs(app).ready("5m");
}, #{
    description: "Obtain the certificates of the TLS-terminating ingresses and wait until each is served",
});
