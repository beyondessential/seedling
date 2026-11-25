// Demo: language-tour.
//
// Calls each standard helper the other demos do not already use, and the
// methods of the values they return, then serves every expression beside its
// result. Everything is computed when the definition is evaluated, so the
// page changes only with the definition. Not a production definition.
//
// Browse https://language-tour.demo.internal/ for the table, or
// /results.json for the same data.

app.description("Demo: the standard helpers and the values they return, evaluated and served as a table");

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("language-tour.demo.internal")
    .description("Hostname the results are served on");

// Each row is an expression as written and the JSON text of its result. A
// Blob has no JSON form, so rows holding one show it as hex.
let row = |expr, value| {
    let shown = if type_of(value) == "blob" {
        `<blob> ${std::encode_hex(value)}`
    } else {
        std::encode_json(value)
    };
    #{ expr: expr, value: shown }
};

// ── Parsing a kind ───────────────────────────────────────────────────────────

let parsing = [
    row.call(`std::parse_text("one line")`, std::parse_text("one line")),
    row.call(`std::parse_multiline("two\nlines")`, std::parse_multiline("two\nlines")),
    row.call(`std::parse_email("ops@demo.internal")`, std::parse_email("ops@demo.internal")),
    row.call(`std::parse_password("correct horse battery staple")`, std::parse_password("correct horse battery staple")),
    row.call(`std::parse_weak_password("1234")`, std::parse_weak_password("1234")),
    row.call(`std::parse_random("0f3c")`, std::parse_random("0f3c")),
    row.call(`std::parse_boolean("true")`, std::parse_boolean("true")),
    row.call(`std::parse_integer("8080", 1..=65535)`, std::parse_integer("8080", 1..=65535)),
    row.call(`std::parse_quantity("1.5Gi")`, std::parse_quantity("1.5Gi")),
    row.call(`std::parse_choice("large", ["small", "large"])`, std::parse_choice("large", ["small", "large"])),
    row.call(`std::parse_hostname("Bücher.Demo.Internal")`, std::parse_hostname("Bücher.Demo.Internal")),
    row.call(`std::parse_volume("scratch")`, std::parse_volume("scratch")),
];

// A parser throws on a value its kind refuses; try/catch turns that into an
// answer.
let refused = "accepted";
try { std::parse_integer("08080"); } catch (e) { refused = `${e}`; }
parsing.push(row.call(`std::parse_integer("08080")`, refused));

// ── Versions ─────────────────────────────────────────────────────────────────

let tag = std::parse_version("16.4-bookworm");
let release = std::parse_version_semver("v2.71.3-rc.1+build.7");
let tags = ["1.10", "1.9", "1.0rc1", "latest", "1.0"].map(|t| std::parse_version(t));
tags.sort(|a, b| if a < b { -1 } else if a > b { 1 } else { 0 });
let versions = [
    row.call(`std::parse_version("16.4-bookworm").segments()`, tag.segments()),
    row.call(`std::parse_version("1.0rc1") < std::parse_version("1.0")`, std::parse_version("1.0rc1") < std::parse_version("1.0")),
    row.call(`std::parse_version("v2.0") == std::parse_version("2.0.0")`, std::parse_version("v2.0") == std::parse_version("2.0.0")),
    row.call(`["1.10", "1.9", "1.0rc1", "latest", "1.0"], sorted`, tags),
    row.call(`release.major(), minor(), patch()`, [release.major(), release.minor(), release.patch()]),
    row.call(`release.pre_release(), build()`, [release.pre_release(), release.build()]),
    row.call(`release.matches("~2.71")`, release.matches("~2.71")),
    row.call(`release.matches(">=2.71.3-rc.0")`, release.matches(">=2.71.3-rc.0")),
    row.call(`text of release`, `${release}`),
];

// ── Package URLs, URLs and hostnames ─────────────────────────────────────────

let purl = std::parse_purl("pkg:npm/%40scope/foo@1.2.0?arch=x64&checksum=SHA256:BB,sha1:aa#lib");
let urls = [
    row.call(`std::parse_purl("pkg:npm/%40scope/foo@1.2.0?arch=x64&checksum=SHA256:BB,sha1:aa#lib")`, purl),
    row.call(`purl.version >= std::parse_version("1.0")`, purl.version >= std::parse_version("1.0")),
    row.call(`std::parse_url("postgres://app:p%40ss@DB.Internal:5432/app?sslmode=require")`, std::parse_url("postgres://app:p%40ss@DB.Internal:5432/app?sslmode=require")),
    row.call(`std::parse_url("https://demo.internal")`, std::parse_url("https://demo.internal")),
    row.call(`std::parse_url_origin("https://Bücher.example:8443/")`, std::parse_url_origin("https://Bücher.example:8443/")),
];

// ── IP addresses, subnets and ranges ─────────────────────────────────────────

let host = std::parse_ip_address("10.0.0.7");
let lan = std::parse_ip_subnet("10.0.0.0/24");
let pool = std::parse_ip_range("10.0.0.250-10.0.1.9");
let v6 = std::parse_ip_address("2001:DB8:0:0::1");
let ips = [
    row.call(`std::parse_ip_address("2001:DB8:0:0::1")`, `${v6}`),
    row.call(`v6.family()`, v6.family()),
    row.call(`host.to_subnet(), to_subnet(16), to_range()`, [`${host.to_subnet()}`, `${host.to_subnet(16)}`, `${host.to_range()}`]),
    row.call(`host < std::parse_ip_address("10.0.0.10")`, host < std::parse_ip_address("10.0.0.10")),
    row.call(`lan.address(), prefix_len(), to_range()`, [`${lan.address()}`, lan.prefix_len(), `${lan.to_range()}`]),
    row.call(`lan.contains(host)`, lan.contains(host)),
    row.call(`lan.overlaps(pool), lan.contains(pool)`, [lan.overlaps(pool), lan.contains(pool)]),
    row.call(`pool.start(), pool.end()`, [`${pool.start()}`, `${pool.end()}`]),
    row.call(`std::parse_ip_range("10.0.0.0/30")`, `${std::parse_ip_range("10.0.0.0/30")}`),
    row.call(`std::parse_ip_subnet("2001:db8::/32", 6).family()`, std::parse_ip_subnet("2001:db8::/32", 6).family()),
];

// ── Durations ────────────────────────────────────────────────────────────────

let d = std::parse_duration("1m 30s");
let durations = [
    row.call(`std::parse_duration("1m 30s").as_seconds()`, d.as_seconds()),
    row.call(`d.as_millis(), d.as_seconds_f()`, [d.as_millis(), std::parse_duration("1500ms").as_seconds_f()]),
    row.call(`std::parse_duration("1500ms").as_millis_f()`, std::parse_duration("1500ms").as_millis_f()),
    row.call(`text of std::parse_duration("90m") and ("2w")`, [`${std::parse_duration("90m")}`, `${std::parse_duration("2w")}`]),
    row.call(`d + d, d - std::parse_duration("30s")`, [`${d + d}`, `${d - std::parse_duration("30s")}`]),
    row.call(`d * 2 == std::parse_duration("3m")`, d * 2 == std::parse_duration("3m")),
    row.call(`d / 4`, `${d / 4}`),
    row.call(`std::parse_duration("1 hour, 30 minutes", #{ max: "2h", precision: "m" })`, `${std::parse_duration("1 hour, 30 minutes", #{ max: "2h", precision: "m" })}`),
];

// ── Regular expressions ──────────────────────────────────────────────────────

let kv = std::regex("(?<key>[a-z_]+)=(?<value>[^;]*)");
let regexes = [
    row.call(`kv.is_match("a=1")`, kv.is_match("a=1")),
    row.call(`kv.find("x; a=1")`, kv.find("x; a=1")),
    row.call(`kv.find_all("a=1;b=2").map(|m| m.text)`, kv.find_all("a=1;b=2").map(|m| m.text)),
    row.call(`kv.captures("a=1").named.value.text`, kv.captures("a=1").named.value.text),
    row.call(`kv.captures_all("a=1;b=2").map(|c| c.groups[1].text)`, kv.captures_all("a=1;b=2").map(|c| c.groups[1].text)),
    row.call(`kv.replace("a=1;b=2", "$value=$key")`, kv.replace("a=1;b=2", "$value=$key")),
    row.call("kv.replace_all(\"a=1;b=2\", \"${value}=${key}\")", kv.replace_all("a=1;b=2", "${value}=${key}")),
    row.call(`kv.replace_all("a=1", |c| c.named.key.text.to_upper())`, kv.replace_all("a=1", |c| c.named.key.text.to_upper())),
    row.call(`std::regex(",\\s*").split("a, b,c")`, std::regex(",\\s*").split("a, b,c")),
    row.call(`std::regex(",\\s*").split("a, b,c", 2)`, std::regex(",\\s*").split("a, b,c", 2)),
    row.call(`std::parse_regex("^v\\d+$").is_match("v12")`, std::parse_regex("^v\\d+$").is_match("v12")),
];

// ── Digests and encodings ────────────────────────────────────────────────────

let encodings = [
    row.call(`std::encode_hex("hi"), std::decode_hex("6869")`, [std::encode_hex("hi"), std::decode_utf8(std::decode_hex("6869"))]),
    row.call(`std::encode_base64("hi?>"), decoded`, [std::encode_base64("hi?>"), std::decode_utf8(std::decode_base64("aGk/Pg=="))]),
    row.call(`std::encode_base64url("hi?>"), decoded`, [std::encode_base64url("hi?>"), std::decode_utf8(std::decode_base64url("aGk_Pg"))]),
    row.call(`std::encode_utf8("ā")`, std::encode_utf8("ā")),
    row.call(`std::encode_percent("p@ss/word"), std::decode_percent("a+b%2F")`, [std::encode_percent("p@ss/word"), std::decode_utf8(std::decode_percent("a+b%2F"))]),
    row.call(`std::digest_sha256("hi")`, std::digest_sha256("hi")),
    row.call(`std::digest_sha512("hi")`, std::digest_sha512("hi")),
    row.call(`std::digest_blake2b("hi")`, std::digest_blake2b("hi")),
    row.call(`std::digest_blake3(app.dir("samples"))`, std::digest_blake3(app.dir("samples"))),
    row.call(`std::hmac_sha256("key", "message")`, std::hmac_sha256("key", "message")),
    row.call(`std::hmac_sha512("key", app.file("samples/app.env"))`, std::hmac_sha512("key", app.file("samples/app.env"))),
    row.call(`std::uuid_v5("6ba7b811-9dad-11d1-80b4-00c04fd430c8", "https://demo.internal/a")`, std::uuid_v5("6ba7b811-9dad-11d1-80b4-00c04fd430c8", "https://demo.internal/a")),
    row.call(`std::quote_shell(["it's", "plain", ""])`, std::quote_shell(["it's", "plain", ""])),
];

// ── Structured formats ───────────────────────────────────────────────────────

let settings = #{ server: #{ port: 8080, hosts: ["a.demo.internal", "b.demo.internal"] }, "debug": false, answer: "no" };
let formats = [
    row.call(`std::encode_json(settings)`, std::encode_json(settings)),
    row.call(`std::encode_yaml(settings)`, std::encode_yaml(settings)),
    row.call(`std::encode_toml(settings)`, std::encode_toml(settings)),
    row.call(`std::encode_env(#{ GREETING: "Kia ora", LEVEL: "info" })`, std::encode_env(#{ GREETING: "Kia ora", LEVEL: "info" })),
    row.call(`std::decode_json("[1, 2.5, null]")`, std::decode_json("[1, 2.5, null]")),
    row.call(`std::decode_yaml(app.file("samples/settings.yaml"))`, std::decode_yaml(app.file("samples/settings.yaml"))),
    row.call(`std::decode_toml(app.file("samples/settings.toml"))`, std::decode_toml(app.file("samples/settings.toml"))),
    row.call(`std::decode_env(app.file("samples/app.env"))`, std::decode_env(app.file("samples/app.env"))),
];

// ── Derived random values ────────────────────────────────────────────────────

// Each value is derived from the app's seed and its label, so it is the same
// on every evaluation and changes when an operator rotates the seed. Anything
// derived from the seed is a secret the runtime withholds wherever it
// describes the app, so the rows show each value's shape and never the value.
let alphanumeric = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
let token = std::random_string("token", 24, alphanumeric);
let uuid = std::uuid_v4("instance-id");
let random = [
    row.call(`std::random_bytes("bytes", 8).len()`, std::random_bytes("bytes", 8).len()),
    row.call(`std::parse_regex("^[A-Za-z0-9]{24}$").is_match(std::random_string("token", 24, alphanumeric))`,
        std::parse_regex("^[A-Za-z0-9]{24}$").is_match(token)),
    row.call(`std::uuid_v4("instance-id").sub_string(14, 1)`, uuid.sub_string(14, 1)),
    row.call(`std::password_argon2id("token-hash", token).split("$").extract(1..4)`,
        std::password_argon2id("token-hash", token).split("$").extract(1..4)),
    row.call(`std::password_bcrypt("token-hash", token).split("$").extract(1..3)`,
        std::password_bcrypt("token-hash", token).split("$").extract(1..3)),
];

let sections = [
    #{ title: "Parsing a kind", rows: parsing },
    #{ title: "Versions", rows: versions },
    #{ title: "Package URLs, URLs and hostnames", rows: urls },
    #{ title: "IP addresses, subnets and ranges", rows: ips },
    #{ title: "Durations", rows: durations },
    #{ title: "Regular expressions", rows: regexes },
    #{ title: "Digests and encodings", rows: encodings },
    #{ title: "Structured formats", rows: formats },
    #{ title: "Derived random values, by shape", rows: random },
];

// ── Serving the results ──────────────────────────────────────────────────────

let escape = |text| std::regex("[&<>]").replace_all(text, |c| switch c.groups[0].text {
    "&" => "&amp;",
    "<" => "&lt;",
    _ => "&gt;",
});

let page = `<!doctype html><meta charset="utf-8"><title>Language tour</title>
<style>body{font-family:sans-serif}td{vertical-align:top;padding:2px 8px}pre{margin:0;white-space:pre-wrap}</style>
<h1>Language tour</h1>
`;
for section in sections {
    page += `<h2>${escape.call(section.title)}</h2><table>\n`;
    for r in section.rows {
        page += `<tr><td><code>${escape.call(r.expr)}</code></td><td><pre>${escape.call(r.value)}</pre></td></tr>\n`;
    }
    page += "</table>\n";
}

let www = app.volume("www")
    .readonly()
    .write("/index.html", page)
    .write("/results.json", std::encode_json(sections));

let web = app.service("web").http(80);
web.ingress(hostname.value(), 443)
    .tls(Terminate.Https, Output.Http1)
    .redirect();

app.deployment("results")
    .image("docker.io/library/busybox:1.38.0-musl")
    // httpd as PID 1 ignores SIGTERM, so a shell runs it and exits on stop.
    .command(["sh", "-c", "trap 'exit 0' TERM INT; httpd -f -p 8080 -h /www & wait"])
    .mount("/www", www)
    .http(8080, web.route("/"));
