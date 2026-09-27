#!/usr/bin/env python3
"""Scripts the build and CI run, but the repository does not carry (the detector behind D-179/D-196).

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

**D-196 added the Makefile's own two lists.** The default goal is `help`, so a bare `make` prints it: it is the
first thing a contributor reads, and it had drifted — twenty targets, nine named, with the whole `verify-*`
family missing and six of those targets absent from `.PHONY` as well (they are recipes, not files, so a file
named `verify-kani` would have made `make verify-kani` say "up to date"). Every `.PHONY` target except `help`
itself must be named by a `help` echo line, and every name the help prints must be a `.PHONY` target; a
Makefile surface with no `.PHONY` is a finding, because then neither half can be checked and a target can be
shadowed by a file.

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
PHONY = re.compile(r"^\.PHONY:(.*)$", re.M)
ECHO = re.compile(r"@echo '([^']*)'")
MAKE_NAME = re.compile(r"make ([a-z][a-z0-9-]*)")


def surfaces() -> list:
    """The files that execute scripts: the Makefile and the workflows, in a stable order."""
    return ["Makefile"] + sorted(
        str(path.relative_to(REPO)) for path in (REPO / ".github" / "workflows").glob("*.yml")
    )


def tracked() -> set:
    out = subprocess.run(["git", "ls-files"], cwd=REPO, capture_output=True, text=True, check=True).stdout
    return {line for line in out.split("\n") if line}


def phony_targets(text: str) -> list[str]:
    """The public targets a Makefile declares, in file order, continuation lines joined.

    A `.PHONY` line may end in `\\` and continue: reading only the first line made the continuation marker a
    phantom target and hid every name after it (measured on the first run of D-196's check, which listed `\\` as a
    target the help did not name and the verify family as names without targets).
    """
    match = PHONY.search(text)
    if not match:
        return []
    parts = [match.group(1)]
    rest = text[match.end():].split("\n")[1:]
    for line in rest:
        if not parts[-1].rstrip().endswith("\\"):
            break
        parts.append(line)
    return " ".join(parts).replace("\\", " ").split()


def help_names(text: str) -> set[str]:
    """Every `make <target>` name the `help` target prints."""
    names: set[str] = set()
    for line in ECHO.findall(text):
        names |= set(MAKE_NAME.findall(line))
    return names


def check_makefile_help(name: str, text: str, findings: list) -> int:
    """Every declared target is in the help and every help name is a target. Returns the pairs checked."""
    declared = phony_targets(text)
    if not declared:
        findings.append(f"{name}: declares no `.PHONY` targets — the help text cannot be checked against them, "
                        "and a file with a target's name would shadow it")
        return 0
    printed = help_names(text)
    for target in declared:
        if target != "help" and target not in printed:
            findings.append(f"{name}: `{target}` is a target `.PHONY` declares and `make help` (the default "
                            "goal) does not name — a contributor reading it cannot see the target")
    for target in sorted(printed - set(declared)):
        findings.append(f"{name}: `make help` names `{target}`, which is not a `.PHONY` target (a file of that "
                        "name would shadow it)")
    return len(declared)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--surface", action="append", help="a surface to read instead of the default ones")
    args = parser.parse_args(argv)
    read = args.surface or surfaces()
    index = tracked()
    findings, checked, targets = [], 0, 0
    for name in read:
        path = REPO / name
        if not path.is_file():
            findings.append(f"{name}: no such surface to read")
            continue
        text = path.read_text(errors="replace")
        if "Makefile" in path.name:
            targets += check_makefile_help(name, text, findings)
        for line_number, line in enumerate(text.split("\n"), 1):
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
    if targets:
        print(f"the Makefile's {targets} `.PHONY` target(s) and its help text name each other")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
