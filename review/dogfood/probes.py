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
import signal
import subprocess
import sys
import tempfile
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
# Per-probe budget, by set: the credential-free probes answer in under a minute, while a model probe waits on
# turns whose length the model chooses. Measured 2026-09-26: `authority.py` took 21 s in one run and 621 s in
# another on the same build, so the models set gets the room (its two turns are bounded by the probe's own 600 s
# each) rather than the harness reporting a slow probe as a failure (D-143).
TIMEOUTS = {"offline": 300, "models": 1800, "all": 1800}


def select(set_name: str, only: list | None) -> list:
    """The probes a run will execute: the whole set, or every entry whose file name was asked for.

    `providers.py` is deliberately in both sets, so `--only providers.py` can select two entries with different
    arguments; `probes.py --self-check` states that.
    """
    return [p for p in SETS[set_name] if not only or p[0] in only]


def needs_credentials(set_name: str) -> bool:
    """Does this set need a model credential? `--self-check` states the answer for every set."""
    return set_name in ("models", "all")


def is_running(stat: str) -> bool:
    """Is this process alive? A zombie is not.

    `crash.py` kills its daemons on purpose, and in this container an orphaned dead child stays `<defunct>`
    (pid 1 does not reap), so a corpse keeps the binary's name and its argv. Counting those reported a leak for
    a process that serves nothing and cannot be killed (D-144).
    """
    return not stat.startswith("Z")


def daemon_pids(root: pathlib.Path | None = None) -> list:
    """`(pid, args)` for the live session daemons, optionally only those serving a root under `root`.

    A daemon is recognised by its **first argument** (`teamagents daemon …`), which is what `make test`'s guard
    checks too. The `args` column of `ps` begins with the binary's *path*, so a prefix test such as
    `args.startswith("daemon ")` matches nothing at all — measured 2026-09-26, after the sweep had been
    silently doing nothing while the guard reported a leak it could not stop (D-144).

    The stop below signals by pid, never by a pattern: `pgrep`/`pkill -f` match any command line that *contains*
    the string, so `daemon --state-root <root>` also matched the shells whose text mentioned it and killed two
    of this session's own shells. The probes' own `stop_daemon` helpers still carry that hazard; this harness
    does not.
    """
    listing = subprocess.run(["ps", "-eo", "pid,stat,comm,args"], capture_output=True, text=True).stdout
    found = []
    for line in listing.splitlines()[1:]:
        parts = line.split(None, 3)
        if len(parts) != 4 or not is_running(parts[1]) or parts[2] != "teamagents":
            continue
        if parts[3].split()[1:2] != ["daemon"]:
            continue
        if root is None or str(root) in parts[3]:
            found.append((int(parts[0]), parts[3]))
    return found


def daemons(root: pathlib.Path | None = None) -> int:
    """How many live daemons there are — the same predicate as `daemon_pids`, so counting and stopping can
    never disagree."""
    return len(daemon_pids(root))


def stop_daemons(state_dir: pathlib.Path) -> None:
    """Stop whatever serves a probe's state root: a killed probe cannot do it itself.

    SIGTERM first, then SIGKILL for what is left (a daemon from a timed-out probe outlived the guard's window,
    D-141); both go to the pid the predicate found, so nothing else can be signalled.
    """
    for pid, _args in daemon_pids(state_dir):
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            continue
    for _ in range(10):
        if not daemon_pids(state_dir):
            return
        time.sleep(0.5)
    for pid, _args in daemon_pids(state_dir):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def strays() -> set:
    """Scratch *directories* the probes would leave behind, by their shared prefix.

    Directories only: every probe's scratch is one (`mkdtemp` or a `mkdir`ed root), while plain files with the
    same prefix appear in `TMPDIR` from elsewhere on this machine — measured 2026-09-26, an empty `ta-cap-stdout`
    and `ta-cap-stderr` and a `ta-wide-d71.log` none of which this tree writes — and a guard that counted them
    would report a leak that is not there (D-143).
    """
    return {p.name for p in pathlib.Path(os.environ.get("TMPDIR", "/tmp")).glob("ta-*") if p.is_dir()}


def self_check() -> int:
    """Check the harness's own rules, with no model, no daemon and no probe run (D-144).

    The harness has accumulated rules that are easy to get subtly wrong — a guard that counts the wrong thing is
    exactly the defect this session kept finding in the probes' assertions — so the rules that can be stated
    without a session are stated here: the stray guard's precision, the selection, and the budget's coverage.
    """
    findings = []
    if select("offline", None) != OFFLINE or select("all", None) != OFFLINE + MODELS:
        findings.append("select(set, None) must be the set itself")
    if len(select("all", ["providers.py"])) != 2 or len(select("offline", ["providers.py"])) != 1:
        findings.append("providers.py is in both sets, so --only must select it in each set it appears in")
    if select("offline", ["nothing.py"]):
        findings.append("an unmatched --only must select nothing (the caller reports it)")
    if set(SETS) != set(TIMEOUTS):
        findings.append(f"every set needs a per-probe budget: sets={sorted(SETS)} timeouts={sorted(TIMEOUTS)}")
    if needs_credentials("offline") or not needs_credentials("models") or not needs_credentials("all"):
        findings.append("the credential requirement is wrong: only the model sets need one")

    keep = os.environ.get("TMPDIR")
    try:
        with tempfile.TemporaryDirectory() as tmp:
            os.environ["TMPDIR"] = tmp
            (pathlib.Path(tmp) / "ta-a-directory").mkdir()
            (pathlib.Path(tmp) / "ta-a-file").write_text("")   # files with the prefix appear from elsewhere
            counted = strays()
            if counted != {"ta-a-directory"}:
                findings.append(f"the stray guard must count directories only, got {sorted(counted)}")
    finally:
        if keep is None:
            os.environ.pop("TMPDIR", None)
        else:
            os.environ["TMPDIR"] = keep

    for finding in findings:
        print("FAIL:", finding)
    if not findings:
        print(f"self-check ok: selection, budgets and the stray guard over {len(SETS)} sets")
    return 1 if findings else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="print the probes in both sets and exit")
    parser.add_argument("--self-check", action="store_true", help="check the harness's own rules and exit")
    parser.add_argument("--only", action="append", help="run one probe by file name (repeatable)")
    parser.add_argument("--set", choices=sorted(SETS), default="offline",
                        help="which set to run (default: offline, the credential-free one)")
    args = parser.parse_args()

    if args.self_check:
        return self_check()
    if args.list:
        for label, probes in (("offline (no model, no credential)", OFFLINE), ("models (credentials required)", MODELS)):
            print(f"  {label}:")
            for name, extra, why in probes:
                print(f"    {name:20s} {' '.join(extra):14s} {why}")
        return 0

    chosen = select(args.set, args.only)
    if not chosen:
        print(f"no probe matches {args.only}")
        return 1
    if needs_credentials(args.set) and not (os.environ.get("DEEPSEEK_API_KEY", "").strip()
                                            or os.environ.get("KIMI_API_KEY", "").strip()):
        print("the models set needs a credential: set DEEPSEEK_API_KEY (the probes' default) or KIMI_API_KEY, "
              "or run --set offline for the seven that need neither")
        return 2
    missing = [name for name, _extra, _why in chosen if not (HERE / name).is_file()]
    if missing:
        print(f"missing probe file(s): {missing}")
        return 1

    before_strays = strays()
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
        # say so *before* the probe runs: a model probe can take minutes (authority.py ran 587 s once), and a
        # silent harness gives no way to tell a long turn from a hang (D-144)
        print(f"     {name} …", flush=True)
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
                print(f"       {line}", flush=True)
    total = round(time.time() - started_all, 1)

    # `pkill` returns before the daemon it signalled is gone, so give a stopped daemon a moment to leave before
    # calling it a leak: the guard must not report a process that is on its way out (measured: a failing probe's
    # daemon was still counted, and the run was red for it — D-141). A probe that passed relies on its own
    # `atexit`, which sends TERM only, so if the count has still grown after the wait the harness applies the
    # same TERM-then-KILL sweep it applies to a failed probe — over its own root, where every probe's state root
    # lives — and only then reports a survivor (measured 2026-09-26: a full set with no failing probe was red
    # for a daemon that outlived its own stop, D-144).
    deadline = time.time() + 15
    while time.time() < deadline and daemons(harness_root):
        time.sleep(0.5)
    if daemons(harness_root):
        stop_daemons(harness_root)
        # A grace long enough for a shutdown that is genuinely under way: `crash.py`'s daemon was still listed
        # ~20 s after its probe exited and gone when checked moments later (measured 2026-09-26). A daemon that
        # ignores TERM *and* the KILL above for a minute is a leak; one that needs a few seconds is not.
        deadline = time.time() + 60
        while time.time() < deadline and daemons(harness_root):
            time.sleep(1.0)
    left, new_strays = daemons(harness_root), strays() - before_strays
    leaks = []
    # a clean run keeps nothing: the harness root only survives when it holds a failure's evidence
    if not failures:
        shutil.rmtree(harness_root, ignore_errors=True)
    if left:
        # name what survived, so the next occurrence says which probe's daemon it was and what it was started
        # with — the guard has twice reported a count and left the reader to guess (D-144)
        leaks.append(f"{left} daemon(s) left running under {harness_root}: {[args[:160] for _pid, args in daemon_pids(harness_root)]}")
    if new_strays:
        leaks.append(f"scratch left behind: {sorted(new_strays)}")
    print(f"{len(chosen)} probes in {total}s; daemons of this run left: {left}; "
          f"new scratch {sorted(strays() - before_strays) or 'none'}", flush=True)
    for leak in leaks:
        print("FAIL:", leak)
    return 1 if (failures or leaks) else 0


if __name__ == "__main__":
    sys.exit(main())
