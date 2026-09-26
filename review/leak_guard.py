#!/usr/bin/env python3
"""The two ways a run leaks into the machine, and the one implementation of both rules (D-147).

A test suite or a probe can leak in exactly two ways, and this repository has recorded a defect for each: a
**live session daemon** the in-test guard did not stop (D-111) and a **scratch directory** a daemon socket path
cannot outlive (D-131). `make test` counted both and stopped neither, so its failure message was a number
("the suite left 1 daemon(s) behind") — never which process, never which directory — and the leaked daemon kept
running, so the *next* run's baseline counted it. `review/dogfood/probes.py` had the same guard written a
second time, and its sweep had been matching nothing at all while it reported a leak it could not stop
(measured 2026-09-26, D-144): the same defect shape twice, in two copies of one rule.

This module is that rule, once:

    python3 review/leak_guard.py snapshot /tmp/guard.json   # before a suite or a probe set
    python3 review/leak_guard.py audit /tmp/guard.json      # after it: name the difference, stop the daemons
    python3 review/leak_guard.py --self-check               # the rules, with no suite, no daemon and no model

`make test` wraps the suite with snapshot/audit; `review/dogfood/probes.py` imports the predicates directly, so
counting and stopping can never disagree about what a daemon is.

Three rules the measurements decided, each stated where it is implemented: a daemon is recognised by its
**first argument**, not by the text of its command line (D-144); a **zombie is not a leak** (D-144); scratch is
**directories only** (D-143). The audit stops a leaked daemon — a process holding a socket and a state root has
no business outliving the run that started it — but keeps its **state root**: that directory is the evidence,
and D-140 is the case where it was gone by the time anyone looked.
"""
import argparse
import contextlib
import io
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tempfile
import time

REPO = pathlib.Path(__file__).resolve().parents[1]
BIN = REPO / "engine/target/debug/teamagents"
SCRATCH_PREFIX = "ta-"

# A daemon refuses to start without a model profile ("the user config has no model profile"), and this guard's
# controls need one that really runs; no key is read unless a model is called, so the guard stays
# credential-free.
CONFIG = """# The leak guard's own control: a daemon needs a model profile to start.
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
"""


def is_running(stat: str) -> bool:
    """Is this process alive? A zombie is not.

    `crash.py` kills its daemons on purpose, and in this container an orphaned dead child stays `<defunct>`
    (pid 1 does not reap). Counting those reported a leak for a process that serves nothing and cannot be
    killed (D-144) — and the kernel garbles a corpse's `args` column the same way (`<def [teamagents]
    <defunct>`, measured 2026-09-26), so `daemon_pids` excludes a zombie twice over: by this rule and by the
    argument test. Both stay, because only one of them is the kernel's behaviour rather than this container's.
    """
    return not stat.startswith("Z")


def daemon_pids(root: pathlib.Path | None = None) -> list[tuple[int, str]]:
    """`(pid, args)` for the live session daemons, optionally only those serving a root under `root`.

    A daemon is recognised by its **first argument** (`teamagents daemon …`), which is what `make test`'s guard
    checked too. The `args` column of `ps` begins with the binary's *path*, so a prefix test such as
    `args.startswith("daemon ")` matches nothing at all — measured 2026-09-26, after the sweep had been
    silently doing nothing while the guard reported a leak it could not stop (D-144).
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


def strays(root: pathlib.Path | None = None) -> set[str]:
    """Scratch *directories* left in `TMPDIR` (the parameter is for the self-check), as full paths.

    Directories only: every suite's and probe's scratch is one (`mkdtemp` or a `mkdir`ed root), while plain
    files with the same prefix appear in `TMPDIR` from elsewhere on this machine — measured 2026-09-26, an
    empty `ta-cap-stdout` and `ta-cap-stderr` and a `ta-wide-d71.log` none of which this tree writes — and a
    guard that counted them would report a leak that is not there (D-143).
    """
    base = pathlib.Path(root if root is not None else os.environ.get("TMPDIR", "/tmp"))
    if not base.is_dir():
        return set()
    return {str(p) for p in base.glob(SCRATCH_PREFIX + "*") if p.is_dir()}


def observe(root: pathlib.Path | None = None) -> dict:
    """What the two rules see right now: `{"daemons": [[pid, args], …], "strays": [path, …]}`."""
    return {"daemons": [[pid, args] for pid, args in daemon_pids(root)], "strays": sorted(strays(root))}


def _alive(pids: list[int]) -> list[int]:
    """Which of these pids are still live daemons — never a pattern, and never a corpse (D-144)."""
    known = {pid for pid, _args in daemon_pids()}
    return [pid for pid in pids if pid in known]


def stop(pids: list[int], grace: float = 5.0, kill_grace: float = 10.0) -> list[int]:
    """Stop these daemons by pid: SIGTERM, then SIGKILL for what is left; return the survivors.

    Signals go to a pid, never to a pattern: `pgrep`/`pkill -f` match any command line that *contains* the
    string, so `daemon --state-root <root>` also matched the shells whose text mentioned it and killed two of
    this session's own shells (D-144). The kill is escalated because a daemon that ignores TERM outlives its
    caller: `crash.py`'s daemon was still listed ~20 s after its probe exited (measured 2026-09-26, D-141).
    """
    for pid in pids:
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            continue
    deadline = time.time() + grace
    while time.time() < deadline and _alive(pids):
        time.sleep(0.25)
    for pid in _alive(pids):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    deadline = time.time() + kill_grace
    while time.time() < deadline and _alive(pids):
        time.sleep(0.25)
    return _alive(pids)


def snapshot(path: pathlib.Path, root: pathlib.Path | None = None) -> None:
    """Write down what the guards see, so the audit can tell a leak from something that was already there."""
    pathlib.Path(path).write_text(json.dumps(observe(root), indent=1) + "\n")


def state_root_of(args: str) -> str:
    """The `--state-root` value in a daemon's argument list, for the message ("which daemon is this?")."""
    parts = args.split()
    for index, part in enumerate(parts):
        if part == "--state-root" and index + 1 < len(parts):
            return parts[index + 1]
        if part.startswith("--state-root="):
            return part.split("=", 1)[1]
    return "(no --state-root named)"


def audit(path: pathlib.Path, root: pathlib.Path | None = None, out=sys.stdout) -> int:
    """Report what appeared since the snapshot, stop the leaked daemons, and return non-zero if anything did.

    A leak is a *difference*, so a daemon or a directory that was already there before the run is not this
    run's doing and is left alone (a developer's own session, an earlier failure's kept state).
    """
    before = json.loads(pathlib.Path(path).read_text())
    now = observe(root)
    known = {pid for pid, _args in before["daemons"]}
    leaked = [(pid, args) for pid, args in now["daemons"] if pid not in known]
    new_strays = sorted(set(now["strays"]) - set(before["strays"]))
    if not leaked and not new_strays:
        print(f"no leak: {len(now['daemons'])} daemon(s) and {len(now['strays'])} scratch directory(ies) "
              "present before the run are still all there is", file=out)
        return 0
    survivors = stop([pid for pid, _args in leaked]) if leaked else []
    for pid, args in leaked:
        fate = "survived SIGTERM and SIGKILL" if pid in survivors else "gone"
        print(f"leaked daemon pid {pid} (state root {state_root_of(args)}): sent SIGTERM, then SIGKILL — {fate}\n"
              f"  started as: {args[:160]}", file=out)
    for stray in new_strays:
        # kept on purpose: a leaked state root is the evidence of the test that wrote it (D-140)
        print(f"leaked scratch directory {stray} (kept: the directory is the evidence)", file=out)
    print(f"FAIL: {len(leaked)} daemon(s) and {len(new_strays)} scratch directory(ies) appeared during the run; a "
          "test that starts a daemon must stop it and remove its own temp directory (D-111, D-131)", file=out)
    return 1


def wait_until(predicate, seconds: float) -> bool:
    """Poll `predicate` for up to `seconds`: the self-check's controls need a started process to appear."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.25)
    return bool(predicate())


def process_state(pid: int) -> str | None:
    """The `stat` column for one pid, or None once the kernel has forgotten it."""
    listing = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return listing.splitlines()[0].strip() if listing else None


def _start_daemon(state_root: pathlib.Path) -> subprocess.Popen:
    """Start the built daemon on an isolated state root (`--self-check`'s controls)."""
    return subprocess.Popen([str(BIN), "daemon", "--state-root", str(state_root)],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=dict(os.environ))


def _restore_environment(keep: dict) -> None:
    for name, value in keep.items():
        if value is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = value


def self_check() -> int:
    """Check the guard's own rules, with no suite, no model and nothing left behind.

    The rules that are easy to get subtly wrong are exactly the ones this repository got wrong: a predicate
    that silently matches nothing (D-144), a corpse counted as a live process (D-144) and a file counted as a
    scratch directory (D-143). Each has a control here that fails if the rule regresses, and the leak control
    drives the whole audit path — report, stop, non-zero exit — not only the predicate.

    The corpse rule is asserted twice on purpose: as a pure function (`is_running`), because the end-to-end case
    cannot isolate it here — `ps` replaces a zombie's arguments with `<defunct>`, so the argument test alone
    already excludes the corpse (measured while writing this check).
    """
    if not BIN.is_file():
        print(f"FAIL: {BIN} is missing; build it first (make build)")
        return 1
    findings: list[str] = []
    started: list[int] = []   # every pid this check starts, so a failing control cannot leak one either
    keep = {name: os.environ.get(name) for name in ("TMPDIR", "XDG_CONFIG_HOME", "XDG_STATE_HOME")}
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="leak-guard-"))
    try:
        # an isolated TMPDIR, so the scratch control cannot see (or be confused by) the machine's own files
        os.environ["TMPDIR"] = str(tmp)
        (tmp / "ta-a-directory").mkdir()
        (tmp / "ta-a-file").write_text("")
        (tmp / "config/teamagents").mkdir(parents=True)
        (tmp / "config/teamagents/config.toml").write_text(CONFIG)
        os.environ["XDG_CONFIG_HOME"] = str(tmp / "config")
        os.environ["XDG_STATE_HOME"] = str(tmp / "state")

        # 0. the corpse rule as a pure function (the end-to-end case cannot isolate it here, see the docstring)
        if is_running("Z") or is_running("Z+") or not is_running("Sl"):
            findings.append(f"a zombie is not a running process: Z->{is_running('Z')}, Sl->{is_running('Sl')}")

        # 1. scratch is directories only (D-143)
        if strays() != {str(tmp / "ta-a-directory")}:
            findings.append(f"the stray rule must count directories only, got {sorted(strays())}")

        # 2. an unchanged machine is not a leak (the audit must not report what it merely sees)
        guard = tmp / "guard.json"
        snapshot(guard)
        report = io.StringIO()
        if audit(guard, out=report) != 0:
            findings.append(f"an unchanged machine must audit clean, got: {report.getvalue().strip()}")

        # 3. a daemon this check starts by hand is found by the predicate (the D-144 control)
        daemon = _start_daemon(tmp / "root")
        started.append(daemon.pid)
        if not wait_until(lambda: daemon.pid in {pid for pid, _args in daemon_pids()}, 20):
            findings.append(f"the daemon predicate did not find a daemon this check started "
                            f"(pid {daemon.pid}, stat {process_state(daemon.pid)!r})")

        # 4. the audit reports a daemon that appeared after the snapshot, stops it, and exits non-zero
        report = io.StringIO()
        if audit(guard, out=report) != 1:
            findings.append(f"a daemon started after the snapshot must audit as a leak, got: {report.getvalue()}")
        text = report.getvalue()
        if str(daemon.pid) not in text or str(tmp / "root") not in text:
            findings.append(f"the leak report must name the pid and the state root: {text.strip()}")
        if daemon.pid in {pid for pid, _args in daemon_pids(tmp)}:
            findings.append("the audit did not stop the daemon it reported")
            stop([daemon.pid])  # a failed control must not leave its own daemon behind either
        with contextlib.suppress(subprocess.TimeoutExpired):
            daemon.wait(timeout=10)

        # 5. a zombie is not a live daemon (D-144)
        corpse = _start_daemon(tmp / "corpse-root")
        started.append(corpse.pid)
        wait_until(lambda: corpse.pid in {pid for pid, _args in daemon_pids()}, 20)
        corpse.kill()
        if not wait_until(lambda: (process_state(corpse.pid) or "").startswith("Z"), 20):
            findings.append(f"the zombie control did not produce a zombie (stat {process_state(corpse.pid)!r})")
        elif corpse.pid in {pid for pid, _args in daemon_pids()}:
            findings.append("a zombie was counted as a live daemon")
        stop([corpse.pid])
        with contextlib.suppress(subprocess.TimeoutExpired):
            corpse.wait(timeout=10)

        # 6. nothing this check did is still running
        if daemon_pids(tmp):
            findings.append(f"the self-check left daemons behind: {daemon_pids(tmp)}")
    finally:
        # the guard's own rule, applied to the guard: signals go to the pids it started, so a control that fails
        # cannot leave a daemon behind (a broken predicate could not find it afterwards; a pid list can)
        stop(started)
        _restore_environment(keep)
        shutil.rmtree(tmp, ignore_errors=True)

    for finding in findings:
        print("FAIL:", finding)
    if not findings:
        print("self-check ok: the daemon predicate, the zombie rule, the scratch rule and the audit/stop path")
    return 1 if findings else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("verb", nargs="?", choices=("snapshot", "audit"), help="what to do with a guard file")
    parser.add_argument("file", nargs="?", help="the guard file: written by snapshot, read by audit")
    parser.add_argument("--self-check", action="store_true", help="check the guard's own rules and exit")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if not args.verb or not args.file:
        parser.error("snapshot FILE or audit FILE (or --self-check)")
    if args.verb == "snapshot":
        snapshot(pathlib.Path(args.file))
        return 0
    return audit(pathlib.Path(args.file))


if __name__ == "__main__":
    sys.exit(main())
