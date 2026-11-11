# Brief: rewriting a Seedling spec in spec voice

Repository root: `/home/felix/.workhorse/repos/beyondessential/seedling/worktrees/c6`.

The specs were tracey files (`docs/spec/{language,runtime,interface,web}.md`) and have been split, unchanged, into `.workhorse/specs/<area>/<slug>.md`.
Each split file has frontmatter `id`, an H1, and the old rule blocks (`> r[rule.id]` blockquotes) under `##` headings carried over from the source, each preceded by a `<!-- from … -->` comment.
You rewrite your assigned files in place, in the standards voice.

## Read first, in full

1. `/home/felix/.workhorse/repos/.claude/skills/spec-voice/style.md`: the rules. Follow every one.
2. `.workhorse/specs/overview.md` and `.workhorse/specs/runtime/reconciliation.md`: the signed-off pilot. Match their shape and voice.
3. `.workhorse/plans/c6/plan.md`, sections "Decisions" and "Conventions settled in the pilot".
4. `AGENTS.md`: the "what, not how" rule and the failure-mode rules, which several specs encode.

## Shape of each rewritten file

- Frontmatter `id` unchanged. H1 may be improved (sentence case, noun phrase).
- Directly under the H1, one sentence: "Applies to …".
- `## Borrowed terms` table (`| term | meaning |`, citing the standard) only where the spec uses terms defined by an external standard.
- `## Terminology` with one `###` per term only for terms this spec coins and owns. A term already in the root glossary (`../overview.md`, or `../../overview.md` from `runtime/tls/`) is linked there on first use, not redefined.
- `##` sections of prose requirements, one sentence per line, related sentences grouped into paragraphs. `###` only where something needs its own anchor.
- Field and message shapes in tables with a header row, followed by the normative sentences the table cannot carry.
- Non-normative material in `> [!NOTE]` blocks, never with an RFC 2119 keyword inside.
- Remove every `> x[rule.id]` marker and every `<!-- from … -->` comment. No blockquoted requirements remain.

## Voice and content

- BCP 14 keywords in capitals carry the force; every sentence with one names its actor ("The runtime MUST …", "A client SHOULD …", "A script MAY …").
- MUST for interoperability, security properties, and product commitments (including the failure-mode rules in `AGENTS.md`, which are commitments). Otherwise SHOULD or MAY.
- Contracts (wire shapes, CLI-visible names, file paths, constants, defaults, error codes, BSL signatures) carry enough to build a compatible implementation. Take values from the code under `crates/`, never from memory.
- Cut: point-in-time language, absences, self-description ("this spec…", "absent specification bugs…"), restatements of other specs, rationale that does not constrain a re-implementer, "what" stand-in nouns, em-dashes. Keep a rationale only as a note, and only where cutting it lets someone build a plausible wrong variant.
- Implementation detail out (tool names like `podman`, `caddy`, `nft`, crate names, internal types, SQL). Interface contracts stay.
- BSL signatures and method names are the language's contract and stay exactly as the code accepts them; check the code where the spec and code might disagree.
- Australian/NZ English. No double spaces after punctuation.

## Terminology decisions

- **app**, not "application" or "Seedling Application" (except in product names already fixed in code-facing contracts).
- **host**, not "node", "instance" or "machine", for the machine one daemon runs on. Keep "node" only where it is a wire or BSL name (e.g. a constant called `NODE_NAME`), and say what it is.
- **runtime**, not "control plane".
- **resource graph** for the evaluated definition (formerly "AppDef"); **definition bundle** for the files.
- **operator**, **Operator Interface** (OI), **desired state**, **lifecycle state**, **lifecycle operation**, **generation**, **fault**, **resource** as in the root glossary.

## Links

- Within your spec: `#heading-slug` (GitHub slug rules: lowercase, spaces to hyphens, punctuation dropped).
- To a root glossary term: `../overview.md#term-slug` (adjust depth).
- To a rule in any other spec: keep or write the placeholder `](rule:<prefix>/<rule.id>)`, where prefix is `l` language, `r` runtime, `i` interface, `w` web. Do not guess other specs' slugs; the final sweep resolves placeholders. To another spec as a whole, a relative link to its file is fine.
- A cross-reference names the subject of its target; it does not summarise what the target says.
- Leave existing `rule:` placeholders that point into your own spec converted to real `#slug` links.

## Rule map (required)

For each file, write `.workhorse/plans/c6/rule-map/<ID>.tsv`: one line per rule id that appeared in the split file, `<prefix>/<rule.id>\t<ID>#<slug>` (or `<ID>` alone when the rule maps to the whole spec).
Every rule id that was in the file must appear, including ones whose text you cut (map them to the section the implementing code still serves).
Code annotations will be rewritten from this map to `// spec: ID#slug`, so the slug must be a heading that exists in your final file.

## Boundaries

- Edit only your assigned spec files and their rule-map files. Do not edit other specs, code, or the root overview.
- Do not run git commands that write (no add, commit, stash, checkout). Reading with `git log`/`git show` is fine.
- Content that belongs in a different spec: leave it out of yours only if you are sure the other spec's rewriter has it; otherwise keep it and report the suggested move.

## Report (your final message)

Per file, concisely:
1. A sentence or two on what changed.
2. Gaps the rewrite exposed and resolved against the code, each with the file and line it rests on.
3. Gaps still open, needing a decision: each a one-line question with the options.
4. Standards cited (name, number, URL) for the root external documents table.
5. Terms you think belong in the root glossary (used across specs), with a one-sentence gloss.
6. Content you think belongs in another spec, with the rule id and target.
7. Places where the spec now leads or contradicts the code.
