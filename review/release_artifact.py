#!/usr/bin/env python3
"""Check publication claims against the newest release tag (D-203/D-457).

The newest tag has three possible states: no public release, a release built from the earlier product layout,
or a release containing `core/src/v2`. Documents must not recommend a release while describing it as another
product generation. `STATEMENTS` records the retired caveats that were required for the second state and are
forbidden for the third; `NO_RELEASE_STATEMENTS` covers the first.

    python3 review/release_artifact.py
    python3 review/release_artifact.py --tag v0.2.0
    python3 review/release_artifact.py --layout core/src   # control: a layout every tag carries
    python3 review/release_artifact.py --doc README.md=/tmp/readme-copy.md

D-216 also checks that all three crates carry the same version. The release workflow reads that version and
refuses a mismatched tag before building an archive.

The check is local: it reads tags in this clone, not GitHub's current release page. A clone without tags gets a
note rather than a network-backed verdict. Statement matching is intentionally narrow; update the statements
and their tests together when the publication state changes.
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

# D-457: no tag at all is a third state, and the correct statement is that no public release exists.
NO_RELEASE_STATEMENTS = [
    ("docs/INSTALL.md", "no public release", "§1's note"),
    ("docs/ACCEPTANCE.md", "no public release", "the known-gap entry"),
    ("README.md", "no public release", "the install section"),
    ("README.zh-CN.md", "GitHub release", "the mirrored install note (an English token: code here is English only)"),
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


def check_no_release(documents: dict) -> list:
    """Findings: while no release exists, every document must say so."""
    out = []
    for path, phrase, where in NO_RELEASE_STATEMENTS:
        if normalize(phrase) not in normalize(documents[path]):
            out.append(f"{path}: the statement that no public release exists is missing — {where}")
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
        print("no release tag exists: no public release is published (D-457)")
        findings = check_no_release(documents)
    else:
        state = "is" if released else "is not"
        print(f"the newest release tag `{tag}` {state} built on {args.layout}: the published artifact "
              f"{'is' if released else 'is not'} the product the documents describe")
        findings = check(released, documents)
    crate_versions = {}
    for crate in ("engine", "core", "tui"):
        manifest = REPO / crate / "Cargo.toml"
        found = re.search(r'^version = "([^"]+)"', manifest.read_text(encoding="utf-8"), re.M)
        crate_versions[crate] = found.group(1) if found else ""
    if len(set(crate_versions.values())) != 1:
        findings.append("the crates disagree about the version the release would carry: "
                        + ", ".join(f"{crate} {version}" for crate, version in crate_versions.items())
                        + " — the release workflow refuses that tag before it builds anything")
    for finding in findings:
        print("FAIL:", finding)
    if released and not findings:
        print("  current-layout release confirmed; retired release caveats are absent")
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
