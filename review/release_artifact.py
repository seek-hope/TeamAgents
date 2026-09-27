#!/usr/bin/env python3
"""One fact, checked against every document that states it: is the published release this product? (D-203)

`docs/INSTALL.md` §1 tells a reader that the latest published release is the earlier implementation, and
`review/install_check.py` shows it live: the artifact's help still offers `validate`, `sessions prune`,
`--plain`, `--resume` and `--team`, verbs D-52/D-73 removed. The fact needs no network here — every release tag
in this repository predates the v2 layout (`git ls-tree <tag> -- core/src/v2` is empty, while the current tree
is built on it), so whatever the newest tag published is not the product the documents describe.

    python3 review/release_artifact.py

The fact is computed from the tags, and the documents have to agree with it:

* while no release tag carries the layout, each statement below must be **present**;
* publishing a v2 release flips it — the newest tag then carries the layout, so each statement must be
  **absent** — and whoever cuts that release meets a failing audit until the statements and this list are
  rewritten together. That is the point: one fact with several statements, not several opinions (the shape
  D-133 established for the project config).

The install guide and the acceptance ledger state it in prose; the README is where the install is *recommended*,
so it must carry the caveat too, and the Chinese README with it. The Chinese note cannot be matched by a
Chinese phrase — this repository's code is English only — so it is held by the tag it must name (`v0.1.2`),
the one token only the caveat mentions in either README.

    python3 review/release_artifact.py --doc README.md=/tmp/readme-without-the-note.md   # the control
    python3 review/release_artifact.py --layout core/src   # the other side: a layout every tag's tree has

Ceiling: the fact is read from the tags in this clone, so a checkout that never fetched them cannot compute it
(a note, not a finding: the statements are still what the documents must say, and the failure is in the lenient
direction); the statements are matched as text, so a document that states the caveat in new words is invisible;
and the Chinese marker is a version token rather than a phrase, a weaker contract, stated here on purpose.
"""
import argparse
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
LAYOUT = "core/src/v2"
# (document, the phrase that must be present while the published release is not this product, where it lives)
STATEMENTS = [
    ("docs/INSTALL.md", "earlier implementation", "§1's note"),
    ("docs/ACCEPTANCE.md", "the published release is the earlier implementation", "the known-gap entry"),
    ("README.md", "earlier implementation", "the install section"),
    ("README.zh-CN.md", "v0.1.2", "the mirrored install note (a version token: code here is English only)"),
]


def git(*args: str) -> tuple[int, str]:
    """`(exit code, stdout)` for a git command in this repository."""
    done = subprocess.run(["git", *args], cwd=REPO, capture_output=True, text=True)
    return done.returncode, done.stdout.strip()


def normalize(text: str) -> str:
    """Markdown emphasis, line wrapping and a sentence's first letter must not hide a claim (D-133)."""
    return re.sub(r"\s+", " ", text.replace("*", " ")).strip().casefold()


def newest_tag() -> str:
    """The release this repository's newest tag names, or `""` when the clone carries no tag."""
    code, out = git("tag", "--sort=-v:refname")
    return out.split("\n")[0] if code == 0 and out else ""


def release_carries(tag: str, layout: str) -> bool:
    """Does the tag's tree contain `layout`, i.e. is the published release this product generation?"""
    if not tag:
        return False
    code, out = git("ls-tree", "-r", "--name-only", tag, "--", layout)
    return code == 0 and bool(out)


def check(released: bool, documents: dict) -> list:
    """Findings: a statement missing while the release is not this product, or left once it is."""
    out = []
    for path, phrase, where in STATEMENTS:
        present = normalize(phrase) in normalize(documents[path])
        if present == released:  # must be present while stale, absent once the release is this product
            verb = "still present although the newest tag carries the layout" if released else "missing"
            out.append(f"{path}: the statement that the published release is not this product is {verb} — {where}")
    return out


def main(argv) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", help="the release tag to read instead of the newest (the control)")
    parser.add_argument("--layout", default=LAYOUT,
                        help="the directory that tells the two product generations apart; a layout every tag "
                             "carries flips the fact (the control)")
    parser.add_argument("--doc", action="append", default=[], metavar="PATH=FILE",
                        help="read a document from FILE instead of the tree (the control)")
    args = parser.parse_args(argv)
    tag = args.tag or newest_tag()
    released = release_carries(tag, args.layout)
    overrides = dict(item.split("=", 1) for item in args.doc)
    documents = {path: pathlib.Path(overrides.get(path, REPO / path)).read_text(errors="replace")
                 for path, _, _ in STATEMENTS}
    if not tag:
        print("note: this clone carries no release tag, so whether the published artifact is this product "
              "cannot be computed; the statements below are read as the stale case")
    state = "is" if released else "is not"
    print(f"the newest release tag `{tag or '(none)'}` {state} built on {args.layout}: the published artifact "
          f"{'is' if released else 'is not'} the product the documents describe")
    findings = check(released, documents)
    for finding in findings:
        print("FAIL:", finding)
    if released and not findings:
        print("  next: the release is this product now, so the caveats and this script's list must be rewritten "
              "together")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
