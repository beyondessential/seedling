# Brief: writing an interface spec for unspecced Operator Interface methods

Repository root: `/home/felix/.workhorse/repos/beyondessential/seedling/worktrees/c6`.
Several Operator Interface (OI) methods exist in the code (`crates/core/src/oi/handler.rs` dispatch and the handlers it calls) but no spec describes their wire contract.
You write one new spec covering one family, in the standards voice.

## Read first, in full

- `/home/felix/.workhorse/repos/.claude/skills/spec-voice/style.md` and `.workhorse/rules.md`.
- `.workhorse/specs/overview.md` (glossary) and `.workhorse/plans/c6/gaps.md` (every "Decided" section is settled).
- A sibling interface spec as the model for shape: `.workhorse/specs/interface/images.md` and `.workhorse/specs/interface/app-control.md`.
- `.workhorse/specs/interface/wire.md`, especially the error codes; `.workhorse/specs/interface/events.md`; and the runtime specs that already specify the behaviour behind your methods (named in your assignment).

## What the spec holds

- Frontmatter `id` as assigned, an H1, one "Applies to …" sentence.
- One `##` section per method or tight group of methods, each with a parameter table (`| member | type | required | meaning |`), the result shape, the refusals with their error codes, the event each emits (linking the EVT row), and whether it records an audit entry.
- The wire contract only. Runtime behaviour already specified elsewhere (SITE, ACTU, LINUX, FLT) is linked, not restated.

## Where the code and the rules disagree

The code is the source for what each method takes and returns, but not authority where it breaks a settled rule. Where it does, the spec states the rule and you report the divergence:

- A malformed or invalid parameter, a duplicate name, or a reference to something that does not exist is a request error (`requirements_invalid`, or `not_found` where the method addresses a named object that is absent, following how sibling specs use it). It is never `internal`.
- `internal` is only for a failure inside the server, and a failed read is never reported as absence or an empty result.
- Reserved names are rejected at creation only, as IDN#reserved-names specifies.
- Mutating methods are attributed to the request's actor.
- Where the code returns an ad-hoc success flag (`{ "created": true }`), keep the code's shape unless a sibling spec's convention clearly differs; if it does, follow the sibling and report it.

## Also

- Check that every method you specify has a CLI command (AGENTS.md: everything in the OI has one). Report any without one; do not specify new CLI commands.
- Add your spec to any index that lists the interface specs (`.workhorse/specs/interface/overview.md` if it lists them).
- Run `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh` and `lychee --offline --include-fragments=anchor-only --no-progress '.workhorse/specs/**/*.md'` when done. Other agents are writing sibling specs at the same time; ignore failures outside your files.
- Do not edit code or commit.

## What you return

The spec's path and id, a one-line summary per section, a "Code divergences" list (file, what the code does, what the spec requires), and any CLI gaps.
