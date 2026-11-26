// Demo: bundle-share.
//
// The smaller half of a pair with the bundle-tour demo: both bundles include
// the same helper script and the same folder from demos/shared, so one copy in
// the repository serves two definitions. This one builds its web server with
// the shared helper and serves the shared page as its home page. Install both
// and the same page, with the same stylesheet, answers on each hostname.
//
// Try on a dev VM:
//   seedling-ctl apps create bundle-share demos/bundle-share
//   seedling-ctl apps install bundle-share
//   curl -k https://bundle-share.demo.internal/   # or browse it
// Then edit demos/shared/www/shared.html and push both demos again with
// `seedling-ctl apps update <app> demos/<app>`: each picks up the change.

app.description("Demo: a second bundle including the same helper script and page as bundle-tour, from one shared folder");
app.canopy_type("seedling-demo");

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("bundle-share.demo.internal")
    .description("Hostname the shared page is served on");

// `static_site` is defined by lib/static-site.rhai, copied in from
// demos/shared and run before this file.
static_site.call("site", hostname.value())
    .write("/index.html", app.file("shared/shared.html"))
    .write_dir("/shared", app.dir("shared"));
