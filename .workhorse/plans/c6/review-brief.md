# Brief: fresh-eyes spec review

Repository root: `/home/felix/.workhorse/repos/beyondessential/seedling/worktrees/c6`.
The specs under `.workhorse/specs/` are written in the standards voice.
Read first, in full: `/home/felix/.workhorse/repos/.claude/skills/spec-voice/style.md` (the rules, and the checklist in `/home/felix/.workhorse/repos/.claude/skills/spec-voice/SKILL.md` section 6), `.workhorse/rules.md`, `.workhorse/specs/overview.md` (the glossary), and `.workhorse/plans/c6/gaps.md` (every "Decided" section: settled, never reverted).

Review your assigned specs as someone reading them for the first time, who must build a compatible implementation from them alone.

## Look for

- **Contradictions**: two sentences, here or in another spec, that cannot both hold. Grep the other specs for every term, method, fault kind and error code your specs use.
- **Gaps**: a behaviour the spec starts but does not finish (an error case with no outcome, a state with no exit, a value with no type or range, a "could not determine" path that is unspecified). Read the code under `crates/` to learn what the system does, but the code is not authority.
- **Ambiguity**: a sentence a careful implementer could read two ways.
- **Information architecture**: content a reader would look for in another spec.
- **Voice and style**: every item of the spec-voice checklist. That covers:
  - one sentence per line;
  - BCP 14 keywords with a named actor;
  - no keyword inside a note;
  - no em-dashes;
  - no point-in-time language, absences, self-description, or "what" stand-ins;
  - cross-references that name their subject rather than recite it;
  - borrowed terms glossaried;
  - Australian/NZ English;
  - implementation detail kept out (tool names, internal types, SQL), interface contracts kept in.
- **Dead or wrong links**: a link whose target does not say what the sentence implies.

## What you do

- Fix voice, style, link and plain wording problems in your assigned files directly, without changing what any requirement means.
- Report contradictions, gaps, ambiguities and information-architecture problems instead of fixing them. Each report states the sentence(s), the file and line, the problem, and your recommended fix in one or two sentences. Give a severity: **critical** (an implementer would build something wrong or incompatible), **suggestion** (worth fixing), or **nit**.
- Do not rename or remove `#` headings. Do not edit code or files outside your assignment.
- Run `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh` and `lychee --offline --include-fragments=anchor-only --no-progress '.workhorse/specs/**/*.md'` when done. Other agents are editing other spec files at the same time, so ignore failures outside your files.

## What you return

Two lists, nothing else: **Fixed** (one line each) and **Findings** (as above, most severe first).
