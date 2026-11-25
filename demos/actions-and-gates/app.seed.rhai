// Demo: actions-and-gates.
//
// A small web page served from a "store" volume whose data format is kept
// current by gates, with operator actions and shells that drive every method
// of the runtime handle. The store is a text file and a format number, so
// nothing here stands for real data. Not a production definition.
//
// Gates, in declaration order:
//   - `migrate` initialises the store on its first run and steps it up one
//     format at a time on later runs. It refuses to step down. The web
//     deployment is held stopped while it runs, and a check job confirms the
//     store's format every ten minutes.
//   - `render` writes the greeting into the page whenever it changes.
//   - `verify` has a check and no body: it waits on a condition alone.
//   - `announce` runs after the web deployment is up, and writes into it.
//
// Try on a dev VM:
//   seedling-ctl apps params set actions-and-gates format=3   # migrate reruns
//   seedling-ctl apps params set actions-and-gates format=1   # migrate refuses
//   seedling-ctl apps params set actions-and-gates greeting=Hello  # render reruns
//   seedling-ctl apps gate run actions-and-gates verify
//   seedling-ctl apps action invoke actions-and-gates note --param text=hi
//   seedling-ctl apps shell actions-and-gates store

app.description("Demo: gates keeping a store's format current, and actions and shells driving the runtime handle");

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("actions-and-gates.demo.internal")
    .description("Hostname the page is served on");

let format = app.param("format")
    .kind(#{ name: "integer", range: 1..=3 })
    .default_value(2)
    .description("Data format the store is migrated to");

let greeting = app.param("greeting")
    .default_value("Kia ora")
    .description("Greeting rendered into the page");

let tools = "docker.io/library/alpine:3.24.4";
let initialiser = "docker.io/library/busybox:1.38.0-musl";

// ── Resources ────────────────────────────────────────────────────────────────

let store = app.volume("store");
let site = app.volume("site");

let web_svc = app.service("web").http(80);
web_svc.ingress(hostname.value(), 443)
    .tls(Terminate.Https, Output.Http1)
    .redirect();

let web = app.deployment("web")
    .image(tools)
    .command(["httpd", "-f", "-p", "8080", "-h", "/www"])
    .mount("/www", site)
    .mount("/store", store)
    .http(8080, web_svc.route("/"));

// A check job proves a gate still holds. It needs a deadline, and belongs to
// one gate only.
let store_check = app.job("store-check")
    .image(tools)
    .command(["sh", "-c", `test "$(cat /store/FORMAT)" = "${format.value()}"`])
    .mount("/store", store)
    .deadline("30s");

let verify_check = app.job("verify-check")
    .image(tools)
    .command(["sh", "-c", "test -s /store/data.txt && test -s /www/index.html"])
    .mount("/store", store)
    .mount("/www", site)
    .deadline("30s");

// Runs a shell command over the store in a fresh anonymous job, and waits for
// it to succeed. A plain closure for shared steps, called with `.call`.
let in_store = |rt, image, script| {
    rt.start(app.job()
        .image(image)
        .command(["sh", "-c", script])
        .mount("/store", store)
    ).terminated().ensure_success();
};

// ── Gates ────────────────────────────────────────────────────────────────────

// The key moves with `format` alone, so changing the greeting never reruns
// the migration. `last` is the app at the gate's last success, or `()` before
// the first one.
let migrate = app.gate("migrate")
    .key(format.parsed())
    .exclusive(web)
    .check(store_check, #{ interval: "10m" })
    .run(|rt, last| {
        let to = format.parsed();
        if last == () {
            in_store.call(rt, initialiser,
                `echo "first entry" > /store/data.txt && echo ${to} > /store/FORMAT`);
            return;
        }
        let from = last.param("format").parsed();
        if to < from {
            throw `refusing to step the store down from format ${from} to ${to}`;
        }
        for step in from..to {
            in_store.call(rt, tools,
                `sed -i 's/^/[v${step + 1}] /' /store/data.txt && echo ${step + 1} > /store/FORMAT`);
        }
    });

let render = app.gate("render")
    .key(greeting.value())
    .after(migrate)
    .run(|rt| {
        rt.write(site, "/index.html", `<h1>${greeting.value()}</h1>\n`);
    });

// No body: the gate is satisfied once its check passes.
let verify = app.gate("verify")
    .after([migrate, render])
    .check(verify_check);

web.after([render, verify]);

// Ordered after a Deployment: runs once the web deployment has a Ready
// instance, and execs into it.
app.gate("announce")
    .after(web)
    .run(|rt| {
        rt.exec(web, ["sh", "-c", "date > /www/up-since.txt"]).ensure_success();
    });

// ── Actions ──────────────────────────────────────────────────────────────────

// A param schema: every key is validated before the closure runs, and the
// closure gets each kind's parsed value.
let note = app.on_action("note", |rt, p| {
    let line = if p.loud { p.text.to_upper() } else { p.text };
    let copies = p.copies;
    in_store.call(rt, tools,
        `for i in $(seq ${copies}); do echo ${std::quote_shell(line)} >> /store/data.txt; done`);
}, #{
    description: "Append a note to the store",
    params: #{
        text: #{ kind: "text", description: "Text to append" },
        loud: #{ kind: "boolean", default_value: false, description: "Upper-case it" },
        copies: #{ kind: #{ name: "integer", range: 1..=5 }, default_value: 1, description: "How many times" },
    },
});

// One action invoking another, by captured handle and by name.
app.on_action("note-twice", |rt, p| {
    note.invoke(#{ text: p.text });
    app.action("note").invoke(#{ text: p.text, loud: true });
}, #{
    description: "Append a note twice, the second time loud",
    params: #{ text: #{ description: "Text to append" } },
});

// A scheduled action. `H` picks a minute fixed for this app and action, and
// the schedule runs with each param's default.
app.on_action("tidy", |rt, p| {
    in_store.call(rt, tools, `tail -n ${p.keep} /store/data.txt > /tmp/d && cat /tmp/d > /store/data.txt`);
}, #{
    description: "Keep only the most recent lines of the store",
    params: #{ keep: #{ kind: #{ name: "integer", range: 1.. }, default_value: 100 } },
})
    .on_schedule("H 3 * * *")
    .on_schedule("*/15 9-17 * * SAT,SUN Pacific/Auckland");

// rt.stop, rt.start and the barriers on what rt.start returns.
app.on_action("bounce", |rt, p| {
    rt.stop(web, p.grace);
    let started = rt.start(web);
    started.scheduled();
    started.running("1m");
    started.ready(p.grace);
}, #{
    description: "Stop the web deployment, then start it and wait for it to be ready",
    params: #{
        grace: #{ kind: #{ name: "duration", min: "5s", max: "10m", precision: "s" }, default_value: "1m" },
    },
});

// rt.restart follows the deployment's update strategy; rt.signal reaches
// PID 1 of every running instance; rt.query waits without starting anything.
app.on_action("restart", |rt, _p| { rt.restart(web); },
    #{ description: "Restart the web deployment" });
app.on_action("reload", |rt, _p| { rt.signal(web, "HUP"); },
    #{ description: "Send SIGHUP to the web server" });
app.on_action("wait-ready", |rt, _p| { rt.query(web).ready_eventually(); },
    #{ description: "Wait, without a deadline, until the web deployment is ready" });

// Collections pick resources by type, name and name pattern, and a Started
// offers the same methods over the instances it holds.
app.on_action("wait-serving", |rt, _p| {
    let workloads = col(app).select(#{ types: [ResourceType.Deployment, ResourceType.Job] });
    let checks = workloads.select(#{ name_patterns: ["*-check"] });
    let serving = workloads.except(checks).only(app.select(#{ names: ["web"] }));
    if serving.one() == () { throw "nothing is serving"; }
    let started = rt.query([serving, col(web_svc)]);
    started.select(#{ types: [ResourceType.Deployment] }).ready("2m");
}, #{ description: "Wait for the serving workloads, leaving out the check jobs" });

// rt.exec runs commands in one running container, here an anonymous job kept
// alive with IDLE_CMD. Its output goes to the container's logs.
app.on_action("count", |rt, _p| {
    let host = rt.start(app.job().image(tools).command(IDLE_CMD).mount("/store", store));
    host.running();
    let counted = rt.exec(host, ["sh", "-c", "wc -l /store/data.txt"], #{ env: #{ LC_ALL: "C" } });
    if !counted.success() { throw `wc exited with ${counted.exit_code()}`; }
    rt.stop(host);
}, #{ description: "Log how many lines the store holds" });

// Warming: certificates for the ingresses, images ahead of use. Only ready()
// and ready_eventually() are allowed on what these return.
app.on_action("warm", |rt, _p| {
    rt.warm_certs(app).ready("5m");
    rt.warm_images(app.job().image(initialiser)).ready_eventually();
}, #{ description: "Provision certificates and pull images ahead of time" });

// A `volume` param asks for a site volume, bound for this operation only and
// delivered as `<name>_volume`.
app.on_action("export", |rt, p| {
    let dest = app.external_volume(p["destination_volume"]);
    let done = rt.start(app.job()
        .image(tools)
        .command(["cp", "/store/data.txt", "/store/FORMAT", "/out/"])
        .mount("/store", store)
        .mount("/out", dest)
    ).terminated_eventually();
    done.ensure_success();
}, #{
    description: "Copy the store into a site volume",
    params: #{ destination: #{ kind: "volume", description: "Site volume to copy into" } },
});

// A choice param, with the web deployment stopped while the store changes.
app.on_action("reset", |rt, p| {
    let line = if p.contents == "sample" { "first entry" } else { "" };
    rt.stop(web);
    in_store.call(rt, tools, `printf '%s\n' ${std::quote_shell(line)} > /store/data.txt`);
    rt.start(web).ready();
}, #{
    description: "Reset the store's contents",
    params: #{ contents: #{ kind: #{ name: "choice", options: ["empty", "sample"] }, default_value: "sample" } },
});

// ── Shells ───────────────────────────────────────────────────────────────────

// A shell is a scripted procedure with an interactive step: prepare, attach,
// then act on how the session ended.
app.on_shell("store", |rt, shell, p| {
    if p.readonly && p.user == "root" {
        shell.error("a read-only session runs as a regular user");
    }
    let ended = shell.attach(app.job()
        .image(tools)
        .command(["sh"])
        .user(if p.user == "root" { "0" } else { "65534" })
        .mount("/store", store));
    if ended.exited() && !ended.success() {
        rt.start(app.job().image(tools).command(["echo", `shell exited with ${ended.exit_code()}`]))
            .terminated()
            .ensure_success();
    }
}, #{
    description: "A shell with the store mounted",
    params: #{
        user: #{ kind: #{ name: "choice", options: ["root", "nobody"] }, default_value: "nobody" },
        readonly: #{ kind: "boolean", default_value: false },
    },
});

// An action and a shell may share a name.
app.on_shell("note", |_rt, shell, _p| {
    shell.attach(app.job().image(tools).command(["sh", "-c", "cat >> /store/data.txt"]).mount("/store", store))
        .ensure_success();
}, #{ description: "Type lines straight into the store; end with Ctrl-D" });
