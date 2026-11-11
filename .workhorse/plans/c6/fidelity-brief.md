# Brief: fidelity pass against the original specs

Repository root: `/home/felix/.workhorse/repos/beyondessential/seedling/worktrees/c6`.

The specs under `.workhorse/specs/` were rewritten from tracey files into the standards voice.
The rewrite was meant to change the voice, not the requirements.
In places it changed the requirements: it softened a definite rule to SHOULD, tightened a rule beyond the original (a restriction applied to more cases than the original named), dropped a requirement, invented a constraint or hedge the original never had, or swapped an original's general rule for a narrower reading taken from the code.
Your job is to find every such drift in your assigned spec files and restore the original's meaning, in the new voice.

## Sources

- The originals: `git show 96eb265f^:docs/spec/<area>.md` for `language`, `runtime`, `interface` and `web`. Rules are blockquotes headed `> r[rule.id]` (prefix `l` language, `r` runtime, `i` interface, `w` web in links).
- The rule map: `.workhorse/plans/c6/rule-map/<ID>.tsv`, one line per original rule, `<prefix>/<rule.id>\t<ID>#<slug>`. Grep every TSV for your spec's id to find every original rule that landed in it, including rules that moved in from other areas.
- Intentional changes, which you MUST NOT revert: `.workhorse/plans/c6/gaps.md`, sections "Decided", "Spec leads or contradicts code" and "Platform hedges", and `.workhorse/specs/platform/linux.md`. Decisions made with the user after the rewrite are listed in "Decided"; read it in full first.
- The house style: `/home/felix/.workhorse/repos/.claude/skills/spec-voice/style.md` and `.workhorse/rules.md`. The style permits cutting point-in-time language, restatements of other specs, explanation that does not constrain an implementer, and implementation detail (tool names, internal types). Those cuts are not drift.
- The code under `crates/` decides nothing here: where the original and the code disagree, the original wins unless gaps.md records otherwise. Read code only to understand what an original rule meant.

## What counts as drift

For each original rule, compare its meaning with the new text and classify:

- **softened**: a definite original ("the runtime tears down…", "must") became SHOULD or MAY, or gained a hedge ("where possible", "SHOULD wait"). Platform hedges listed in gaps.md and resolved in LINUX are intentional; other new hedges are drift.
- **tightened**: the new text forbids or requires more than the original (a restriction widened to more cases, a range narrowed, a value pinned that the original left open, an "only" added).
- **loosened**: the new text permits more than the original, or drops a condition.
- **dropped**: an original requirement with no counterpart anywhere in the specs (grep the other specs before calling it dropped; it may have moved).
- **invented**: a requirement in the new text with no original and no entry in gaps.md. Check the rule map and gaps.md before calling it invented.
- **changed**: the meaning differs in some other way (a different default, a different error code, a different actor).

Keyword strength: the originals often wrote plain declaratives ("X is Y", "the runtime does Z") for requirements. A plain declarative about behaviour is a MUST unless the original says "should", "may", "can", or gives it as an example.

## What you do

- Restore unambiguous drift by editing your assigned files in place, in the voice: one sentence per line, BCP 14 keywords with a named actor, notes in `> [!NOTE]` with no keyword, no em-dashes, Australian/NZ English.
- Do not rename, remove or add `#` headings: they are citation anchors. If a fix needs a heading change, report it instead.
- Do not edit any file outside your assignment, any code, or gaps.md. If a drift's fix belongs in another spec, report it.
- Report instead of editing when: the fix would revert or conflict with anything in gaps.md or LINUX; the original is ambiguous; the original contradicts another original rule; or restoring would change behaviour the code relies on in a way that looks deliberate (say what and where).
- Run `etc/ci/check-spec-style.sh`, `etc/ci/check-spec-citations.sh` and `lychee --offline --include-fragments=anchor-only --no-progress '.workhorse/specs/**/*.md'` when done; fix anything your edits broke. Other agents are editing other spec files at the same time, so ignore failures in files outside your assignment.

## What you return

Two lists, nothing else:

1. **Restored**: one line per edit, `ID#slug: <class>: <what the original required, and what the rewrite had>` (original rule id in brackets).
2. **For the user**: one entry per item you did not fix, with the original text, the new text, the file and line, why you did not fix it, and the fix you recommend.

Be thorough: go through every original rule mapped to your specs, not a sample.
