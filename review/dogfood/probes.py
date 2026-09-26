#!/usr/bin/env python3
"""The dogfood probes in one command, in the two sets their prerequisites split them into (D-138, D-141)

The probes in this directory drive the real product — the built CLI, a real daemon, a real terminal, a real
socket, a real model — and this harness runs a whole set of them with one command, reporting a line per probe:

    python3 review/dogfood/probes.py                  # --set offline (the default): no model, no credential
    python3 review/dogfood/probes.py --set models     # the probes that take a model, one after another (~7 min)
    python3 review/dogfood/probes.py --set all
    python3 review/dogfood/probes.py --list           # every probe in both sets, and why it is there
    python3 review/dogfood/probes.py --only checks.py [--only boundary.py]

It needs the built binaries (`make build`; the Makefile targets depend on it). The `models` set additionally
needs the credential its configurations name (`DEEPSEEK_API_KEY` by default), and every probe runs at its
model's native context window (D-36).

Each probe runs with an explicit `--state-dir` under this harness's own root, and the harness closes the two
loops a probe can leave open: it counts the scratch directories and the session daemons before and after, and
fails if the run added either (D-111 for daemons, D-131 for scratch); and a probe that fails or is killed keeps
its state, with the path printed, because that directory is the evidence (D-140) — a probe something else has
to kill cannot run its own cleanup, which is D-138's measured case.

Ceiling: the `offline` set is the credential-free subset; the `models` set is one probe at a time, deliberately
— they share a machine and a provider, and a batch is not a benchmark. `--only` matches by file name, so
`providers.py` in both sets runs both of its shapes.
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

# (file, extra arguments, why it is in this set) — the credential-free set: no model, no credential
OFFLINE = [
    ("boundary.py", [], "the state roots the CLI refuses (A33/A34)"),
    ("budget.py", [], "a ceiling below one request is refused before any model call (A18)"),
    ("truncation.py", [], "A19's whole path over a real socket: truncated before output retries, after it fails"),
    ("input_latency.py", [], "per-keystroke composer latency against the scripted daemon"),
    ("tui_panels.py", [], "the instances panel's keys against a real daemon (D-95)"),
    ("tui_reconnect.py", [], "the TUI through a daemon kill and restart (A28, D-99)"),
    ("providers.py", ["--self-check"], "the A27 probe's task-result rule, without a model"),
]

# The set that takes a model. Each probe is the live half of an acceptance item or a decision; `--state-dir`
# keeps a failure's evidence (the harness passes one under its own root and keeps it when a probe fails).
MODELS = [
    ("run.py", ["--task", "edit-integrity"], "a fixture task the probe verifies itself, outside the agent"),
    ("crash.py", [], "a daemon crash replays nothing (A08/A11/A12)"),
    ("unknown_outcome.py", [], "the unverifiable crash parks the task and never replays the effect (A09)"),
    ("cancel.py", [], "the terminate lever really stops a running command (A13/D-88)"),
    ("lifecycle_run.py", [], "what pause and terminate do to a run that is already waiting (D-98)"),
    ("job_identity.py", [], "the running job's identity, a duplicate GO and a guessed token (A15/A10)"),
    ("checks.py", [], "a required check that can never pass blocks the goal (A16/D-50)"),
    ("stale_check.py", [], "a check that rewrites its own declared input cannot let the goal settle (A17)"),
    ("two_gates.py", [], "the runtime's checks and the client's `--check` in one run (D-101)"),
    ("exec_check.py", [], "the client's `--check` contract, a forged success marker included (D-93)"),
    ("queued_input.py", [], "a queued input's run reports its own outcome (D-72)"),
    ("runtime_note.py", [], "the runtime's closing note rides the next turn (D-71)"),
    ("instructions.py", [], "`instruction_files` is declared, validated and not read (D-102)"),
    ("hooks.py", [], "the `pre_tool` veto and the `notify` stream (D-53)"),
    ("skills.py", [], "the configured skills registry reaches the model (A26)"),
    ("web.py", [], "the bound web tools and their private-address guard (D-79)"),
    ("mcp.py", [], "a configured MCP service is bound and called (D-74)"),
    ("mcp_http.py", [], "the HTTP transport and its bearer token, live (D-104)"),
    ("workspace.py", [], "the git-worktree lifecycle, end to end (D-76)"),
    ("authority.py", [], "the user's authority surface: grant, a worker runs, revoke (D-61)"),
    ("approval.py", [], "the approval decision in the real TUI (A25/D-89)"),
    ("tui.py", [], "the real TUI on a real daemon, with the answer on screen (D-85)"),
    ("team_ring.py", [], "a message travels A -> B -> C -> A around a real team (A02/D-118)"),
    ("providers.py", [], "one team spanning DeepSeek and Kimi (A27)"),
]

SETS = {"offline": OFFLINE, "models": MODELS, "all": OFFLINE + MODELS}
# Per-probe budget, by set: the credential-free probes answer in under a minute, while a model probe can
# wait on a turn (`crash.py` takes ~70 s, and one `authority.py` run needed more than 400 s while the host
# was loaded), so the models set gets the room rather than the harness reporting a slow probe as a failure.
TIMEOUTS = {"offline": 300, "models": 900, "all": 900}


def daemons() -> int:
    """Live session daemons, by the subcommand (D-111's predicate, not the binary's name)."""
    listing = subprocess.run(["ps", "-eo", "comm,args"], capture_output=True, text=True).stdout
    return sum(1 for line in listing.splitlines()[1:]
               if len(parts := line.split(None, 2)) == 3 and parts[0] == "teamagents"
               and parts[2].startswith("daemon "))


def stop_daemons(state_dir: pathlib.Path) -> None:
    """Stop whatever serves this probe's state root: a killed probe cannot do it itself.

    SIGTERM first, then SIGKILL for what is left: the harness reports a leftover daemon as a leak, so a probe it
    had to kill must not leave one behind because the daemon was slow to honour the first signal (measured: a
    daemon from a timed-out probe outlived the guard's 15 s window — D-141).
    """
    for root in state_dir.rglob("root"):
        pattern = f"daemon --state-root {root}"
        subprocess.run(["pkill", "-f", pattern], capture_output=True)
        for _ in range(10):
            if subprocess.run(["pgrep", "-f", pattern], capture_output=True).returncode != 0:
                break
            time.sleep(0.5)
        else:
            subprocess.run(["pkill", "-9", "-f", pattern], capture_output=True)


def strays() -> set:
    """Scratch directories the probes would leave behind, by their shared prefix."""
    return {p.name for p in pathlib.Path(os.environ.get("TMPDIR", "/tmp")).glob("ta-*")}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="print the probes in both sets and exit")
    parser.add_argument("--only", action="append", help="run one probe by file name (repeatable)")
    parser.add_argument("--set", choices=sorted(SETS), default="offline",
                        help="which set to run (default: offline, the credential-free one)")
    args = parser.parse_args()

    if args.list:
        for label, probes in (("offline (no model, no credential)", OFFLINE), ("models (credentials required)", MODELS)):
            print(f"  {label}:")
            for name, extra, why in probes:
                print(f"    {name:20s} {' '.join(extra):14s} {why}")
        return 0

    chosen = [p for p in SETS[args.set] if not args.only or p[0] in args.only]
    if not chosen:
        print(f"no probe matches {args.only}")
        return 1
    if args.set in ("models", "all") and not (os.environ.get("DEEPSEEK_API_KEY", "").strip()
                                              or os.environ.get("KIMI_API_KEY", "").strip()):
        print("the models set needs a credential: set DEEPSEEK_API_KEY (the probes' default) or KIMI_API_KEY, "
              "or run --set offline for the seven that need neither")
        return 2
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
    timeout = TIMEOUTS[args.set]
    harness_root = pathlib.Path(os.environ.get("TMPDIR", "/tmp")) / f"teamagents-probe-harness-{os.getpid()}"
    for name, extra, why in chosen:
        state_dir = harness_root / name.removesuffix(".py")
        started = time.time()
        try:
            # `-u`: a probe the harness has to kill must still have printed its findings (measured: an
            # `authority.py` whose turn ran to its own 600 s deadline produced nothing, because Python
            # buffers stdout when it is not a terminal — D-141)
            completed = subprocess.run([sys.executable, "-u", str(HERE / name), *extra, "--state-dir", str(state_dir)],
                                       capture_output=True, text=True, timeout=timeout)
            output, returncode = completed.stdout, completed.returncode
        except subprocess.TimeoutExpired as expired:
            output = (expired.stdout or b"").decode("utf-8", "replace") if isinstance(expired.stdout, bytes) \
                else (expired.stdout or "")
            returncode, why = 124, f"{why} — timed out after {timeout}s"
        elapsed = round(time.time() - started, 1)
        status = "ok  " if returncode == 0 else "FAIL"
        print(f"{status} {name:20s} {elapsed:5.1f}s  {why}", flush=True)
        if returncode == 0:
            shutil.rmtree(state_dir, ignore_errors=True)
        else:
            failures.append(name)
            stop_daemons(state_dir)
            print(f"       state kept for inspection: {state_dir}")
            for line in output.splitlines()[-6:]:
                print(f"       {line}")
    total = round(time.time() - started_all, 1)

    # `pkill` returns before the daemon it signalled is gone, so give a stopped daemon a moment to leave before
    # calling it a leak: the guard must not report a process that is on its way out (measured: a failing probe's
    # daemon was still counted, and the run was red for it — D-141).
    deadline = time.time() + 15
    while time.time() < deadline and daemons() > before_daemons:
        time.sleep(0.5)
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
