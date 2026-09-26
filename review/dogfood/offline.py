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
import shutil
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


def stop_daemons(state_dir: pathlib.Path) -> None:
    """Stop whatever serves this probe's state root: a killed probe cannot do it itself."""
    for root in state_dir.rglob("root"):
        subprocess.run(["pkill", "-f", f"daemon --state-root {root}"], capture_output=True)


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
    # Each probe runs with an explicit --state-dir under this harness, so a probe that is killed cannot leave
    # anything behind (its own `atexit` cleanup does not run when the harness has to kill it) and a failing
    # probe's state is kept and named: that directory is the evidence, and D-140 is the case where it was gone
    # by the time anyone looked. The harness root is deliberately not named `ta-*`, so it cannot be mistaken
    # for a probe's own scratch by the guard below.
    harness_root = pathlib.Path(os.environ.get("TMPDIR", "/tmp")) / f"teamagents-probe-harness-{os.getpid()}"
    for name, extra, why in chosen:
        state_dir = harness_root / name.removesuffix(".py")
        started = time.time()
        try:
            completed = subprocess.run([sys.executable, str(HERE / name), *extra, "--state-dir", str(state_dir)],
                                       capture_output=True, text=True, timeout=TIMEOUT)
            output, returncode = completed.stdout, completed.returncode
        except subprocess.TimeoutExpired as expired:
            output = (expired.stdout or b"").decode("utf-8", "replace") if isinstance(expired.stdout, bytes) \
                else (expired.stdout or "")
            returncode, why = 124, f"{why} — timed out after {TIMEOUT}s"
        elapsed = round(time.time() - started, 1)
        status = "ok  " if returncode == 0 else "FAIL"
        print(f"{status} {name:20s} {elapsed:5.1f}s  {why}")
        if returncode == 0:
            shutil.rmtree(state_dir, ignore_errors=True)
        else:
            failures.append(name)
            stop_daemons(state_dir)
            print(f"       state kept for inspection: {state_dir}")
            for line in output.splitlines()[-6:]:
                print(f"       {line}")
    total = round(time.time() - started_all, 1)

    new_daemons, new_strays = daemons(), strays() - before_strays
    leaks = []
    # a clean run keeps nothing: the harness root only survives when it holds a failure's evidence
    if not failures:
        shutil.rmtree(harness_root, ignore_errors=True)
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
