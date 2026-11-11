# Rewrite specs in spec voice, remove tracey

## Decisions

- Specs move from `docs/spec/{language,runtime,interface,web}.md` to `.workhorse/specs/`, one area folder per former file, split along top-level headings.
  The split map is `split.py` beside this plan.
- Root `.workhorse/specs/overview.md` (id `SEED`) is the meta spec: one line on Seedling, requirements notation, external documents, terminology.
- Ids are short and unprefixed; tracey's `r`/`l`/`i`/`w` prefixes go.
- Tracey annotations in code (`r[impl x]`, `l[verify x]`, …) become `// spec: ID#heading-slug`, with adjacent duplicates collapsed.
  Placement rule carries over: the citation sits on the statement that implements the requirement, not on the enclosing item.
- `tracey.yml` and `.config/tracey/` go.
  CI gains: lychee offline link and anchor check over `.workhorse/specs`; `etc/ci/check-spec-citations.sh` resolving every `// spec:` citation to a spec id and heading; `etc/ci/check-spec-style.sh` for unique ids, no em-dashes, no keywords inside notes.
- `AGENTS.md` tracey rules are rewritten for `// spec:` citations; `.workhorse/rules.md` gains house style, "what not how", "present not past" sections modelled on bliti.
- `threat-model.md`, `failure-modes.md`, `README.md` and code doc comments are repointed; `docs/plans/*.md` are left as they stand.
- Terminology: **app** (not "Seedling Application"); **host** (not node or instance) for the machine one daemon runs on; **runtime** (not "control plane"); **resource graph** for the evaluated definition (formerly "AppDef"), distinct from the **definition bundle**.
- Conflict with `.agents/docs/spec-format.md` (plain bullets, no RFC 2119 keywords): spec-voice wins per the card; recorded in `.workhorse/rules.md`.

## Mechanics

The split keeps tracey rule markers (`> r[x.y]`) in place so each rewrite can map old rules to new sections.
Links to rules become placeholders `](rule:r/x.y)` during the split.
Each spec rewrite writes `rule-map/<ID>.tsv` beside this plan: `<prefix>/<rule id>\t<ID>#<slug>` (slug empty when the rule maps to the whole spec).
A final sweep (`sweep.py`) resolves placeholders in specs, converts code annotations, and repoints docs, all from the combined map.

## Conventions settled in the pilot

(filled in as the pilot settles them)

## Steps

- [ ] Split `docs/spec/*.md` into `.workhorse/specs/` per `split.py`
- [ ] Pilot: root overview and `runtime/reconciliation.md`; voice signed off
- [ ] Terminology sweep across split specs
- [ ] Voice rewrite, one spec per pass, each emitting its rule map
- [ ] Sweep code annotations and spec links from the rule map
- [ ] Remove tracey config and workflow; add CI checks
- [ ] Update `AGENTS.md`, add `.workhorse/rules.md`, repoint docs
- [ ] cargo build, clippy, tests; CI scripts pass locally
- [ ] Pause for the whole-spec review loop

## Gaps

(running list)
