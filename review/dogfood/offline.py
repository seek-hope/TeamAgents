#!/usr/bin/env python3
"""The dogfood probes that need no model and no credentials, in one command (D-138)

The probes in this directory drive the real product — the built CLI, a real daemon, a real terminal, a real
socket — and the ones listed here do it without a model and without a credential, so anyone can re-run them:

    python3 review/dogfood/offline.py             # run the whole set
    python3 review/dogfood/offline.py --list      # what it runs, and why each is in the set
    python3 review/dogfood/offline.py --only boundary.py

It needs the built binaries (`make build`; the Makefile target depends on it) and nothing else. A probe keeps
its scratch under `TMPDIR` and removes it at exit; pass `--state-dir` to a probe that fails to keep its state
for inspection. This harness closes that loop for the whole set: it counts the scratch directories and the
daemons before and after, and fails if the run added either — a probe that forgets one is a leak the machine
pays for (D-111 for daemons, D-131 for scratch).

Ceiling: it runs the credential-free subset only. The probes that need a model are listed in
`review/dogfood/README.md` and run one at a time, with each model's native window (D-36).
"""
import argparse
import os
import pathlib
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
HERE = REPO / "review/dogfood"

# (file, extra arguments, why it is in this set)
PROBES = [
    ("boundary.py", [], "the state roots the CLI refuses (A33/A34)"),
    ("budget.py", [], "a ceiling below one request is refused before any model call (A18)"),
    ("truncation.py", [], "A19's whole path over a real socket: truncated before output retries, after it fails"),
    ("input_latency.py", [], "per-keystroke composer latency against the scripted daemon"),
    ("tui_panels.py", [], "the instances panel's keys against a real daemon (D-95)"),
    ("tui_reconnect.py", [], "the TUI through a daemon kill and restart (A28, D-99)"),
    ("providers.py", ["--self-check"], "the A27 probe's task-result rule, without a model"),
]
TIMEOUT = 300


def daemons() -> int:
    """Live session daemons, by the subcommand (D-111's predicate, not the binary's name)."""
    listing = subprocess.run(["ps", "-eo", "comm,args"], capture_output=True, text=True).stdout
    return sum(1 for line in listing.splitlines()[1:]
               if len(parts := line.split(None, 2)) == 3 and parts[0] == "teamagents"
               and parts[2].startswith("daemon "))


def strays() -> set:
    """Scratch directories the probes would leave behind, by their shared prefix."""
    return {p.name for p in pathlib.Path(os.environ.get("TMPDIR", "/tmp")).glob("ta-*")}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="print the probes in the set and exit")
    parser.add_argument("--only", action="append", help="run one probe by file name (repeatable)")
    args = parser.parse_args()

    chosen = [p for p in PROBES if not args.only or p[0] in args.only]
    if args.list:
        for name, extra, why in PROBES:
            print(f"  {name:20s} {' '.join(extra):14s} {why}")
        return 0
    if not chosen:
        print(f"no probe matches {args.only}")
        return 1
    missing = [name for name, _extra, _why in chosen if not (HERE / name).is_file()]
    if missing:
        print(f"missing probe file(s): {missing}")
        return 1

    before_daemons, before_strays = daemons(), strays()
    failures, started_all = [], time.time()
    for name, extra, why in chosen:
        started = time.time()
        completed = subprocess.run([sys.executable, str(HERE / name), *extra],
                                   capture_output=True, text=True, timeout=TIMEOUT)
        elapsed = round(time.time() - started, 1)
        status = "ok  " if completed.returncode == 0 else "FAIL"
        print(f"{status} {name:20s} {elapsed:5.1f}s  {why}")
        if completed.returncode != 0:
            failures.append(name)
            for line in completed.stdout.splitlines()[-6:]:
                print(f"       {line}")
            for line in completed.stderr.splitlines()[-3:]:
                print(f"       {line}")
    total = round(time.time() - started_all, 1)

    new_daemons, new_strays = daemons(), strays() - before_strays
    leaks = []
    if new_daemons > before_daemons:
        leaks.append(f"{new_daemons - before_daemons} daemon(s) left running")
    if new_strays:
        leaks.append(f"scratch left behind: {sorted(new_strays)}")
    print(f"{len(chosen)} probes in {total}s; daemons {before_daemons} -> {new_daemons}; "
          f"new scratch {sorted(strays() - before_strays) or 'none'}")
    for leak in leaks:
        print("FAIL:", leak)
    return 1 if (failures or leaks) else 0


if __name__ == "__main__":
    sys.exit(main())
