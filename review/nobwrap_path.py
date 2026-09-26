#!/usr/bin/env python3
"""Build a PATH without a usable bubblewrap, so the sandbox-less conditions are reproducible locally (D-113/D-114).

Two conditions matter and neither is visible on a dev machine whose sandbox works:

* **No bwrap at all** — the GitHub runner (its image ships no `bubblewrap`, verified against
  `actions/runner-images` on 2026-09-26). `make check-nobwrap` runs the whole gate this way.
* **bwrap installed but unable to create a namespace** — the default on Ubuntu 23.10+/24.04, where AppArmor
  restricts unprivileged user namespaces and `bwrap` dies with `bwrap: setting up uid map: Permission denied`.
  The binary is on `PATH`, so `bwrap_available()` is true and `doctor` has to *probe* to notice; the tests must
  still assert the fail-closed half instead of failing on an assertion that assumes a working sandbox.
  `--stub-bwrap` reproduces it with a stub that prints that line and exits non-zero, so `make check-broken-sandbox`
  runs the gate as such a machine would.

    python3 review/nobwrap_path.py <dir>                    # fill <dir>: every tool except bwrap
    python3 review/nobwrap_path.py <dir> --stub-bwrap       # ... with a bwrap stub that cannot start a sandbox
    python3 review/nobwrap_path.py <dir> [--stub-bwrap] --verify   # assert what the farm promises
"""
import os
import pathlib
import subprocess
import sys

STUB = """#!/bin/sh
echo "bwrap: setting up uid map: Permission denied" >&2
exit 1
"""


def build(farm, stub):
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
    if stub:
        stub_path = farm / "bwrap"
        stub_path.write_text(STUB)
        stub_path.chmod(0o755)
    return linked


def verify(farm, stub):
    names = {entry.name for entry in farm.iterdir()}
    problems = []
    if "bwrap" in names and not stub:
        problems.append("the farm still offers bwrap")
    if stub:
        if "bwrap" not in names:
            problems.append("the stub farm has no bwrap for the product to find")
        else:
            probe = subprocess.run([str(farm / "bwrap")], capture_output=True, text=True)
            if probe.returncode == 0 or not probe.stderr.startswith("bwrap: "):
                problems.append(f"the stub does not look like a broken bwrap: {probe.stderr!r}")
    for needed in ("sh", "python3", "cargo", "make"):
        if needed not in names:
            problems.append(f"the farm is missing {needed}, so `make check` cannot run in it")
    return problems


def main(argv):
    args = argv[1:]
    stub = "--stub-bwrap" in args
    verify_only = "--verify" in args
    positional = [a for a in args if not a.startswith("--")]
    if not positional:
        print(__doc__)
        return 2
    farm = pathlib.Path(positional[0])
    linked = build(farm, stub)
    problems = verify(farm, stub)
    for problem in problems:
        print(problem)
    if problems:
        return 1
    what = "a bwrap that cannot start a sandbox" if stub else "no bwrap"
    print(f"{farm}: {linked} symlinks plus {what} (D-113/D-114)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
