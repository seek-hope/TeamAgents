#!/usr/bin/env python3
"""Check documentation ownership, links and entry-point reachability.

Every tracked Markdown document has one declaring map and a reader role, except
explicit entry points, the Chinese translation and workflow release notes.
Maps must link their subjects, use a supported role and name an existing document.
Use --list to inspect ownership and --self-check for synthetic broken-map controls.
This validates navigation structure, not prose quality or semantic completeness.
"""
import argparse
import collections
import pathlib
import re
import subprocess
import sys
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[1]

# The five places a reader starts. A document mapped by one of these is reachable; the entry points themselves
# need no listing because nothing introduces them.
ROOTS = ["README.md", "AGENTS.md", "docs/DEVELOPMENT.md", "verification/README.md", "tools/README.md"]
ROLES = ["user", "developer", "agent", "evidence"]

# The documented language exception (AGENTS.md): the Chinese README is a translation held to README.md by
# `tools/readme_zh.py`; it is the only content with a language exemption.
EXCLUDED_FILES = {"README.zh-CN.md": "the Chinese README, a translation of README.md (tools/readme_zh.py)"}
EXCLUDED_PREFIXES = {}

# A tracked file that is not a document for a reader. The listing convention does not apply to it; the reason
# is stated here rather than left implicit, the way `dead_code.py` states its exemptions.
EXEMPT = {
    ".github/release-notes.md":
        "the release workflow's --notes-file body, read by .github/workflows/release.yml "
        "(tools/build_references.py checks the argument); a build input, not a document for a reader",
}

MAP_LINE = re.compile(r"^<!--\s*maps:\s+(\S+)\s+reader=(\w+)\s*-->$")
LINK = re.compile(r"\[[^\]]*\]\(\s*<?([^)\s>]+)>?[^)]*\)")


def tracked_markdown() -> list:
    """Every tracked markdown path, minus the documented exceptions, as repository-relative strings."""
    out = subprocess.run(["git", "ls-files", "*.md"], cwd=REPO, capture_output=True, text=True, check=True)
    kept = []
    for rel in out.stdout.split():
        if rel in EXCLUDED_FILES or any(rel.startswith(p) for p in EXCLUDED_PREFIXES):
            continue
        kept.append(rel)
    return sorted(kept)


def links(text: str, source: str, tracked: set) -> set:
    """The tracked documents `text` links relatively, resolved against `source`'s own directory."""
    found = set()
    for raw in LINK.findall(text):
        target = raw.split("#", 1)[0]
        if not target or "://" in target or target.startswith("mailto:"):
            continue
        resolved = (pathlib.Path(source).parent / target).resolve()
        try:
            rel = resolved.relative_to(REPO).as_posix()
        except ValueError:
            continue
        if rel in tracked:
            found.add(rel)
    return found


def analyse(root: pathlib.Path, tracked: list) -> tuple:
    """The findings and the listings. `root` is the tree root, `tracked` its documents (relative paths)."""
    tracked = sorted(tracked)
    known = set(tracked)
    texts = {rel: (root / rel).read_text(errors="ignore") for rel in tracked if (root / rel).is_file()}

    # the link graph: BFS from the entry points, so what a reader following the documents actually reaches.
    # Each document's links resolve against *its own* directory (the source is passed through), which is the
    # difference between `docs/CONFIG.md` as written in `docs/USER-GUIDE.md` and a file of that name at the root.
    reachable = set(r for r in ROOTS if r in known)
    queue = list(reachable)
    while queue:
        source = queue.pop()
        for target in links(texts.get(source, ""), source, known):
            if target in texts and target not in reachable:
                reachable.add(target)
                queue.append(target)

    listings: dict = collections.defaultdict(list)   # subject -> [(map, reader, line)]
    for rel, text in texts.items():
        for number, line in enumerate(text.splitlines(), 1):
            match = MAP_LINE.match(line.strip())
            if match:
                listings[match.group(1)].append((rel, match.group(2), number))

    findings = []
    for subject, entries in sorted(listings.items()):
        for rel, role, number in sorted(entries):
            where = f"{rel}:{number}"
            if subject not in known:
                findings.append(("unknown", f"{where} lists {subject}, which is not a tracked document"))
                continue
            if role not in ROLES:
                findings.append(("reader",
                                 f"{where} declares reader={role} for {subject}: a reader must be one of "
                                 f"{', '.join(ROLES)}"))
            if subject not in links(texts.get(rel, ""), rel, known):
                findings.append(("unlinked", f"{where} maps {subject} but does not link it"))
    for rel in tracked:
        if rel in ROOTS or rel in EXEMPT or rel not in texts:
            continue
        entries = listings.get(rel, [])
        if not entries:
            if rel in reachable:
                findings.append(("unlisted", f"{rel} is reachable but no map lists it"))
            else:
                findings.append(("orphan", f"{rel} is reachable from no entry point"))
        elif len(entries) > 1:
            where = ", ".join(f"{m} (reader={r})" for m, r, _n in sorted(entries))
            findings.append(("two maps", f"{rel} is listed by {len(entries)} documents: {where}"))
    return findings, listings


def report(root: pathlib.Path, tracked: list, findings: list, listings: dict) -> None:
    roles = collections.Counter(role for entries in listings.values() for _m, role, _n in entries)
    lines = sum(len((root / rel).read_text(errors="ignore").splitlines()) for rel in tracked
                if (root / rel).is_file())
    print(f"{len(tracked)} documents tracked, {lines} lines; {len(ROOTS)} entry points, "
          f"{len(EXEMPT) + len(EXCLUDED_FILES)} exempt (excluding {len(EXCLUDED_PREFIXES)} frozen prefixes); "
          f"readers: " + ", ".join(f"{role} {roles[role]}" for role in ROLES))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="print every document with its reader and its map")
    parser.add_argument("--self-check", action="store_true",
                        help="run the rule over a synthetic tree, including its controls")
    args = parser.parse_args()
    if args.self_check:
        return self_check()

    tracked = tracked_markdown()
    findings, listings = analyse(REPO, tracked)
    report(REPO, tracked, findings, listings)
    if args.list:
        owners = {}
        for subject, entries in listings.items():
            for rel, role, _number in entries:
                owners[subject] = (role, rel)
        for rel in tracked:
            if rel in EXEMPT:
                print(f"  exempt     —          {rel}")
            elif rel in ROOTS:
                print(f"  entry      —          {rel}")
            else:
                role, owner = owners.get(rel, ("?", "?"))
                print(f"  {role:9s}  {owner:22s} {rel}")
    for kind, message in findings:
        print(f"  {kind}: {message}")
    return 1 if findings else 0


def self_check() -> int:
    """The rule over synthetic trees: the clean case passes and each refusal fires (D-443)."""
    base = {
        "README.md": "# R\n<!-- maps: a.md reader=user -->\n[a](a.md)\n",
        "AGENTS.md": "# A\n<!-- maps: b.md reader=developer -->\n[b](b.md)\n",
        "docs/DEVELOPMENT.md": "# D\n",
        "verification/README.md": "# V\n",
        "tools/README.md": "# W\n",
        "a.md": "# a\n",
        "b.md": "# b\n",
    }
    cases = [
        ("clean", base, []),
        ("orphan (reachable from no entry point)",
         {**base, "c.md": "# c\n"}, ["orphan"]),
        ("listing without a valid reader",
         {**base, "README.md": "# R\n<!-- maps: a.md reader=audience -->\n[a](a.md)\n"}, ["reader must be one of"]),
        ("listed by two maps",
         {**base, "AGENTS.md": "# A\n<!-- maps: b.md reader=developer -->\n<!-- maps: a.md reader=user -->\n"
                              "[b](b.md)\n[a](a.md)\n"}, ["listed by 2"]),
        ("a listing whose map does not link it",
         {**base, "README.md": "# R\n<!-- maps: a.md reader=user -->\n"}, ["does not link"]),
        ("a listing for a path that is not a document",
         {**base, "README.md": "# R\n<!-- maps: a.md reader=user -->\n<!-- maps: ghost.md reader=user -->\n"
                              "[a](a.md)\n"}, ["is not a tracked document"]),
    ]
    ok = True
    for name, files, expected in cases:
        with tempfile.TemporaryDirectory(prefix="teamagents-doc-inventory-") as scratch:
            root = pathlib.Path(scratch)
            for rel, text in files.items():
                path = root / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
            findings, _listings = analyse(root, sorted(files))
            if expected:
                hit = all(any(needle in f"{kind}: {message}" for kind, message in findings)
                          for needle in expected)
            else:
                hit = not findings
            detail = "; ".join(f"{kind}: {message}" for kind, message in findings) or "no findings"
            print(f"  self-check {name}: {'ok' if hit else 'FAILED'} — {detail}")
            ok = ok and hit
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
