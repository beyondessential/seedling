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
    .strategy(DeploymentStrategy.Replace);

app.deployment("tasks")
    .image(image.call("facility"))
    .command("startTasks")
    .mount("/var/run/postgres", pg_socket)
    .strategy(DeploymentStrategy.Replace);

// this is a closure, so the migrate Job is not static
// and is instead defined when the closure is called
let migrate = || app.job("migrate")
    .image(image.call("facility"))
    .command("migrate")
    .mount("/var/run/postgres", pg_socket);

// actions are available to operators to run
app.on_action("migrate", |rt| {
    // `rt` is the "runtime", and is how you control resources
    // (vs defining them or selecting them)
    rt.run(migrate.call());
    // run() will setup the job, start its container,
    // wait until it's successfully done, and "block" until then
}, #{
    description: "Migrate the database",
});

// some actions are special, so they have first-class syntax,
// but are also equivalent to something like:
// on_action("start", |rt| { ... }, #{ description: "Start the app" })
app.on_start(|rt| {
    // start the networking
    rt.start([traffic, control]);

    rt.start(frontend); // start the frontend
    // start "blocks" until things are properly started

    // with select(), you can search for resources instead of matching exactly
    rt.start(app.select(#{ kinds: ["deployment"], name: ["sync", "tasks"] }));

    // you'll notice we didn't explicitly start sync_api:
    // as it's referenced by a mount() on "sync", it gets started on-demand

    // start everything else
    rt.start(app);
});

// shells are available to operators to debug
// they're essentially actions that provide an interactive terminal/shell
app.on_shell("node", |_rt| {
    app.job("shell-node")
        .image(image.call("central"))
        .command("shell")
    // a Resource is returned, so beset rt.start()s it and then attaches
}, #{
    description: "A Node.js shell in the Tamanu app context",
});

// for more control including cleanup
app.on_shell("db", |rt, attach| {
    // this job is only defined here, so it can't be used outside this block
    let shell = app.job("shell-db")
        .image(image.call("toolbox"))
        .command(["bestool", "t", "p"]);
    rt.start(shell);
    attach(shell); // this "blocks" until the shell exits

    rt.run(app.job("shell-db-cleanup")
        .image(image.call("toolbox"))
        .command(["bestool", "t", "p", "-e", "select revert_changes();"]))
});
// you don't need to provide a description, but it *is* nicer

// technically equivalent too, but this one has the extra `old` argument
// while on_action("upgrade", ...) doesn't
app.on_upgrade(|rt, old| {
    // this starts a new version of the control service
    rt.start(control);

    // this instead matches up the old ingress and merges new and old together
    // it will transfer control of the actual underlying resources (sockets etc)
    // and make sure everything is configured as per the new spec as things come online
    // but it won't drop traffic to the old resources until they stop
    rt.reconcile(old.select(#{ kinds: [ResourceType.Ingress], names: ["public"] }).one(), traffic);

    // given an array, it stops all in parallel
    rt.stop([
        old.select(#{ kinds: [ResourceType.Deployment], names: ["sync", "tasks", "api"] }),
        old.select(#{ kinds: [ResourceType.Deployment], name_patterns: "fhir-*" }),
        old.select(#{ kinds: [ResourceType.CronJob] }),
    ]);

    // run()ing a job waits until it's done, but start()ing doesn't
    let migration = rt.start(migrate.call());

    rt.start(frontend);
    rt.stop(old.select(#{ kinds: [ResourceType.Deployment], names: ["frontend"] }));

    rt.stop(old);

    // wait until the migration job is done
    rt.wait(migration);

    rt.start(app);
});

// if the host crashes or power was lost...
app.on_crash_recovery(|rt, history| {
    // ...and we were in an upgrade...
    if history.was_upgrading() {
        // ...then migrate and...
        rt.run(migrate.call());
    }

    // ...then do a normal start
    rt.action("start");
    // (if the "crash_recovery" action isn't set, it defaults to calling "start")
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

    rt.start(frontend);
    rt.run(migrate.call());
    rt.run(provision);
    rt.start(app);
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
