// Beset is a control-plane and DSL that defines how to run an application
// but also how to upgrade it, how to debug it, perform actions on it, etc
// and it works the same way on a single Linux container host or a Kubernetes

// grab a parameter from the control plane, specific to this app
let domain = app.param("domain");
// `app` is a pre-established reference to the App you're defining

// you can use closures to make yourself little helpers
let version = app.param("version");
let image = |name| `ghcr.io/beyondessential/tamanu-${name}:${version}`;

// a service is a network endpoint that operators and other resources can access
let control = app.service("frontend-control");

// specialising a service as http() enables path routing and other goodies
let sync_api = app.service("sync-api").http(4100);

// an ingress is a publicly routable service
let traffic = app.service("public")
    .ingress(domain, 443).http() // terminate HTTPS for the domain, and output HTTP/1.1
    .service() // get the service instance back
    .http(80); // specialise to get path routing

// defining something doesn't start it immediately, see actions (on_...) below
let frontend = app.deployment("frontend")
    .image(image.call("frontend"))

    // this will ensure that there's always 2 instances
    // if one dies, another will start, if there's ever 3, beset will end one
    .scale(2)

    // http traffic can be routed by path
    .http(3000, traffic.route("/"))

    // tcp/udp traffic can only be routed by port
    .tcp(2525, control.port(25))

    // if the service-side port isn't given, it defaults to the deployment-side port
    .tcp(8080, control);

// grab an external volume
// this is like a param, but the control plane has to provide a volume instead of a string
let pg_socket = app.external_volume("postgres-unix-socket");

// if you don't need to refer to it, you can leave it unassigned
app.deployment("api")
    .image(image.call("facility"))
    .command("serveApi")
    .scale(1..AVAILABLE_THREADS)

    .mount("/var/run/postgres", pg_socket)

    // mount is not just for volumes
    .mount(sync_api.port(4100))

    // HTTP path routes will match by segment length, not definition order
    .http(3000, traffic.route("/api"))
    .http(3000, traffic.route("/v1"));
    // if you need something more complicated, spin up a custom HTTP server

app.deployment("sync")
    .image(image.call("facility"))
    .command("startSync")
    .mount("/var/run/postgres", pg_socket)

    .http(4100, sync_api) // not using route() will default to /

    // shut down the existing process before starting the new one
    .on_update(OnUpdate.Replace);

app.deployment("tasks")
    .image(image.call("facility"))
    .command("startTasks")
    .mount("/var/run/postgres", pg_socket)
    .on_update(OnUpdate.Replace);

// this is a closure, so the migrate Job is not static
// and is instead defined when the closure is called
let migrate = || app.job("migrate")
    .image(image.call("facility"))
    .command("migrate")
    .mount("/var/run/postgres", pg_socket);

// actions are available to operators to run
let migrate_action = app.on_action("migrate", |rt| {
    // `rt` is the "runtime", and is how you control resources
    // (vs defining them or selecting them)

    // we dynamically create a "migrate" job
    let job = migrate.call();

    let waiter = rt.start(job); // we schedule the job; beset will eventually start its container

    // wait until it's terminated, and "block" until then
    waiter.terminated();

    // this could also be written like this:
    // rt.start(migrate.call()).terminated();
}, #{
    description: "Migrate the database",
});

// some actions are special, so they have first-class syntax,
// but are also equivalent to something like:
// on_action("start", |rt| { ... }, #{ description: "Start the app" })
app.on_start(|rt| {
    // start the networking
    rt.start([traffic, control]);

    rt.start(frontend) // start the frontend
        .scheduled(); // scheduled() only waits until the containers are allocated
                      // they may be just starting to pull their image

    // with select(), you can search for resources instead of matching exactly
    rt.start(app.select(#{ types: [ResourceType.Deployment], names: ["sync", "tasks"] }))
        .running(); // running() waits until the resources are actually running

    // start everything else
    rt.start(app).ready(); // ready() waits until things are ready to be used
});

// shells are available to operators to debug
// they're essentially actions that provide an interactive terminal/shell
app.on_shell("node", |_rt| {
    app.job("shell-node")
        .image(image.call("central"))
        .command("shell")
    // a Resource is returned, so beset rt.start()s it and then attaches when ready()
}, #{
    description: "A Node.js shell in the Tamanu app context",
});

// for more control including cleanup
app.on_shell("db", |rt, attach| {
    // this job is only defined here, so it can't be used outside this block
    let shell = app.job("shell-db")
        .image(image.call("toolbox"))
        .command(["bestool", "t", "p"]);
    rt.start(shell).running();
    attach.call(shell); // this "blocks" until the shell exits

    rt.start(app.job("shell-db-cleanup")
        .image(image.call("toolbox"))
        .command(["bestool", "t", "p", "-e", "select revert_changes();"])
    ).terminated();
});
// you don't need to provide a description, but it *is* nicer

version.on_change(|rt, old| {
    rt.start(control);

    rt.reconcile(old.select(#{ types: [ResourceType.Ingress], names: ["public"] }).one(), traffic);

    rt.stop([
        old.select(#{ types: [ResourceType.Deployment], names: ["sync", "tasks", "api"] }),
        old.select(#{ types: [ResourceType.Deployment], name_patterns: ["fhir-*"] }),
    ]);

    let migration = rt.start(migrate.call());

    rt.start(frontend);
    rt.stop(old.select(#{ types: [ResourceType.Deployment], names: ["frontend"] }));

    rt.stop(old);

    try {
        migration.terminated().ensure_success();
    } catch (_err) {
        rt.start(migrate.call()).terminated().ensure_success();
    }

    rt.start(app).ready();
});

// on first install, we can have a special sequence of events
// and we can also require/prompt the user for information
app.on_install(|rt, reqs| {
    // because this job is only defined in this block,
    // it will not exist at all on subsequent boots
    let provision = app.job("provisioning")
        .image(image.call("central"))
        .command(["provision", "/prov/isioning.json5"])
        .mount("/prov", {
            // this could be defined in the higher scope,
            // but it's often cleaner to have the entire setup contained

            let vol = app.volume();

            let users = #{};
            users[reqs.admin_user_email] = #{
                role: "admin",
                password: reqs.admin_user_password,
                displayName: "Super Admin",
            };

            vol.write("/isioning.json5", #{
                users: users,
            }.to_json());

            vol
        });

    rt.start(frontend).ready();
    rt.start(migrate.call()).terminated();
    rt.start(provision).terminated().ensure_success();
    rt.start(app.select(#{ types: [ResourceType.Action], names: ["start"] }));
}, #{
    // here, we ask for the credentials of the first (superadmin) user:
    // all values are required unless `required: false` is set...
    admin_user_email: #{
        // ...but if a default_value is given, then that's prepopulated
        default_value: "admin@tamanu.app",
        kind: "email",
        description: "The login for the superadmin user",
    },
    admin_user_password: #{
        // ...and if kind=password, then Beset will generate a random one
        kind: "password",
        description: "Initial password for the superadmin user",
        // ...and restrict weak passwords, unless kind="weak-password" is set
    },
});
