// Demo: bundle-tour.
//
// A definition as a folder rather than a lone script: a seedling.toml with a
// Seedling version requirement, three script files run in order, a metadata
// table holding one value of each TOML type, sidecar files, a file and a
// folder included from demos/shared, and a .seedignore that leaves a file
// out. The script writes what it reads from the bundle into a volume, and a
// web server shows the result over HTTPS: a home page rendered from the
// metadata and the release notes, the files written into the volume, and a
// copy of the whole bundle as the host received it. Not a production
// definition.
//
// Try on a dev VM:
//   seedling-ctl definitions pack demos/bundle-tour /tmp/bundle-tour.tar.gz
//     # assembles the bundle as a push would, and reports the version requirement
//   seedling-ctl apps create bundle-tour demos/bundle-tour
//   seedling-ctl apps install bundle-tour
//   curl -k https://bundle-tour.demo.internal/   # or browse it
//   seedling-ctl apps logs bundle-tour            # what the release gate logged
// Then change `release` in seedling.toml, or edit notes.txt, and push again
// with `seedling-ctl apps update bundle-tour demos/bundle-tour`.

app.description("Demo: a definition bundle's metadata, script files, sidecar files and includes, written into a volume and served over HTTPS");
app.canopy_type("seedling-demo");

let hostname = app.param("hostname")
    .kind("hostname")
    .default_value("bundle-tour.demo.internal")
    .description("Hostname the site is served on");

// ── The site ─────────────────────────────────────────────────────────────────

// `static_site` comes from lib/static-site.rhai, the first script file, which
// the bundle holds because seedling.toml includes it from demos/shared.
let site = static_site.call("site", hostname.value());

// A sidecar file, read as text for the home page and written as a file below.
let notes = app.file("notes.txt");

// `render_page` also comes from the shared helper; `banner` and
// `metadata_rows` come from metadata.rhai, the second script file.
let home = render_page.call(`bundle-tour ${meta.release}`, `
<p class="banner">${banner}</p>
<p><img src="/logo.png" alt="" width="32" height="32"> Build ${meta.build},
published ${meta.published}, maintained by ${meta.maintainer.name}.</p>

<h2>The metadata table</h2>
<p>Each value of <code>[metadata]</code> in seedling.toml, as
<code>app.metadata()</code> returns it.</p>
<table>
<tr><th>key</th><th>BSL type</th><th>value</th></tr>
${metadata_rows}</table>

<h2>Release notes</h2>
<p>From <code>app.file("notes.txt").text()</code>; the file itself is at
<a href="/notes.txt">/notes.txt</a>.</p>
<pre>${notes.text()}</pre>

<h2>Written into the site volume</h2>
<ul>
<li><a href="/notes.txt">/notes.txt</a> and <a href="/logo.png">/logo.png</a>:
single sidecar files, byte for byte.</li>
<li><a href="/guide.html">/guide.html</a>: the <code>pages</code> folder,
written into the root of the volume.</li>
<li><a href="/shared/shared.html">/shared/shared.html</a> and
<a href="/shared/style.css">/shared/style.css</a>: the folder included from
demos/shared/www.</li>
<li><a href="/draft.html">/draft.html</a>: not found, since .seedignore left
<code>pages/draft.html</code> out of the bundle.</li>
</ul>

<h2>The bundle as the host received it</h2>
<p>Every file of the bundle, written under /bundle. The two included paths are
here although this folder in the repository holds neither.</p>
<ul>
<li><a href="/bundle/seedling.toml">seedling.toml</a></li>
<li><a href="/bundle/.seedignore">.seedignore</a></li>
<li><a href="/bundle/lib/static-site.rhai">lib/static-site.rhai</a> (included)</li>
<li><a href="/bundle/metadata.rhai">metadata.rhai</a></li>
<li><a href="/bundle/app.seed.rhai">app.seed.rhai</a></li>
<li><a href="/bundle/notes.txt">notes.txt</a></li>
<li><a href="/bundle/assets/logo.png">assets/logo.png</a></li>
<li><a href="/bundle/pages/guide.html">pages/guide.html</a></li>
<li><a href="/bundle/shared/shared.html">shared/shared.html</a> (included)</li>
<li><a href="/bundle/shared/style.css">shared/style.css</a> (included)</li>
<li><a href="/bundle/pages/draft.html">pages/draft.html</a>: not found</li>
</ul>
`);

// Declared writes land in the order they are declared, each replacing what
// its path held.
site
    // A string, written as its UTF-8 bytes.
    .write("/index.html", home)
    // Files, written byte for byte; logo.png is not text.
    .write("/notes.txt", notes)
    .write("/logo.png", app.file("assets/logo.png"))
    // Folders: every file beneath the bundle path, at its path relative to
    // it. `write_dir` may target the volume's root, which `write` may not.
    .write_dir("/", app.dir("pages"))
    .write_dir("/shared", app.dir("shared"))
    // "." names the bundle root, so this is the whole bundle.
    .write_dir("/bundle", app.dir("."));

// ── A gate reading the previous bundle ───────────────────────────────────────

// Runs on install, and again whenever a push changes the release in the
// metadata. `last` is the app as it stood at the gate's last success, with
// that generation's bundle: `last.metadata()`, `last.file()` and `last.dir()`
// read it. The body logs where the release came from through a short job;
// see the actions-and-gates demo for what else a gate can do.
app.gate("release")
    .key(meta.release)
    .run(|rt, last| {
        // `app.metadata()` may be called in any context, a gate body included.
        let now = app.metadata().release;
        let message = if last == () {
            `first release seen: ${now}`
        } else {
            let was = last.metadata().release;
            // Throws if the previous bundle held no notes.txt.
            let was_notes = last.file("notes.txt").text().split("\n")[0];
            let pages = if std::encode_hex(std::digest_blake3(last.dir("pages")))
                == std::encode_hex(std::digest_blake3(app.dir("pages"))) { "unchanged" } else { "changed" };
            `release ${was} -> ${now}; pages ${pages}; previous notes: ${was_notes}`
        };
        rt.start(app.job()
            .image("docker.io/library/busybox:1.38.0-musl")
            .command(["echo", message])
        ).terminated().ensure_success();
    });
