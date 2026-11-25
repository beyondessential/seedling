// Demo: params-tour.
//
// One parameter of every kind a parameter can take, with the schema methods
// (kind, kind arguments, required, default_value, description, secret), the
// readers (value, parsed, is_set) and validators, including a pair of
// validators that read each other's parameter through `all`.
//
// A busybox web server renders every parameter into a page served over HTTPS
// on the `hostname` parameter: its kind, whether the operator set it or its
// default is in effect, its value() and its parsed() result. Secret values are
// shown only as set or default. Change a value with
// `seedling-ctl apps params set params-tour name=value` and reload the page:
// the deployment is restarted with the new page. A value its kind or a
// validator refuses, such as a `pool` outside the `subnet`, is refused with
// the reason and nothing changes. Not a production definition.

app.description("Demo: one parameter of every kind, its default, its parsed value and its validators, rendered into a web page");
app.canopy_type("seedling-demo");

// Required, and the only parameter without a default: installing the app
// needs it set first, and value() would throw while it is unset.
let contact = app.param("contact")
    .kind("email")
    .required(true)
    .description("Who to contact about this demo; required, so it has no default");

let greeting = app.param("greeting")
    .kind("text")
    .default_value("Kia ora")
    .description("Heading of the page, a single line of text");

let notes = app.param("notes")
    .kind("multiline")
    .default_value("Every line of this text is kept.\nSet it with a value holding line breaks.")
    .description("Text that may span several lines");

// Implied secret: a password parameter is secret without saying so. An
// operator's value reaches the workload but is withheld wherever the app is
// described. A real definition would not default a password; this one does
// only so that the demo installs with a single required parameter.
let admin_password = app.param("admin-password")
    .kind("password")
    .default_value("Tussock-kea-harbour-48-lantern")
    .description("A strong password, refused if weak; secret because of its kind");

// The implied secret overridden: `secret(false)` here comes before `kind`, and
// an explicit secret call wins in either order. A weak password is never
// refused for its strength.
let kiosk_pin = app.param("kiosk-pin")
    .secret(false)
    .kind("weak-password")
    .default_value("1234")
    .description("A password of any strength, deliberately not secret, so it is shown");

// `random` does not imply secret, so a parameter holding key material says so.
let session_key = app.param("session-key")
    .kind("random")
    .secret(true)
    .default_value("6f1c0a4e9b7d2c5a3e8f0b1d4c7a9e2f5b8d1c4a7e0f3b6d9c2a5e8f1b4d7c0a")
    .description("Random key material; secret only because the definition says so");

// A boolean default given as a boolean is stored as its text, "true".
let redirect_http = app.param("redirect-http")
    .kind("boolean")
    .default_value(true)
    .description("Whether plain HTTP on port 80 redirects to HTTPS");

// An integer default given as an integer, within the kind's range.
let replicas = app.param("replicas")
    .kind(#{ name: "integer", range: 1..=3 })
    .default_value(1)
    .description("How many copies of the web server run, from 1 to 3");

let memory = app.param("memory")
    .kind(#{ name: "quantity", range: 16 * 1024 * 1024..=256 * 1024 * 1024 })
    .default_value("64Mi")
    .description("Memory limit of each web server, in bytes, from 16Mi to 256Mi");

let on_update = app.param("on-update")
    .kind(#{ name: "choice", options: ["rolling", "replace"] })
    .default_value("rolling")
    .description("How the web server is replaced when the page changes");

let upstream_version = app.param("upstream-version")
    .kind("version")
    .default_value("16.4-bookworm")
    .description("A version of any scheme, such as an image tag");

let release = app.param("release")
    .kind("version-semver")
    .default_value("v1.4.2")
    .description("A Semantic Versioning 2.0.0 version");

let dependency = app.param("dependency")
    .kind("purl")
    .default_value("pkg:npm/%40demo/widget@1.2.0?arch=x64#lib")
    .description("A Package URL");

let upstream = app.param("upstream")
    .kind("url")
    .default_value("https://API.demo.internal:8443/v1?debug=1")
    .description("A URL of any scheme; its host is lower-cased when parsed");

// A single-parameter validator: it receives the parsed value, and throwing
// rejects it with the thrown text as the reason.
let origin = app.param("origin")
    .kind("url-origin")
    .default_value("https://params-tour.demo.internal")
    .description("An http or https origin; this demo accepts https only")
    .validate(|v, all| {
        if v.scheme != "https" {
            throw "the origin must use https";
        }
    });

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("params-tour.demo.internal")
    .description("Hostname the page is served on");

let dns_server = app.param("dns-server")
    .kind("ip-address")
    .default_value("2001:DB8::53")
    .description("An IPv4 or IPv6 address, printed in canonical form when parsed");

// `subnet` and `pool` are checked against each other. A validator runs only
// while its own parameter is set, and `all` holds only the parameters that
// would be set, so each side of the rule is checked from both parameters and
// falls back to the other's default when the operator has not set it. A
// captured Param is not used for this: when the proposed values fail to
// evaluate, the validators of the last good evaluation run, and the Params it
// captured hold the old values.
let subnet_default = "10.20.0.0/24";
let pool_default = "10.20.0.100-10.20.0.199";
let pool_within = |pool, subnet| {
    if !subnet.contains(pool) {
        throw `the pool ${pool} lies outside the subnet ${subnet}`;
    }
};

let subnet = app.param("subnet")
    .kind("ip-subnet")
    .default_value(subnet_default)
    .description("A subnet of either family that must contain the pool")
    .validate(|v, all| {
        let pool = if "pool" in all { all.pool } else { std::parse_ip_range(pool_default, 4) };
        pool_within.call(pool, v);
    });

let pool = app.param("pool")
    .kind(#{ name: "ip-range", family: 4 })
    .default_value(pool_default)
    .description("An IPv4 range, which must lie within the subnet")
    .validate(|v, all| {
        let subnet = if "subnet" in all { all.subnet } else { std::parse_ip_subnet(subnet_default) };
        pool_within.call(v, subnet);
    });

// A Duration default; `precision: "s"` refuses a fraction of a second when
// it is entered, since stop_timeout takes whole seconds.
let grace = app.param("grace")
    .kind(#{ name: "duration", min: "1s", max: "2m", precision: "s" })
    .default_value(std::parse_duration("10s"))
    .description("How long the web server has to stop, from 1s to 2m in whole seconds");

let path_filter = app.param("path-filter")
    .kind("regex")
    .default_value("^/(status|health)$")
    .description("A regular expression; the page tests it against two paths");

fn escape_html(text) {
    text.replace("&", "&amp;");
    text.replace("<", "&lt;");
    text.replace(">", "&gt;");
    text
}

fn source(p) {
    if p.is_set() { "set" } else { "default" }
}

fn shown(name, p) {
    `<tr><th>${name}</th><td>${source(p)}</td><td><pre>${escape_html(p.value())}</pre></td><td><pre>${escape_html(p.parsed().to_string())}</pre></td></tr>`
}

fn withheld(name, p) {
    `<tr><th>${name}</th><td>${source(p)}</td><td>withheld</td><td>withheld</td></tr>`
}

let address = contact.parsed();
let semver = release.parsed();
let purl = dependency.parsed();
let url = upstream.parsed();
let pattern = path_filter.parsed();

let rows = [
    shown("contact (email)", contact),
    shown("greeting (text)", greeting),
    shown("notes (multiline)", notes),
    withheld("admin-password (password)", admin_password),
    shown("kiosk-pin (weak-password, secret(false))", kiosk_pin),
    withheld("session-key (random, secret(true))", session_key),
    shown("redirect-http (boolean)", redirect_http),
    shown("replicas (integer)", replicas),
    shown("memory (quantity)", memory),
    shown("on-update (choice)", on_update),
    shown("upstream-version (version)", upstream_version),
    shown("release (version-semver)", release),
    shown("dependency (purl)", dependency),
    shown("upstream (url)", upstream),
    shown("origin (url-origin)", origin),
    shown("hostname (hostname)", hostname),
    shown("dns-server (ip-address)", dns_server),
    shown("subnet (ip-subnet)", subnet),
    shown("pool (ip-range, family 4)", pool),
    shown("grace (duration)", grace),
    shown("path-filter (regex)", path_filter),
];

// What a parsed() result offers beyond its text.
let uses = [
    `contact: local part ${address.local}, domain ${address.domain}`,
    `memory: ${memory.parsed()} bytes`,
    `upstream-version: segments ${upstream_version.parsed().segments()}`,
    `release: major ${semver.major()}, minor ${semver.minor()}, patch ${semver.patch()}, matches ^1: ${semver.matches("^1")}`,
    `dependency: type ${purl.type}, name ${purl.name}`,
    `upstream: host ${url.host}, port ${url.port}, path ${url.path}`,
    `dns-server: IPv${dns_server.parsed().family()}`,
    `pool: ${pool.parsed().start()} to ${pool.parsed().end()}, within subnet: ${subnet.parsed().contains(pool.parsed())}`,
    `grace: ${grace.parsed().as_seconds()} seconds`,
    `path-filter: matches /status: ${pattern.is_match("/status")}, matches /admin: ${pattern.is_match("/admin")}`,
];

let page = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>params-tour</title></head><body>
<h1>${escape_html(greeting.value())}</h1>
<p>Every parameter of the params-tour demo. "set" means the operator stored a value; "default" means the definition's default is in effect.</p>
<table border="1">
<tr><th>parameter (kind)</th><th>source</th><th>value()</th><th>parsed()</th></tr>
${rows.reduce(|all, row| all + row + "\n", "")}</table>
<h2>Using parsed values</h2>
<ul>
${uses.reduce(|all, line| all + "<li>" + escape_html(line) + "</li>\n", "")}</ul>
</body></html>
`;

let web = app.service("web").http(80);

let ingress = web.ingress(hostname.parsed(), 443)
    .tls(Terminate.Https, Output.Http1);
if redirect_http.parsed() {
    ingress.redirect();
}

app.deployment("page")
    .image("docker.io/library/busybox:1.38.0-musl")
    .user("65534:65534")
    // httpd as PID 1 ignores SIGTERM, so a shell runs it and exits on stop.
    .command(["sh", "-c", "mkdir -p /tmp/www && printf '%s' \"$PAGE\" > /tmp/www/index.html; trap 'exit 0' TERM INT; httpd -f -p 8080 -h /tmp/www & wait"])
    .env("PAGE", page)
    // Secret values reach the workload; the server ignores them.
    .env("ADMIN_PASSWORD", admin_password.value())
    .env("SESSION_KEY", session_key.value())
    .scale(replicas.parsed())
    .memory(memory.parsed())
    .stop_timeout(grace.parsed())
    .on_update(if on_update.parsed() == "replace" { OnUpdate.Replace } else { OnUpdate.Rolling })
    .http(8080, web.route("/"));
