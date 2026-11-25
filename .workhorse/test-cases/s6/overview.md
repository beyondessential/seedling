# Bundle includes and definition packing

Scenarios for sharing files between sibling definitions and packing definitions for registries.

## Server: include table

- [x] A bundle whose `include` keys name a file and a directory it holds is accepted (verifies spec: BND)
- [x] An `include` that is not a table, has a non-string or empty value, an invalid, absolute or root key, a `seedling.toml` key, a duplicate or nested key, or a key naming nothing in the bundle is refused with `bundle_invalid` naming the key (verifies spec: BND, DEFN)
- [x] A bundle for a later Seedling with a malformed `include` is reported unsupported rather than invalid (verifies spec: DEFN)

## CLI: lone files

- [x] Pushing `web.seed.rhai` sends a bundle holding it under that name and a `seedling.toml` naming it in `script` (verifies spec: CLI)
- [x] Pushing a file called `seedling.toml`, or one with a non-UTF-8 name, is refused before any request (verifies spec: CLI)
- [x] Pushing a lone script with a syntax error reports the error against the script's own file name (verifies spec: CLI)

## CLI: include assembly

- [x] A folder in a `.git` or `.jj` repository includes a file and a folder from beside it (verifies spec: CLI)
- [x] Outside any repository, an include reaching above the folder is refused (verifies spec: CLI)
- [x] An include that climbs past the repository, is absolute, holds a null byte or enters `.git` is refused without being read, and a copy in the folder stands in for it (verifies spec: CLI)
- [x] An include through a symlink beneath the repository root is refused; a repository reached through a symlinked parent works (verifies spec: CLI)
- [x] An included folder that is empty, or holds a `seedling.toml` at any depth, is refused (verifies spec: CLI)
- [x] A copy in the folder that matches its source is accepted, one that differs is refused, and one whose source is gone is used as it stands (verifies spec: CLI)
- [x] An included path matched by the folder's `.seedignore` is still placed (verifies spec: CLI)
- [x] An exported app with includes, written outside any repository, pushes again to the same files (verifies spec: CLI)
- [x] A GitHub URL includes from a sibling folder at the same commit, and a symlink elsewhere in the repository does not fail it (verifies spec: CLI)
- [x] A GitHub include through a symlink entry, or an archive entry beneath one, is refused, and nothing from the archive is written to disk (verifies spec: CLI)
- [x] The same definition folder, with nested `.seedignore` files and includes, reads the same from disk and from a GitHub URL (verifies spec: CLI)
- [x] A link a GitHub definition's `.seedignore` leaves out does not fail it (verifies spec: CLI)

## CLI: packing

- [x] A packed folder reads back as the bundle a push of it would send, and packing twice gives identical bytes (verifies spec: CLI)
- [x] Packing prints the version annotation when the definition declares a requirement (verifies spec: CLI)
- [x] Packing a bundle that breaks a bundle rule, or over an existing file, writes nothing (verifies spec: CLI)
- [x] A packed lone file keeps its name as the bundle's script file (verifies spec: CLI)
- [ ] A definition packed with `definitions pack`, pushed with `oras` using the printed annotation, installs on a host with `apps create --ref` (verifies spec: CLI, DEFN)
