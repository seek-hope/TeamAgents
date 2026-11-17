#!/usr/bin/env python3
"""The two ways a run leaks into the machine, and the one implementation of both rules (D-147).

A test suite or a probe can leak in exactly two ways, and this repository has recorded a defect for each: a
**live session daemon** the in-test guard did not stop (D-111), a **command runner** that outlives
the daemon that started it (D-226) and a **scratch directory** a daemon socket path
cannot outlive (D-131). `make test` counted both and stopped neither, so its failure message was a number
("the suite left 1 daemon(s) behind") — never which process, never which directory — and the leaked daemon kept
running, so the *next* run's baseline counted it. `tools/dogfood/probes.py` had the same guard written a
second time, and its sweep had been matching nothing at all while it reported a leak it could not stop
(measured 2026-09-26, D-144): the same defect shape twice, in two copies of one rule.

This module is that rule, once:

    python3 tools/leak_guard.py snapshot /tmp/guard.json   # before a suite or a probe set
    python3 tools/leak_guard.py audit /tmp/guard.json      # after it: name the difference, stop the daemons
    python3 tools/leak_guard.py --self-check               # the rules, with no suite, no daemon and no model

`make test` wraps the suite with snapshot/audit; `tools/dogfood/probes.py` imports the predicates directly, so
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
import uuid

REPO = pathlib.Path(__file__).resolve().parents[1]
BIN = REPO / "engine/target/debug/teamagents"
# The prefixes a run's scratch root carries in `TMPDIR`: `ta-*` (the `cli.rs`/install/probe families) and the
# integration suites' `teamagents-*` roots, which the rule could not see until D-226 — 46,025 of them had piled up
# on one developer machine (8.8 GiB) while the audit reported "no leak". The probe harness's own root
# (`teamagents-probe-harness-*`) is deliberately absent: a failing probe *keeps* it as evidence (D-140), and the
# harness reports that failure itself, so counting the kept root here would turn one failure into two (D-171).
SCRATCH_PREFIXES = ("ta-", "teamagents-v2-", "teamagents-jobs-", "teamagents-rebuild-")

# Ownership, which a name cannot prove (D-528). `ta-*` / `teamagents-*` is a naming *convention*: a concurrent
# session of the same user runs the same suites under the same `TMPDIR` and creates the same prefixes, so a
# difference since the snapshot is not by itself evidence that *this* run made it. The signal that is evidence is
# inherited: `make test` exports `TEAMAGENTS_LEAK_GUARD_RUN=<uuid>` for the whole gate, `cargo test` hands its
# environment to every suite process, and the product's detached spawns pass it on — `engine/src/main.rs` starts
# `setsid <exe> daemon …` and `engine/src/jobs/runner.rs` starts `jobs-runner` with `env_remove("RUST_LOG")` only,
# never `env_clear`. `/proc/<pid>/environ` is read from the process itself, so `setsid`, `--fork` and reparenting
# cannot hide it: the process tree is not the evidence, the environment is.
#
# Deliberately *not* claimed: a leaked child that clears or rewrites its environment (`env -i`, a login shell that
# resets it) is indistinguishable from a peer and is reported, not stopped; and a peer running an older build
# without the marker is spared but also unattributable. When no marker is set at all (a hand-run `audit`, older
# tooling) the rule keeps its previous machine-wide meaning rather than silently disabling itself.
RUN_MARKER = "TEAMAGENTS_LEAK_GUARD_RUN"

# The daemon refuses to start without the model profile's API key environment variable, and this check needs a
# daemon that really runs. The value is never used: no model is called, and the key is read only when one is.
PLACEHOLDER_KEY = "leak-guard-self-check-no-model-call"


def run_marker() -> str:
    """This run's ownership marker: `""` when the caller set none, which keeps the machine-wide rule."""
    return os.environ.get(RUN_MARKER, "")


def carries_marker(pid: int, expected: str) -> bool:
    """Does this pid's *own* environment carry `RUN_MARKER=expected`?

    Read from `/proc/<pid>/environ`, not from the process tree, so a detached or reparented child answers the same
    as one still under its parent. An empty `expected` means the caller set no marker: ownership is then not
    distinguishable and every difference counts, exactly as before.
    """
    if not expected:
        return True
    try:
        raw = pathlib.Path(f"/proc/{pid}/environ").read_bytes()
    except OSError:
        # A process that vanished cannot be asked; it is not evidence of ownership either way.
        return False
    return f"{RUN_MARKER}={expected}".encode() in raw.split(b"\0")

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


def _pids(subcommand: str, root: pathlib.Path | None = None) -> list[tuple[int, str]]:
    """`(pid, args)` for the live `teamagents <subcommand> …` processes serving a root under `root`.

    Such a process is recognised by its **first argument** (`teamagents daemon …`, `teamagents jobs-runner …`),
    which is what `make test`'s guard checked too. The `args` column of `ps` begins with the binary's *path*, so
    a prefix test such as `args.startswith("daemon ")` matches nothing at all — measured 2026-09-26, after the sweep had been
    silently doing nothing while the guard reported a leak it could not stop (D-144).
    """
    listing = subprocess.run(["ps", "-eo", "pid,stat,comm,args"], capture_output=True, text=True).stdout
    found = []
    for line in listing.splitlines()[1:]:
        parts = line.split(None, 3)
        if len(parts) != 4 or not is_running(parts[1]) or parts[2] != "teamagents":
            continue
        # Only this user's processes can be this suite's leak. A `teamagents daemon` started by root inside a
        # benchmark container is visible in `/proc` and cannot be signalled; counting it made `make check` exit 2
        # while a benchmark ran (measured 2026-10-02, D-384). Ownership, not a pattern, is the rule (D-144).
        try:
            if os.stat(f"/proc/{parts[0]}").st_uid != os.geteuid():
                continue
        except OSError:
            continue
        if parts[3].split()[1:2] != [subcommand]:
            continue
        if root is None or str(root) in parts[3]:
            found.append((int(parts[0]), parts[3]))
    return found


def daemon_pids(root: pathlib.Path | None = None) -> list[tuple[int, str]]:
    """The live session daemons (`teamagents daemon …`), optionally only those serving a root under `root`."""
    return _pids("daemon", root)


def daemons(root: pathlib.Path | None = None) -> int:
    """How many live daemons there are — the same predicate as `daemon_pids`, so counting and stopping can
    never disagree."""
    return len(daemon_pids(root))


def runner_pids(root: pathlib.Path | None = None) -> list[tuple[int, str]]:
    """The live command runners (`teamagents jobs-runner <job dir>`, §6.2/A12).

    A runner belongs to the *job directory* under a state root, so a probe that wants to crash a running
    command stops it the same way it stops a daemon: by pid, from a predicate — never by `pkill -f`.
    """
    return _pids("jobs-runner", root)


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
    found = set()
    for prefix in SCRATCH_PREFIXES:
        found |= {str(p) for p in base.glob(prefix + "*") if p.is_dir()}
    return found


def observe(root: pathlib.Path | None = None) -> dict:
    """What the three rules see right now: daemons, runners and stray scratch directories.

    A command runner (§6.2/A12) outlives the daemon that started it *by design*, so a test that crashes a session
    leaves one behind without any daemon to point at it — measured: `runner_crash_mid_job_is_outcome_unknown`
    left a live runner behind on every run until D-226 gave the suites a scratch root that stops them.
    """
    return {"daemons": [[pid, args] for pid, args in daemon_pids(root)],
            "runners": [[pid, args] for pid, args in runner_pids(root)],
            "strays": sorted(strays(root))}


def _alive(pids: list[int], finder=daemon_pids) -> list[int]:
    """Which of these pids are still live processes of that family — never a pattern, never a corpse (D-144)."""
    known = {pid for pid, _args in finder()}
    return [pid for pid in pids if pid in known]


def stop(pids: list[int], finder=daemon_pids, grace: float = 5.0, kill_grace: float = 10.0) -> list[int]:
    """Stop these processes by pid: SIGTERM, then SIGKILL for what is left; return the survivors.

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
        except PermissionError:
            # Another user's process: this guard is not entitled to stop it, and a guard that crashes on a
            # machine it shares is worse than one that reports. Measured 2026-10-02: a concurrent benchmark
            # made `make check` exit 2 here instead of reporting a result (D-384).
            continue
    deadline = time.time() + grace
    while time.time() < deadline and _alive(pids, finder):
        time.sleep(0.25)
    for pid in _alive(pids, finder):
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except PermissionError:
            pass
    deadline = time.time() + kill_grace
    while time.time() < deadline and _alive(pids, finder):
        time.sleep(0.25)
    return _alive(pids, finder)


def kill(pids: list[int], finder=daemon_pids, grace: float = 10.0) -> list[int]:
    """SIGKILL these processes and return the survivors: for a probe that deliberately *crashes* one.

    Not the same thing as `stop`: since D-150 the daemon handles SIGTERM and shuts itself down gracefully, so a
    probe that measures what a *crash* leaves behind (`crash.py`, `unknown_outcome.py`) must send the signal
    that has no handler. Signals still go to a pid, never to a pattern (D-144).
    """
    for pid in pids:
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            continue
        except PermissionError:
            continue  # another user's process; see `stop`
    deadline = time.time() + grace
    while time.time() < deadline and _alive(pids, finder):
        time.sleep(0.25)
    return _alive(pids, finder)


def kill_daemons(root: pathlib.Path) -> list[int]:
    """Crash the live daemons serving `root`: SIGKILL, no shutdown path (D-150)."""
    return kill([pid for pid, _args in daemon_pids(root)])


def kill_runners(root: pathlib.Path) -> list[int]:
    """Crash the live command runners serving `root`: SIGKILL, no shutdown path (D-150)."""
    return kill([pid for pid, _args in runner_pids(root)], finder=runner_pids)


def stop_daemons(root: pathlib.Path) -> list[int]:
    """Stop the live daemons serving `root` (or all of them): what every probe registers with `atexit`.

    Each probe used to run `pkill -f "daemon --state-root <root>"` here, the hazard D-144 measured: `pkill -f`
    matches any command line that *contains* the string, so it also matched the shells whose text mentioned it
    and killed two of this session's own shells (D-148).
    """
    return stop([pid for pid, _args in daemon_pids(root)])


def stop_runners(root: pathlib.Path) -> list[int]:
    """Stop the live command runners serving `root` (or all of them): `crash.py` and `unknown_outcome.py`."""
    return stop([pid for pid, _args in runner_pids(root)], finder=runner_pids)


def snapshot(path: pathlib.Path, root: pathlib.Path | None = None) -> None:
    """Write down what the guards see, so the audit can tell a leak from something that was already there."""
    pathlib.Path(path).write_text(json.dumps(observe(root), indent=1) + "\n")


def _served_path(args: str) -> str:
    """The directory a `daemon` or `jobs-runner` command line serves: its state root, or its job directory."""
    parts = args.split()
    if parts[1:2] == ["jobs-runner"]:
        return parts[2] if len(parts) > 2 else ""
    return state_root_of(args) if parts[1:2] == ["daemon"] else ""


def in_use(stray: str, daemons: list, runners: list) -> bool:
    """Is this scratch-looking directory being *used* by a live process right now?

    The stray rule's job is "a test left its temporary directory behind" (D-131), and a directory a live daemon or
    runner is serving has not been left behind: it is that process's state root or job directory. This matters for
    a concurrent session (D-528): its roots carry the same `ta-*` / `teamagents-*` names as our own, and a name
    cannot say whose it is — but "in use" can, without pretending to know the owner. It also keeps this audit from
    reporting a peer's *live* directory as our leak, while still reporting a directory whose processes are gone.
    """
    for _pid, args in list(daemons) + list(runners):
        served = _served_path(args)
        if served and (served == stray or served.startswith(stray.rstrip("/") + "/")):
            return True
    return False


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
    """Report what appeared since the snapshot, stop *this run's* leaked processes, and return non-zero if any did.

    A leak is a *difference*, so a daemon or a directory that was already there before the run is not this
    run's doing and is left alone (a developer's own session, an earlier failure's kept state).

    A difference is attributed before anything is signalled (D-528). Two things can make one: a process this run's
    suites leaked, or a *different live session* of the same user, which runs the same suites under the same
    `TMPDIR` and the same binary. The second is not a leak, and until the marker rule existed it was reported and
    SIGTERM-then-SIGKILLed — a peer's in-flight command died, its runner was gone and the journal it left at
    `RUNNING` became `OUTCOME_UNKNOWN` on recovery, which correctly refuses that session's `finish`. So only a
    process carrying this run's [`RUN_MARKER`] is signalled; a process without it is *named* in the report and its
    pid is left alone. The classes:

    * `leaked` — new, and this run's by environment. Signalled; fails the audit.
    * `foreign` — new, and carrying a different run's marker or none: another session's work. Reported, kept, and
      it does not fail this run. The message says why, so a reader is never told "clean" about a live daemon.
    * `unattributed` — new, and its environment could not be read (it vanished, or another user owns it). Also
      reported and kept: an unknown owner is not evidence that this run started it.

    The scratch-directory half has no environment to read, and it is deliberately **report-only**: this audit
    prints a stray and keeps it as evidence (D-140), it never removes it and never signals anything on its behalf,
    so a peer's directory can at worst be mentioned in this run's output. A name is a convention, not ownership,
    and this report does not claim otherwise.
    """
    before = json.loads(pathlib.Path(path).read_text())
    now = observe(root)
    marker = run_marker()
    known_daemons = {pid for pid, _args in before["daemons"]}
    known_runners = {pid for pid, _args in before.get("runners", [])}
    new_daemons = [(pid, args) for pid, args in now["daemons"] if pid not in known_daemons]
    new_runners = [(pid, args) for pid, args in now["runners"] if pid not in known_runners]

    def split(entries):
        """`(owned, foreign, unattributed)` for new processes, by inherited environment rather than by name."""
        owned, foreign, unattributed = [], [], []
        for pid, args in entries:
            if not marker:
                owned.append((pid, args))          # no marker set: the caller keeps the machine-wide rule
                continue
            try:
                raw = pathlib.Path(f"/proc/{pid}/environ").read_bytes()
            except OSError:
                unattributed.append((pid, args))
                continue
            (owned if f"{RUN_MARKER}={marker}".encode() in raw.split(b"\0") else foreign).append((pid, args))
        return owned, foreign, unattributed

    leaked, foreign_daemons, unattributed_daemons = split(new_daemons)
    leaked_runners, foreign_runners, unattributed_runners = split(new_runners)
    new_strays = sorted(set(now["strays"]) - set(before["strays"]))
    held = [stray for stray in new_strays if in_use(stray, now["daemons"], now["runners"])]
    new_strays = [stray for stray in new_strays if stray not in held]
    for pid, args in foreign_daemons + foreign_runners:
        kind = "command runner" if (pid, args) in foreign_runners else "daemon"
        print(f"another run's {kind} pid {pid} appeared during this run (serving "
              f"{_served_path(args) or 'an unnamed directory'}): kept, not signalled - it does not carry this run's marker, so it is a "
              f"concurrent session's work ({RUN_MARKER} is unset or different)", file=out)
    for pid, args in unattributed_daemons + unattributed_runners:
        print(f"unattributed process pid {pid} appeared during this run and its environment could not be read: "
              f"kept, not signalled - ownership is unknown, which is not evidence that this run leaked it",
              file=out)
    for stray in held:
        print(f"scratch directory {stray} appeared during this run and a live process is serving it: kept, not "
              f"counted as this run's leftover - a directory in use has not been left behind", file=out)
    if not leaked and not leaked_runners and not new_strays:
        kept_processes = len(foreign_daemons) + len(foreign_runners) + len(unattributed_daemons) \
            + len(unattributed_runners)
        if kept_processes or held:
            # "clean" here is this run's verdict, not a claim that nothing else is live: the peer processes and the
            # in-use directories above are somebody's work, and calling that machine idle would be a second bug
            print(f"no leak from this run: {len(now['daemons'])} daemon(s), {len(now['runners'])} runner(s) and "
                  f"{len(now['strays'])} scratch directory(ies) were present before the run; "
                  f"{kept_processes} process(es) of another or unknown run and {len(held)} in-use directory(ies) "
                  f"were reported above and left running (marker {marker or 'unset: machine-wide rule'})", file=out)
        else:
            print(f"no leak: {len(now['daemons'])} daemon(s), {len(now['runners'])} runner(s) and "
                  f"{len(now['strays'])} scratch directory(ies) present before the run are still all there is "
                  f"(marker {marker or 'unset: machine-wide rule'})", file=out)
        return 0
    survivors = stop([pid for pid, _args in leaked]) if leaked else []
    for pid, args in leaked:
        fate = "survived SIGTERM and SIGKILL" if pid in survivors else "gone"
        print(f"leaked daemon pid {pid} (state root {state_root_of(args)}): sent SIGTERM, then SIGKILL — {fate}\n"
              f"  started as: {args[:160]}", file=out)
    runner_survivors = stop([pid for pid, _args in leaked_runners], finder=runner_pids) if leaked_runners else []
    for pid, args in leaked_runners:
        fate = "survived SIGTERM and SIGKILL" if pid in runner_survivors else "gone"
        print(f"leaked command runner pid {pid}: sent SIGTERM, then SIGKILL — {fate}\n"
              f"  started as: {args[:160]}", file=out)
    for stray in new_strays:
        # kept on purpose: a leaked state root is the evidence of the test that wrote it (D-140)
        print(f"leaked scratch directory {stray} (kept: the directory is the evidence)", file=out)
    kept = len(foreign_daemons) + len(foreign_runners) + len(unattributed_daemons) + len(unattributed_runners)
    print(f"FAIL: {len(leaked)} daemon(s), {len(leaked_runners)} command runner(s) and {len(new_strays)} scratch "
          "directory(ies) appeared during the run; a test that starts a daemon or a runner must stop it and remove "
          f"its own temp directory (D-111, D-131, D-226)"
          + (f"; {kept} process(es) of another or unknown run were reported and left running (D-528)" if kept else ""),
          file=out)
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


def _start_daemon(state_root: pathlib.Path, marker: str | None = None) -> subprocess.Popen:
    """Start the built daemon on an isolated state root (`--self-check`'s controls).

    The profile in [`CONFIG`] reads an API key from the environment and the daemon refuses to start without it, so
    every control here used to die before it could be a subject (`stat 'Z<'`, measured on this machine before the
    fix). The placeholder is inert: this check never calls a model, and the key is only read when one is called.

    `marker` is the ownership marker the daemon inherits. `None` keeps the caller's environment; an explicit value
    is how one control plays a *peer session* — same user, same binary, same `/tmp` prefixes, different run.
    """
    env = dict(os.environ)
    env.setdefault("DEEPSEEK_API_KEY", PLACEHOLDER_KEY)
    if marker is not None:
        env[RUN_MARKER] = marker
    return subprocess.Popen([str(BIN), "daemon", "--state-root", str(state_root)],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)


def _start_runner(job_root: pathlib.Path, marker: str | None = None, seconds: int = 60) -> subprocess.Popen:
    """Start a real `jobs-runner` over a job directory this check wrote (§6.2), for the runner controls.

    The runner is the second thing a suite leaks, and it is started exactly the way the driver starts it — no
    `env_clear` — so it answers the same ownership question the daemon controls ask.
    """
    token = f"leak-guard-{uuid.uuid4()}"
    job_root.mkdir(parents=True, exist_ok=True)
    (job_root / "job.json").write_text(json.dumps({
        "job_id": "leak-guard-control", "program": "/bin/sh", "args": ["-c", f"sleep {seconds}"],
        "cwd": str(job_root), "env": [["PATH", os.environ.get("PATH", "/usr/bin:/bin")]],
        "deadline_ms": int(time.time() * 1000) + (seconds + 30) * 1000, "token": token,
    }))
    env = dict(os.environ)
    if marker is not None:
        env[RUN_MARKER] = marker
    return subprocess.Popen([str(BIN), "jobs-runner", str(job_root)], env=env,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


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

    The runner predicate is checked as far as a synthetic process can take it: a live daemon is *not* a runner
    (its first argument is `daemon`), the two families filter by root independently, and the observation carries
    all three rules (D-226). A live runner needs a job directory a daemon wrote, so the runners' own stop is
    exercised where they exist — `crash.py` and `unknown_outcome.py` (D-148) and, since D-226, the suites'
    scratch root, which stops the runners under it the way the probes do.
    """
    if not BIN.is_file():
        print(f"FAIL: {BIN} is missing; build it first (make build)")
        return 1
    findings: list[str] = []
    started: list[int] = []   # every pid this check starts, so a failing control cannot leak one either
    keep = {name: os.environ.get(name) for name in ("TMPDIR", "XDG_CONFIG_HOME", "XDG_STATE_HOME",
                                                     RUN_MARKER, "DEEPSEEK_API_KEY")}
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="leak-guard-"))
    try:
        # an isolated TMPDIR, so the scratch control cannot see (or be confused by) the machine's own files
        os.environ["TMPDIR"] = str(tmp)
        (tmp / "ta-a-directory").mkdir()
        (tmp / "ta-a-file").write_text("")
        (tmp / "teamagents-v2-leaked").mkdir()          # the suites' own prefix (D-226)
        (tmp / "teamagents-probe-harness-kept").mkdir()  # kept on purpose when a probe fails (D-140)
        (tmp / "config/teamagents").mkdir(parents=True)
        (tmp / "config/teamagents/config.toml").write_text(CONFIG)
        os.environ["XDG_CONFIG_HOME"] = str(tmp / "config")
        os.environ["XDG_STATE_HOME"] = str(tmp / "state")
        # the profile's API key variable: the daemon refuses to boot without it, and no model is ever called here
        os.environ.setdefault("DEEPSEEK_API_KEY", PLACEHOLDER_KEY)
        # this check is a run: it owns exactly the processes that inherit its marker
        os.environ[RUN_MARKER] = f"self-check-{uuid.uuid4()}"

        # 0. the corpse rule as a pure function (the end-to-end case cannot isolate it here, see the docstring)
        if is_running("Z") or is_running("Z+") or not is_running("Sl"):
            findings.append(f"a zombie is not a running process: Z->{is_running('Z')}, Sl->{is_running('Sl')}")

        # 1. scratch is directories only (D-143), and it carries both families of prefix (D-226) — while the
        # probe harness's kept root stays out of it, because that root is a failure's evidence (D-140)
        expected = {str(tmp / "ta-a-directory"), str(tmp / "teamagents-v2-leaked")}
        if strays() != expected:
            findings.append(f"the stray rule must count directories only, both prefixes and not the probe "
                            f"harness's kept root, got {sorted(strays())}")
        if set(observe()) != {"daemons", "runners", "strays"}:
            findings.append(f"the observation must carry the three rules, got {sorted(observe())}")

        # 2. an unchanged machine is not a leak (the audit must not report what it merely sees). Scoped to `tmp`:
        #    a self-check that swept the machine would stop real work to prove a rule (D-528).
        guard = tmp / "guard.json"
        snapshot(guard, tmp)
        report = io.StringIO()
        if audit(guard, tmp, out=report) != 0:
            findings.append(f"an unchanged machine must audit clean, got: {report.getvalue().strip()}")

        # 3. a daemon this check starts by hand is found by the predicate (the D-144 control)
        daemon = _start_daemon(tmp / "root")
        started.append(daemon.pid)
        if not wait_until(lambda: daemon.pid in {pid for pid, _args in daemon_pids(tmp)}, 20):
            findings.append(f"the daemon predicate did not find a daemon this check started "
                            f"(pid {daemon.pid}, stat {process_state(daemon.pid)!r})")
        if runner_pids(tmp) or not daemon_pids(tmp):
            findings.append(f"a daemon is not a runner: runners={runner_pids(tmp)}, daemons={daemon_pids(tmp)}")

        # 3b. the marker really is inherited through the product's own detached spawn, which is the whole premise of
        #     the ownership rule: `setsid <exe> daemon …` does not clear the environment (§6.2's runner does not
        #     either, see control 7). If this fails, the rule below would spare everything and catch nothing.
        if not carries_marker(daemon.pid, os.environ[RUN_MARKER]):
            findings.append(f"the run marker did not survive the daemon spawn (pid {daemon.pid}); the ownership "
                            f"rule cannot work without it")

        # 4. this run's daemon that appeared after the snapshot: reported, stopped, non-zero exit
        report = io.StringIO()
        if audit(guard, tmp, out=report) != 1:
            findings.append(f"a daemon started after the snapshot must audit as a leak, got: {report.getvalue()}")
        text = report.getvalue()
        if str(daemon.pid) not in text or str(tmp / "root") not in text:
            findings.append(f"the leak report must name the pid and the state root: {text.strip()}")
        if daemon.pid in {pid for pid, _args in daemon_pids(tmp)}:
            findings.append("the audit did not stop the daemon it reported")
            stop([daemon.pid])  # a failed control must not leave its own daemon behind either
        with contextlib.suppress(subprocess.TimeoutExpired):
            daemon.wait(timeout=10)

        # 4b. a scratch directory nobody serves is still this run's leftover (D-131): excluding in-use directories
        #     must not be a way to make the stray rule silently stop firing, so the positive case is pinned here.
        stray_guard = tmp / "stray-guard.json"
        snapshot(stray_guard, tmp)
        leftover = tmp / "ta-unserved-leftover"
        leftover.mkdir()
        report = io.StringIO()
        if audit(stray_guard, tmp, out=report) != 1:
            findings.append(f"a new scratch directory no live process serves must still audit as a leak, got: "
                            f"{report.getvalue().strip()}")
        if str(leftover) not in report.getvalue():
            findings.append(f"the leak report must name the stray directory: {report.getvalue().strip()}")
        shutil.rmtree(leftover, ignore_errors=True)

        # 5. a *peer session* is not this run's leak (D-528). Same user, same binary, same `/tmp` prefixes, a
        #    different run marker. Before the marker rule, `audit` reported this daemon and SIGTERM-then-SIGKILLed
        #    it, which is how a concurrent `make check` killed a live command runner and left the journal at
        #    RUNNING — the residue that blocks that session's `finish`. This is the control that fails before the
        #    fix and must keep failing if the rule is ever weakened back to a name or to a bare difference.
        peer_guard = tmp / "peer-guard.json"
        snapshot(peer_guard, tmp)
        peer = _start_daemon(tmp / "ta-peer-root", marker=f"peer-{uuid.uuid4()}")
        started.append(peer.pid)
        if not wait_until(lambda: peer.pid in {pid for pid, _args in daemon_pids(tmp)}, 20):
            findings.append(f"the peer control's daemon did not start (pid {peer.pid})")
        report = io.StringIO()
        verdict = audit(peer_guard, tmp, out=report)
        if verdict != 0:
            findings.append(f"a peer session's daemon must not fail this run's audit, got: {report.getvalue()}")
        if peer.pid not in {pid for pid, _args in daemon_pids(tmp / "ta-peer-root")}:
            findings.append("the audit stopped a peer session's daemon: an inherited marker, not a name or a "
                            "difference, is ownership")
            stop([peer.pid])
        if "another run's daemon" not in report.getvalue():
            findings.append(f"the audit must name a kept peer process and say why: {report.getvalue().strip()}")
        if "no leak from this run" not in report.getvalue():
            findings.append(f"the audit must qualify a clean verdict while a peer daemon is live: "
                            f"{report.getvalue().strip()}")
        stop([peer.pid])
        with contextlib.suppress(subprocess.TimeoutExpired):
            peer.wait(timeout=10)

        # 6. the runner half: a suite leaks a `jobs-runner` as readily as a daemon (§6.2/D-226), and the same
        #    machine-wide signal used to stop it. Owned one is a leak and is stopped…
        runner_guard = tmp / "runner-guard.json"
        snapshot(runner_guard, tmp)
        owned_runner = _start_runner(tmp / "ta-jobs-owned")
        started.append(owned_runner.pid)
        if not wait_until(lambda: owned_runner.pid in {pid for pid, _args in runner_pids(tmp)}, 20):
            findings.append(f"the owned-runner control did not start (pid {owned_runner.pid})")
        report = io.StringIO()
        if audit(runner_guard, tmp, out=report) != 1:
            findings.append(f"this run's leaked command runner must audit as a leak, got: {report.getvalue()}")
        if owned_runner.pid in {pid for pid, _args in runner_pids(tmp)}:
            findings.append("the audit did not stop the command runner it reported")
            stop([owned_runner.pid], finder=runner_pids)
        with contextlib.suppress(subprocess.TimeoutExpired):
            owned_runner.wait(timeout=10)

        # …and a peer session's runner is not, even though it is a *new* `jobs-runner` under the same `TMPDIR`
        # with the same binary. This is the exact shape of the field failure.
        peer_runner_guard = tmp / "peer-runner-guard.json"
        snapshot(peer_runner_guard, tmp)
        peer_runner = _start_runner(tmp / "ta-jobs-peer", marker=f"peer-{uuid.uuid4()}")
        started.append(peer_runner.pid)
        if not wait_until(lambda: peer_runner.pid in {pid for pid, _args in runner_pids(tmp)}, 20):
            findings.append(f"the peer-runner control did not start (pid {peer_runner.pid})")
        report = io.StringIO()
        if audit(peer_runner_guard, tmp, out=report) != 0:
            findings.append(f"a peer session's command runner must not fail this run's audit, got: "
                            f"{report.getvalue()}")
        if peer_runner.pid not in {pid for pid, _args in runner_pids(tmp)}:
            findings.append("the audit stopped a peer session's command runner (the field failure)")
        if "another run's command runner" not in report.getvalue():
            findings.append(f"the audit must name the kept peer runner: {report.getvalue().strip()}")
        stop([peer_runner.pid], finder=runner_pids)
        with contextlib.suppress(subprocess.TimeoutExpired):
            peer_runner.wait(timeout=10)

        # 7. reparenting: a leaked child that detached itself (`setsid --fork`, the shape both the daemon and the
        #    runner take) has no ancestor left to attribute it to, so only its inherited environment can say whose
        #    it is — and it must still be caught and stopped. The parent assertion keeps this control honest: a
        #    process that merely stayed this check's child would not test reparenting at all.
        forked_guard = tmp / "forked-guard.json"
        snapshot(forked_guard, tmp)
        forked_root = tmp / "ta-forked-child"
        subprocess.run(["setsid", "--fork", str(BIN), "daemon", "--state-root", str(forked_root)],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=dict(os.environ), timeout=30)
        forked = []
        if wait_until(lambda: bool(daemon_pids(forked_root)), 20):
            forked = [pid for pid, _args in daemon_pids(forked_root)]
            started.extend(forked)
        if not forked:
            findings.append("the reparenting control's daemon did not start")
        else:
            parent = pathlib.Path(f"/proc/{forked[0]}/stat").read_text().split()[3]
            if int(parent) == os.getpid():
                findings.append(f"the reparenting control is not reparented; its parent is this check ({forked})")
            report = io.StringIO()
            if audit(forked_guard, tmp, out=report) != 1:
                findings.append(f"a reparented daemon carrying this run's marker must still be a leak, got: "
                                f"{report.getvalue().strip()}")
            if daemon_pids(forked_root):
                findings.append("the audit did not stop the reparented owned daemon")
                stop([pid for pid, _args in daemon_pids(forked_root)])

        # 8. and when ownership cannot be determined, nothing is signalled (D-528). A process that clears its
        #    environment (`env -i`) carries no marker, so it is indistinguishable from a peer: it is reported and
        #    kept. This is a *documented coverage limit*, not a pass: the rule under-claims rather than killing a
        #    process it cannot attribute. A process that vanished mid-audit is `unattributed` for the same reason.
        cleared_guard = tmp / "cleared-guard.json"
        snapshot(cleared_guard, tmp)
        cleared_root = tmp / "ta-cleared-env"
        cleared_root.mkdir()
        cleared = subprocess.Popen(["/usr/bin/env", "-i", f"DEEPSEEK_API_KEY={PLACEHOLDER_KEY}",
                                    f"XDG_CONFIG_HOME={tmp / 'config'}",
                                    str(BIN), "daemon", "--state-root", str(cleared_root)],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        started.append(cleared.pid)
        if not wait_until(lambda: bool(daemon_pids(cleared_root)), 20):
            findings.append("the cleared-environment control's daemon did not start")
        else:
            pid = daemon_pids(cleared_root)[0][0]
            started.append(pid)
            if carries_marker(pid, os.environ[RUN_MARKER]):
                findings.append("a process started with `env -i` must not appear to carry the run marker")
            report = io.StringIO()
            if audit(cleared_guard, tmp, out=report) != 0:
                findings.append(f"an unattributable process must be reported and kept, not signalled: "
                                f"{report.getvalue().strip()}")
            if not daemon_pids(cleared_root):
                findings.append("the audit signalled a process whose ownership it could not determine")
            stop([p for p, _args in daemon_pids(cleared_root)])

        # 8b. the unattributed path itself: a pid the kernel has forgotten has no environment to read, which is
        #     "unknown owner", never "this run's".
        if carries_marker(2 ** 22 + 12345, os.environ[RUN_MARKER]):
            findings.append("a vanished pid must not report the run marker")

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

        # 9. nothing this check did is still running
        if daemon_pids(tmp) or runner_pids(tmp):
            findings.append(f"the self-check left processes behind: daemons={daemon_pids(tmp)}, runners={runner_pids(tmp)}")
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
