# Bundle includes and definition packing

Sibling definitions share files through an `include` table in `seedling.toml`, which the CLI resolves when it reads a definition, confined to the enclosing repository.
The server only checks that every included path is present.
`definitions pack` writes the assembled bundle as a definition artefact layer, and the CLI sends a lone script file as a bundle under its own name.

Specs: BND (included files), DEFN (bundle rules), CLI (definition sources, include assembly, definition packing).

## Tech notes

- **Bundle rules move to a shared crate.**
  `seedling-ctl` depends only on `seedling-protocol`, but `definitions pack` applies the bundle rules, and AGENTS.md treats two implementations of one contract as the bug.
  `crates/core/src/runtime/definition/bundle.rs` and `version.rs` move into a new `crates/bundle` (`seedling-bundle`), used by both core and ctl.
  The deterministic tar.gz writer goes there too, beside `Bundle::from_tar_gz`, so the reader and writer are tested as a round trip.
  It is a new crate rather than part of `seedling-protocol`, which keeps toml, semver, tar and flate2 out of the wire-types crate.
- **One assembly over one tree view.**
  Local folders and GitHub URLs both resolve to "a folder inside a boundary", and include assembly runs once over that.
  For GitHub, the CLI extracts the whole repository at the commit, not just the folder, and the extraction root is the boundary.
- **GitHub symlinks are recorded, never written.**
  Extraction currently fails on any non-regular entry beneath the folder.
  With whole-repository extraction, a symlink elsewhere in the repository must not fail a definition that never reads it.
  So extraction records non-regular entries' paths without writing them, and refuses to write any entry beneath a recorded path.
  Assembly treats a recorded path on a source's way down from the boundary as a symlink.
  Writing symlinks into the temporary folder is ruled out, because a later entry written through one escapes the extraction (tar-slip).
- **Out-of-bounds checks are lexical.**
  The source path is the folder's path relative to the boundary, joined with the value, then normalised.
  For a local folder, each segment from the boundary down is then checked without following symlinks, before any read.
- **`Definition::Script` goes.**
  Once a lone file is sent as a bundle, the CLI never sends `script` or `body`, so the variant and `DefinitionKeys.script` are removed, not left unused.
  The server keeps the `script` and `body` members for the console and other clients.

## Checklist

### Before starting

- [ ] Run `cargo clippy`, `cargo fmt --check`, `etc/ci/check-spec-style.sh` and `etc/ci/check-spec-citations.sh`, and fix anything outstanding

### Shared bundle crate

- [ ] Create `crates/bundle` (`seedling-bundle`) in the workspace, with the dependencies `bundle.rs` and `version.rs` use (`cargo add`)
- [ ] Move `bundle.rs` and `version.rs` into it, and update every `use` in core and its tests to the new paths, with no re-export shim left in core
- [ ] Add a deterministic `Bundle::to_tar_gz`:
  - entries in ascending path order, each directory before its contents
  - fixed mtime, mode and owner, and no gzip file name or timestamp
  - regular files and directories only
- [ ] Unit tests: `to_tar_gz` then `from_tar_gz` round-trips the files and the content hash; packing twice gives identical bytes

### Server: include table (BND#included-files)

- [ ] In `read_script`, accept an `include` field and check it against the bundle's files with a new `read_include`:
  - the value is a table of non-empty strings
  - each key is a valid bundle path, is not the root or `seedling.toml`, and is neither equal to nor beneath another key once normalised
  - each key names a file, or a directory with at least one file beneath it
  - each error message names the key
- [ ] Core tests in `crates/core/src/tests/bundle.rs`, one per rule above, plus:
  - a present file key and a present directory key are accepted
  - an unsupported template with a malformed `include` is still stored

### CLI: lone file sources (CLI#definition-sources)

- [ ] In `DefinitionArgs::resolve`, read a file path as a bundle: the file under its own name, and a `seedling.toml` serialised with `toml` whose only field is `script`, naming it
- [ ] Refuse a file named `seedling.toml`, or with a non-UTF-8 name
- [ ] Remove `Definition::Script` and `DefinitionKeys.script`, and update `apps`, `templates` and `apps plan` callers
- [ ] Tests: a lone file is sent as `bundle` with both files; a name needing TOML quoting round-trips; the two refusals

### CLI: include assembly (CLI#include-assembly)

- [ ] Find the boundary for a local folder: the nearest directory at or above it holding a `.git` or `.jj` entry, otherwise the folder itself
- [ ] Change GitHub `download` to extract the whole repository at the commit into a temporary folder, recording non-regular entries' paths instead of writing them, and refusing entries beneath a recorded path or with a non-normal component
- [ ] Scope the "outside" error to entries outside the repository (CLI#definition-sources)
- [ ] Add `assemble(folder, boundary, special_paths)` in `crates/ctl/src/definition.rs`, run after `read_folder` for both local and GitHub sources. For each key:
  - compute the source path and classify it as out of bounds, missing, a file or a directory
  - refuse a source reached through a symlink or recorded path, or that is neither a file nor a directory
  - read a directory source with `read_folder`, and refuse one that yields nothing or holds a `seedling.toml` at any depth
  - compare with the folder's files at or beneath the key: use the folder's files when the source is out of bounds or missing, refuse on a mismatch, and refuse when neither exists
  - place the included files without applying the folder's `.seedignore`
- [ ] Parse `seedling.toml` for `include` leniently enough to assemble, and leave all other metadata checks to the bundle rules
- [ ] Cite `// spec: CLI#include-assembly` at each rule
- [ ] Tests in `crates/ctl/src/definition/tests.rs`, using temporary directories with `.git` or `.jj` markers:
  - a file include, a directory include, and an include inside the folder
  - boundary at `.git`, at `.jj`, and at the folder when neither exists
  - each out-of-bounds form (`..` past the boundary, absolute, null byte, through `.git`), with and without a folder copy
  - a symlinked segment beneath the boundary is refused; a symlink above the boundary is fine
  - a directory source that is empty, or holds a nested `seedling.toml`, is refused
  - a folder copy that matches, one that differs, and one used because the source is missing
  - an included path matched by `.seedignore` is still placed
  - an `apps export` of an app with includes, written outside any repository, pushes again unchanged
- [ ] GitHub tests, built from synthetic tarballs as the existing `extract_folder` tests are:
  - an include reaching a sibling folder at the same commit
  - a symlink elsewhere in the repository does not fail the definition
  - an include through a symlink entry is refused
  - an entry beneath a symlink entry is refused

### CLI: definition packing (CLI#definition-packing)

- [ ] Add a `definitions` command group in `crates/ctl/src/main.rs`, with `pack <source> <file>` taking a file, folder or GitHub URL (not `--ref`)
- [ ] Resolve and assemble the source as for a push, then apply the strict bundle rules: `Bundle::from_files` and the script reading, without the running-version check
- [ ] Refuse an existing `file` before doing any work, and write nothing on any error
- [ ] Write `Bundle::to_tar_gz`, then print the declared Seedling version requirement where there is one
- [ ] Tests:
  - a bundle that breaks a rule writes nothing and names the path or field
  - an existing output file is refused
  - two packs give identical bytes
  - the requirement is printed when declared, and nothing when not
  - a packed bundle read back by `from_tar_gz` has the content hash of the same source pushed

### Docs

- [ ] `docs/bsl-scripting.md`, Definition bundles: the `include` table, concatenating shared scripts through `script`, and the confinement rules in brief
- [ ] `docs/deploying.md`, App definitions:
  - a lone script keeps its name
  - includes from a repository or GitHub
  - re-pushing an export
  - publishing uses `seedling-ctl definitions pack` in place of `tar`, with the printed requirement passed to `--annotation`

### Finish

- [ ] Run `cargo clippy`, `cargo fmt`, the spec style and citation checks, and the core and ctl test suites
