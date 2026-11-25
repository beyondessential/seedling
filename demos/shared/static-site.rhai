// Shared helpers for the bundle demos, which copy this file in through the
// `[include]` table of their seedling.toml and list it first in `script`.
// Script files run in one scope, so the closures below are in scope for every
// file listed after this one. Not a demo of its own.

// Serves a static website over HTTPS on `hostname`: busybox httpd on a high
// port, behind an HTTP service and a deployment both called `name`. Returns
// the volume the site is read from, for the caller to write its files into.
//
// The site volume is memory-backed, since everything in it comes from the
// bundle, and named after a digest of the whole bundle. A push that changes
// any file gives a volume of a new name, which the runtime creates and writes
// afresh, and the deployment mounting it is replaced to match; the old
// volume, no longer declared, is deleted.
let static_site = |name, hostname| {
    let digest = std::encode_hex(std::digest_blake3(app.dir(".")));
    let site = app.volume(`${name}-${digest.sub_string(0, 12)}`).tmpfs().readonly();

    // httpd offers a file with a suffix it does not know as a download; these
    // suffixes show as text in the browser instead.
    let conf = app.volume(`${name}-httpd`).tmpfs().readonly()
        .write("/httpd.conf", ".rhai:text/plain\n.toml:text/plain\n.seedignore:text/plain\n");

    let web = app.service(name).http(80);
    web.ingress(hostname, 443)
        .tls(Terminate.Https, Output.Http1)
        .redirect();

    app.deployment(name)
        .image("docker.io/library/alpine:3.24.4")
        .command(["httpd", "-f", "-p", "8080", "-h", "/www", "-c", "/etc/site/httpd.conf"])
        .mount("/www", site)
        .mount("/etc/site", conf)
        .http(8080, web.route("/"));

    site
};

// Wraps `body`, a fragment of HTML, in a page titled `title` that uses the
// shared stylesheet, which each demo writes at /shared/style.css.
let render_page = |title, body| `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>${title}</title>
<link rel="stylesheet" href="/shared/style.css">
</head>
<body>
<main>
<h1>${title}</h1>
${body}
</main>
</body>
</html>
`;
