// Demo: http-tour.
//
// Every proxy setting an HTTP service can declare, each on a route of its
// own, in front of a request-echo server whose replies show what the proxy
// did: routes and prefix normalisation, route redirects in every form,
// compression, balancing across replicas, rate limiting, header operations,
// and how a route's settings resolve against its service's. A second, smaller
// service shows the fallback route a service is served through when no pod
// binds any of its prefixes. Not a production definition.
//
// The echo server prints the request it received (method, path, protocol and
// every header) along with the Hostname and IP of the replica that answered.
// The hostnames must resolve to the host, for example through /etc/hosts, or
// pass `--resolve <name>:443:<host address>` to curl. Both are `.internal`
// names, so the proxy's internal CA issues their certificates; pass `-k` to
// curl unless the client trusts that CA.
//
// Try on a dev VM:
//   seedling-ctl apps create http-tour demos/http-tour
//   seedling-ctl apps install http-tour
//   seedling-ctl apps show http-tour   # the resolved settings of every route
//   curl -k https://http-tour.demo.internal/
//   curl -i http://http-tour.demo.internal/   # 307 to https, from port 80

app.description("Demo: every HTTP service proxy setting, one route each, in front of a request-echo server");
app.canopy_type("seedling-demo");

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("http-tour.demo.internal")
    .description("Hostname the tour's routes answer on");

let fallback_hostname = app.param("fallback-hostname")
    .kind("hostname")
    .default_value("fallback.http-tour.demo.internal")
    .description("Hostname of the service served through its fallback route");

let whoami = "docker.io/traefik/whoami:v1.12.0";

// Service-level settings. Every route of `web` takes these unless it declares
// its own, and each kind of setting resolves on its own, so a route that
// overrides compression still inherits the service's balancing, rate limit
// and headers. Balancing belongs to the Service itself; `web.balance(...)` on
// the HTTP view below would set the same service-wide value.
let web = app.service("web")
    .balance(#{ policy: "round_robin", try_duration: "2s", interval: "100ms" })
    .http(80)
    // Only gzip is offered service-wide; routes below add zstd or narrow the
    // types. Responses under 256 bytes go uncompressed.
    .compress(#{ encodings: ["gzip"], minimum_length: 256 })
    // One budget of 120 requests a minute per client, shared by every route
    // that inherits it rather than declaring a limit of its own.
    .rate_limit(#{ max_events: 120, window: "1m" })
    // Every request reaches the echo server carrying X-Tour, and every
    // response, including a redirect or a 429 the proxy makes itself,
    // carries X-Tour-Served-By.
    .headers(#{
        request: #{ add: #{ "X-Tour": "http-tour" } },
        response: #{ replace: #{ "X-Tour-Served-By": "seedling" } },
    });

web.ingress(hostname.value(), 443)
    .tls(Terminate.Https, Output.Http1)
    .redirect();

// Prefixes are normalised by dropping trailing `/`, so "/headers/" and
// "/headers" below name one route, and "//" would name "/". The path reaches
// the pod unchanged: GET /headers/x arrives at the echo server as /headers/x.
let root = web.route("/");

// Compression. The echo server's /data?size=N returns N bytes of text/plain.
//   curl -k -sD - -o /dev/null -H 'Accept-Encoding: gzip' 'https://http-tour.demo.internal/data?size=4096'
// shows Content-Encoding: gzip; with size=512 the response is under this
// route's threshold and goes uncompressed. The route names only
// minimum_length, so it keeps the service's encodings.
let data = web.route("/data")
    .compress(#{ minimum_length: 1024 });

// The echo server's /api answers in JSON. This route offers zstd ahead of
// gzip and compresses only JSON, and keeps the service's 256-byte threshold.
//   curl -k -sD - -o /dev/null -H 'Accept-Encoding: zstd, gzip' https://http-tour.demo.internal/api
let api = web.route("/api")
    .compress(#{ encodings: ["zstd", "gzip"], content_types: ["application/json"] });

// compress(false) turns compression off for this route alone; compare the
// headers of /raw with those of / for the same Accept-Encoding.
let raw = web.route("/raw").compress(false);

// Balancing across the three replicas of `echo`. Repeat a request and watch
// the Hostname line: / goes round the replicas in turn (the service's
// policy), /first always reaches the same one, /random picks at random.
// /least-conn picks the replica with the fewest requests in flight, which
// shows with slow requests run side by side, such as
//   curl -k 'https://http-tour.demo.internal/least-conn?wait=5s' &
// Each route names only its policy and keeps the service's try_duration and
// interval. /no-retry gives up at once when the chosen replica cannot be
// reached; a zero interval is accepted only alongside a zero try_duration.
let first = web.route("/first").balance(#{ policy: "first" });
let random = web.route("/random").balance(#{ policy: "random" });
let least_conn = web.route("/least-conn").balance(#{ policy: "least_conn" });
let no_retry = web.route("/no-retry").balance(#{ try_duration: "0s", interval: "0s" });

// Rate limiting. /limited has a budget of its own, 5 requests in any 10
// seconds per client (one /64 for IPv6), and answers past it with 429 and
// Retry-After:
//   for i in $(seq 7); do curl -k -s -o /dev/null -w '%{http_code}\n' https://http-tour.demo.internal/limited; done
// /unlimited opts out of the service's limit altogether.
let limited = web.route("/limited").rate_limit(#{ max_events: 5, window: "10s" });
let unlimited = web.route("/unlimited").rate_limit(false);

// Header operations, every kind in both directions:
//   curl -k -i https://http-tour.demo.internal/headers
// The echo shows Host and User-Agent replaced, Accept removed, two
// X-Tour-Note lines, and X-Tour replaced. The response carries two Set-Cookie
// lines, Cache-Control from the second headers() call, which layers over the
// first by header name, and no X-Tour-Served-By. A route's operation wins
// for each header it names, so the service's X-Tour and X-Tour-Served-By
// operations do not apply here, while any header the route leaves alone
// would still take the service's.
let headers = web.route("/headers/")
    .headers(#{
        request: #{
            replace: #{
                "Host": "backend.http-tour.demo.internal",
                "User-Agent": "seedling-http-tour",
                "X-Tour": "replaced by the /headers route",
            },
            add: #{ "X-Tour-Note": ["first", "second"] },
            remove: ["Accept"],
        },
        response: #{
            replace: #{ "Cache-Control": "no-store" },
            add: #{ "Set-Cookie": ["tour=headers; Path=/headers", "step=1; Path=/headers"] },
            remove: ["X-Tour-Served-By"],
        },
    })
    .headers(#{ response: #{ replace: #{ "Cache-Control": "no-cache" } } });

// Route redirects. A redirect reaches no pod and is never bound by one.
// The positional forms keep the path after the prefix and the query:
//   curl -k -i 'https://http-tour.demo.internal/old/a/b?c=d'   # 307 to /moved/a/b?c=d
//   curl -k -i 'https://http-tour.demo.internal/retired/a'     # 308 to /moved/a
// A redirect route claims whole path segments, so /older is not redirected
// and reaches the echo server through /, whereas a proxied route such as
// /data also claims /database. The redirect carries the service's response
// header and this route's own; the service's request header is ignored here.
web.route("/old")
    .redirect("/moved")
    .headers(#{ response: #{ add: #{ "X-Tour-Redirect": "positional" } } });
web.route("/retired").redirect("/moved", 308);

// The map form writes the target out in full, naming the parts of the
// request it keeps with <tail> and <query>; a target with neither is
// literal, so every request under /exact goes to exactly /moved.
web.route("/exact").redirect(#{ to: "/moved" });
web.route("/tail-only").redirect(#{ to: "/moved<tail>", code: 302 });
web.route("/query-only").redirect(#{ to: "/moved/search<query>", code: 301 });

// Absolute targets leave the hostname: /elsewhere moves to the fallback
// service's hostname, keeping path and query, and /example to a fixed page.
web.route("/elsewhere").redirect(`https://${fallback_hostname.value()}`, 302);
web.route("/example").redirect(#{ to: "http://example.org/", code: 307 });

// Three replicas, so balancing has a choice to make. Each binding serves one
// route from the same pod port; /moved and anything else unmatched falls to /.
app.deployment("echo")
    .image(whoami)
    .arg(["--port", "8080", "--name", "http-tour"])
    .scale(3)
    .http(8080, root)
    .http(8080, data)
    .http(8080, api)
    .http(8080, raw)
    .http(8080, first)
    .http(8080, random)
    .http(8080, least_conn)
    .http(8080, no_retry)
    .http(8080, limited)
    .http(8080, unlimited)
    .http(8080, web.route("/headers"));

// The fallback route. No pod binds a prefix of `plain`: its pod binds the
// Service Port directly, so the proxy serves the whole service through one
// route at / whose upstream is the service's own address. That route takes
// the settings declared on route("/"). Routes are shared by every HTTP view
// of a service, so route("/") on the port 8080 view is the same route; the
// fallback route's upstream is the lowest port the service is viewed on, 80.
//   curl -k -i https://fallback.http-tour.demo.internal/anything
// shows Name: fallback and the X-Tour-Fallback header. A redirect route
// leaves the fallback route in place:
//   curl -k -i https://fallback.http-tour.demo.internal/home   # 301 to the tour
let plain = app.service("plain");
let plain_http = plain.http(80);
plain.http(8080).route("/")
    .compress(false)
    .headers(#{ response: #{ add: #{ "X-Tour-Fallback": "served through the fallback route" } } });
plain_http.route("/home").redirect(`https://${hostname.value()}`, 301);

// Port 80 here redirects with 308 rather than the default 307.
plain_http.ingress(fallback_hostname.value(), 443)
    .tls(Terminate.Https, Output.Http1)
    .redirect(80, 308);

app.deployment("fallback-echo")
    .image(whoami)
    .arg(["--port", "8080", "--name", "fallback"])
    .tcp(8080, plain.port(80));
