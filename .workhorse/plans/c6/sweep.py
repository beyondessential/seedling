#!/usr/bin/env python3
"""Resolve tracey rule ids to `ID#slug` citations across specs, code and docs.

Reads rule-map/*.tsv beside this file. Run from the repository root.
  sweep.py check   report unmapped rules and unresolved placeholders, change nothing
  sweep.py apply   rewrite files
"""

import os
import re
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = Path.cwd()
SPECS = ROOT / ".workhorse/specs"

CODE_DIRS = ["crates", "etc"]
CODE_EXT = {".rs", ".ts", ".tsx", ".sh"}
SKIP_PARTS = {"target", "node_modules", "migrations", "dist"}
DOCS = [ROOT / "README.md", ROOT / "DEVELOP.md"] + sorted((ROOT / "docs").glob("*.md"))

TOKEN = r"[rliw]\[(?:(?:impl|verify) )?[a-z][a-z0-9-]*(?:\.[a-z0-9-]+)*\]"
ANNOT = re.compile(r"([rliw])\[(impl|verify) ([a-z][a-z0-9.\-]*)\]")
MENTION = re.compile(r"([rliw])\[([a-z][a-z0-9-]*(?:\.[a-z0-9-]+)*)\]")
PLACEHOLDER = re.compile(r"\]\(rule:([rliw])/([a-z0-9.\-]+)\)")
OLD_DOC_LINK = re.compile(r"(?:\.\./)*docs/spec/(language|runtime|interface|web)\.md#([rliw])--([a-z0-9.\-]+)")
PREFIX = {"language": "l", "runtime": "r", "interface": "i", "web": "w"}


def load_map():
    m = {}
    for tsv in sorted((HERE / "rule-map").glob("*.tsv")):
        for line in tsv.read_text().splitlines():
            if not line.strip():
                continue
            key, cite = line.split("\t")
            if key in m and m[key] != cite:
                sys.exit(f"{key} mapped twice: {m[key]} and {cite}")
            m[key] = cite
    return m


def spec_paths():
    ids = {}
    for p in SPECS.rglob("*.md"):
        mt = re.match(r"---\nid: ([A-Z0-9]+)\n---", p.read_text())
        if mt:
            ids[mt.group(1)] = p
    return ids


def cite_path(cite, ids, base_dir):
    sid, _, slug = cite.partition("#")
    rel = os.path.relpath(ids[sid], base_dir)
    return rel + (f"#{slug}" if slug else "")


def code_files():
    for d in CODE_DIRS:
        for p in (ROOT / d).rglob("*"):
            if p.is_file() and p.suffix in CODE_EXT and not SKIP_PARTS & set(p.parts):
                yield p


COMMENT_LINE = re.compile(r"^(\s*)(//[/!]?|#|\{/\*)\s*((?:" + TOKEN.replace("(?:(?:impl|verify) )?", "(?:impl|verify) ") + r"\s*)+)(.*)$")


def convert_code(text, rmap, missing):
    out = []
    for line in text.split("\n"):
        mt = COMMENT_LINE.match(line)
        if mt:
            indent, marker, toks, rest = mt.groups()
            cites = []
            for p, _, rid in ANNOT.findall(toks):
                c = rmap.get(f"{p}/{rid}")
                if c is None:
                    missing.add(f"{p}/{rid}")
                    c = f"{p}[{rid}]"
                if c not in cites:
                    cites.append(c)
            rest = rest.rstrip()
            sep = "" if not rest or rest.startswith("*/") else " "
            if marker == "{/*" and rest.startswith("*/"):
                sep = " "
            line = f"{indent}{marker} spec: {', '.join(cites)}{sep}{rest}"
        else:
            def ann(m):
                c = rmap.get(f"{m.group(1)}/{m.group(3)}")
                if c is None:
                    missing.add(f"{m.group(1)}/{m.group(3)}")
                    return m.group(0)
                return f"spec: {c}"

            line = ANNOT.sub(ann, line)

            def men(m):
                c = rmap.get(f"{m.group(1)}/{m.group(2)}")
                return c if c else m.group(0)

            line = MENTION.sub(men, line)
        if out and line.strip().startswith(("// spec:", "/// spec:", "{/* spec:")) and line == out[-1]:
            continue
        out.append(line)
    return "\n".join(out)


def convert_doc(path, text, rmap, ids):
    base = path.parent

    def old(m):
        c = rmap.get(f"{m.group(2)}/{m.group(3)}")
        if not c:
            return m.group(0)
        return f"[{c}]({cite_path(c, ids, base)})"

    text = re.sub("`" + OLD_DOC_LINK.pattern + "`", old, text)

    def men(m):
        c = rmap.get(f"{m.group(1)}/{m.group(2)}")
        if not c:
            return m.group(0)
        return f"[{c}]({cite_path(c, ids, base)})"

    text = re.sub(r"`(" + MENTION.pattern + r")`", lambda m: men(MENTION.match(m.group(1))), text)
    return text


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "check"
    rmap = load_map()
    ids = spec_paths()
    problems = []

    for p in SPECS.rglob("*.md"):
        text = p.read_text()
        for tok in re.findall(r"^> " + TOKEN + r"\s*$", text, re.M):
            problems.append(f"{p.relative_to(ROOT)}: rule marker left: {tok}")

        def res(m):
            c = rmap.get(f"{m.group(1)}/{m.group(2)}")
            if c is None:
                problems.append(f"{p.relative_to(ROOT)}: unmapped placeholder {m.group(1)}/{m.group(2)}")
                return m.group(0)
            return f"]({cite_path(c, ids, p.parent)})"

        new = PLACEHOLDER.sub(res, text)
        if mode == "apply" and new != text:
            p.write_text(new)

    missing = set()
    changed = 0
    for p in code_files():
        text = p.read_text()
        new = convert_code(text, rmap, missing)
        if new != text:
            changed += 1
            if mode == "apply":
                p.write_text(new)
    problems += [f"code: unmapped rule {k}" for k in sorted(missing)]

    for p in DOCS:
        if not p.exists():
            continue
        text = p.read_text()
        new = convert_doc(p, text, rmap, ids)
        if mode == "apply" and new != text:
            p.write_text(new)

    for line in problems:
        print(line)
    print(f"{len(rmap)} rules mapped, {changed} code files to change, {len(problems)} problems", file=sys.stderr)
    sys.exit(1 if problems else 0)


if __name__ == "__main__":
    main()
