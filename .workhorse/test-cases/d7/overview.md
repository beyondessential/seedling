# Free-form metadata in seedling.toml

## Bundle rules

- [x] A bundle with no metadata file, or no `metadata` field, has an empty table (verifies spec: BND#metadata-table)
- [x] Strings, integers, floats, booleans, all four TOML date/time types, arrays and nested tables convert as the spec's table gives, with integers and floats kept apart (verifies spec: BND#metadata-table)
- [x] `metadata` that is not a table is `bundle_invalid`, naming the field (verifies spec: DEFN#bundle-rules)
- [x] `inf`, `-inf` and `nan` at any depth are `bundle_invalid`, naming where they sit, through the OI as well (verifies spec: DEFN#bundle-rules)
- [x] A bundle for an unsupported Seedling with an unreadable table is still reported as unsupported (verifies spec: DEFN#seedling-version-support)

## Scripts

- [x] `app.metadata()` reads the table, nested tables as maps, and a missing key as `()` (verifies spec: BND#metadata-table)
- [x] Changing the returned map leaves a later call unchanged (verifies spec: BND#metadata-table)
- [x] `app.metadata()` works in the action and validator contexts (verifies spec: BND#metadata-table)
- [x] In a gate body, `last.metadata()` reads the last success's bundle while `app.metadata()` reads the current one (verifies spec: BND#metadata-table)

## Provenance

- [x] Pushed and fetched definitions report the table, `{}` without one (verifies spec: DEFN#provenance-reporting)
- [x] An unsupported template whose metadata table breaks the rules reports `metadata: null` (verifies spec: DEFN#provenance-reporting)
- [x] An unsupported template with a field this Seedling does not know, or a missing script file, still reports its readable table (verifies spec: DEFN#provenance-reporting)
- [x] The generation history reports the same table as the app detail (verifies spec: DEFN#provenance-reporting)

## Web console

- [x] The definition section shows each top-level key with its value, arrays and tables as JSON (verifies spec: UI#app-definitions)
- [x] No metadata row for an empty table (verifies spec: UI#app-definitions)
