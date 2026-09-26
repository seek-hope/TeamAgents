#!/usr/bin/env python3
"""Build a PATH that is this machine's PATH minus bwrap — the GitHub runner's condition, locally (D-113).

CI runs `make test` on a machine whose kernel forbids unprivileged user namespaces and which has no bubblewrap
installed, so the sandboxed shell cannot start there. That condition is invisible on a dev machine that has
bwrap, and it is the condition in which one test used to *fail* rather than skip. This script makes the
condition reproducible: it fills <dir> with symlinks to every executable of the current PATH except `bwrap`, so
`PATH=<dir> make check` runs exactly what the runner runs.

    python3 review/nobwrap_path.py <dir>            # fill <dir>
    python3 review/nobwrap_path.py --verify <dir>   # ... and assert the farm is what it claims
"""
import os
import pathlib
import sys


def build(farm):
    farm.mkdir(parents=True, exist_ok=True)
    linked = 0
    for directory in os.environ.get("PATH", "").split(":"):
        source = pathlib.Path(directory)
        if not source.is_dir():
            continue
        for entry in source.iterdir():
            if entry.name == "bwrap":
                continue
            target = farm / entry.name
            if target.exists() or target.is_symlink():
                continue
            try:
                target.symlink_to(entry)
                linked += 1
            except OSError:
                pass
    return linked


def verify(farm):
    names = {entry.name for entry in farm.iterdir()}
    problems = []
    if "bwrap" in names:
        problems.append("the farm still offers bwrap")
    for needed in ("sh", "python3", "cargo", "make"):
        if needed not in names:
            problems.append(f"the farm is missing {needed}, so `make check` cannot run in it")
    return problems


def main(argv):
    verify_only = len(argv) > 1 and argv[1] == "--verify"
    path = argv[2] if verify_only else (argv[1] if len(argv) > 1 else None)
    if path is None:
        print(__doc__)
        return 2
    farm = pathlib.Path(path)
    linked = build(farm)
    problems = verify(farm)
    for problem in problems:
        print(problem)
    if problems:
        return 1
    print(f"{farm}: {linked} symlinks, no bwrap (CI's condition, D-113)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
