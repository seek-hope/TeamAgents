#!/usr/bin/env python3
"""Scripts the build and CI run, but the repository does not carry (the detector behind D-179).

`make check` on a fresh clone fails the moment it reaches a file the repository does not have, and nothing
watched that. `git commit -a` stages modifications but **skips new files**, so a script a Makefile target calls
in the same commit stays untracked — twice in this repository's own history (D-170's `decision_citations.py`,
D-178's `test_counts.py`), each time caught only by reading `git status` after committing. The other failure
mode is the same check one step over: a path typo in a target, which no gate would notice either.

    python3 review/build_references.py

It reads the surfaces that *execute* scripts — the `Makefile` and `.github/workflows/*.yml` — takes every
`python3`/`sh`/`bash` invocation of a `.py`/`.sh` path, and requires each one to exist **and** to be tracked by
git. `--surface PATH` replaces the list, which is how the control is run (a surface copy naming a file that
exists but is untracked must produce exactly one finding).

Limits: only these two surfaces are read (a script named in a document is a citation, which `citations.py`
resolves); the invocation shape is textual, so a path built at runtime is invisible; and the check is about the
index, not about a commit — `git add`-ing and committing are still the author's job.
"""
import argparse
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[1]
# the command word must stand alone: without the lookbehind, the "sh" at the end of `install.sh` reads as an
# invocation of the next path on the line (`cp install.sh dist/install.sh` was the first run's false positive)
RUNS = re.compile(
    r"(?<![\w.-])(?:python3|python|sh|bash)(?:\s+-(?:u|n|e|ex))?\s+([A-Za-z0-9_][A-Za-z0-9_./-]*\.(?:py|sh))\b"
)


def surfaces() -> list:
    """The files that execute scripts: the Makefile and the workflows, in a stable order."""
    return ["Makefile"] + sorted(
        str(path.relative_to(REPO)) for path in (REPO / ".github" / "workflows").glob("*.yml")
    )


def tracked() -> set:
    out = subprocess.run(["git", "ls-files"], cwd=REPO, capture_output=True, text=True, check=True).stdout
    return {line for line in out.split("\n") if line}


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--surface", action="append", help="a surface to read instead of the default ones")
    args = parser.parse_args(argv)
    read = args.surface or surfaces()
    index = tracked()
    findings, checked = [], 0
    for name in read:
        path = REPO / name
        if not path.is_file():
            findings.append(f"{name}: no such surface to read")
            continue
        for line_number, line in enumerate(path.read_text(errors="replace").split("\n"), 1):
            for referenced in RUNS.findall(line):
                checked += 1
                if not (REPO / referenced).is_file():
                    findings.append(f"{name}:{line_number}: runs {referenced}, which does not exist")
                elif referenced not in index:
                    findings.append(
                        f"{name}:{line_number}: runs {referenced}, which is not tracked by git — a fresh clone "
                        "would fail there (`git commit -a` skips new files; D-170/D-178)"
                    )
    for finding in findings:
        print(f"FAIL: {finding}")
    if findings:
        return 1
    print(f"{checked} script reference(s) across {len(read)} surface(s): every one exists and is tracked")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
