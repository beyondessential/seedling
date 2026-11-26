// The second script file of the bundle-tour demo. It runs after
// lib/static-site.rhai and before app.seed.rhai, in the same scope as both,
// so `meta`, `banner` and `metadata_rows` below are in scope in app.seed.rhai.

// The `[metadata]` table of seedling.toml as a map. Each call returns a new
// map, so changing this one changes nothing else.
let meta = app.metadata();

// A key the table does not hold reads as the unit value, so a script can
// fall back to a default. Add `banner = "..."` under `[metadata]` to set one.
let banner = if meta.banner == () { "No banner is set in the metadata." } else { meta.banner };

// One table row per metadata value: its key, the BSL type the TOML value
// arrived as, and the value written out as JSON.
let metadata_rows = "";
for key in meta.keys() {
    let value = meta[key];
    metadata_rows += `<tr><td><code>${key}</code></td><td>${type_of(value)}</td><td><pre>${std::encode_json(value)}</pre></td></tr>
`;
}
