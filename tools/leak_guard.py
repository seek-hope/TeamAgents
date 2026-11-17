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
import ctypes
import io
import json
import os
import pathlib
import platform
import re
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

# A process carries its gate in its environment; a directory cannot, and a prefix is not ownership either. The
# shared test helper (`test_scratch_owner.rs`) appends each root it creates to `$TMPDIR/.ta-leak-owner-<marker>`, and
# this audit reads that registry to tell this gate's leftover from a peer's. A directory no registry names is
# **unattributed**: reported and kept, never called this run's leftover and never called a stranger's — no record
# does not prove a foreign owner, only that ownership was not established (D-528).
REGISTRY_PREFIX = ".ta-leak-owner-"
# How long a gate's registry is kept for a later reader. A day is far longer than any gate and far shorter than a
# machine's lifetime, so only abandoned lists are dropped.
REGISTRY_LIFETIME_SECONDS = 24 * 60 * 60

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
    which is what `make test`'s guard checked too. The `args` column of `ps` begins with the binary's *path*, so a
    prefix test such as `args.startswith("daemon ")` matches nothing at all — measured 2026-09-26, after the sweep
    had been silently doing nothing while the guard reported a leak it could not stop (D-144).
    """
    return [(pid, args) for pid, _identity, args in _selected(subcommand, root)]


UNKNOWN_IDENTITY = -1   # the start time could not be read: fail closed rather than skip the check


def _selected(subcommand: str, root: pathlib.Path | None = None) -> list[tuple[int, int, str]]:
    """`(pid, identity, args)` for this family, with the identity read *around* the metadata for that pid.

    The `ps` line and the `/proc/<pid>/stat` read are two moments, so a pid number reused between them would pair
    one process's arguments with another process's start time. Whenever that pair cannot be established the entry
    carries `UNKNOWN_IDENTITY`, and [`acquire`] then refuses it: an incomplete identity never disables the check.
    """
    listing = subprocess.run(["ps", "-eo", "pid,stat,comm,args"], capture_output=True, text=True).stdout
    found = []
    for line in listing.splitlines()[1:]:
        parts = line.split(None, 3)
        if len(parts) != 4 or not is_running(parts[1]) or parts[2] != "teamagents":
            continue
        try:
            if os.stat(f"/proc/{parts[0]}").st_uid != os.geteuid():
                continue
        except OSError:
            continue
        if parts[3].split()[1:2] != [subcommand]:
            continue
        if root is None or (lambda served: served and _under(served, str(root)))(_served_path(parts[3])):
            pid = int(parts[0])
            # re-read the arguments after the identity: if the process changed between the two reads, the pair is
            # inconsistent and the entry is unusable
            identity = start_identity(pid)
            try:
                after = pathlib.Path(f"/proc/{pid}/cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace")
            except OSError:
                after = ""
            if identity is None or (after and after.split()[1:2] != [subcommand]):
                identity = UNKNOWN_IDENTITY
            found.append((pid, identity, parts[3]))
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
    # The start identity travels with the pid, captured in the same selection that produced the arguments: a
    # baseline of bare pid numbers would hide a new process that reused a peer's number, and an identity read later
    # could describe a different process than the arguments record.
    return {"daemons": [list(row) for row in _selected("daemon", root)],
            "runners": [list(row) for row in _selected("jobs-runner", root)],
            "strays": sorted(strays(root))}


# The kernel's `pidfd_open(2)` / `pidfd_send_signal(2)` numbers, needed only when libc does not export the
# wrappers. They are never used silently on an unlisted architecture: the mapping below is the whole allowlist, and
# anything else is reported `unavailable` instead of calling a number that means something else there.
PIDFD_SYSCALLS = {"x86_64": (434, 424), "aarch64": (434, 424)}

# The four answers a handle can give, and only one of them may ever be read as "this process is gone". Treating
# ENOSYS/EPERM/EMFILE as ESRCH would turn "we could not look" into "it is already stopped" — a false stop receipt,
# which is the one thing this module must never produce (D-528).
PIDFD_OK = "ok"
PIDFD_GONE = "gone"                # ESRCH: the kernel says that pid does not exist
PIDFD_REFUSED = "refused"          # the pid exists but is not the process we decided about
PIDFD_UNAVAILABLE = "unavailable"  # we cannot obtain or use an identity-bound handle at all

# errno values that mean "the pid is gone" versus "we could not ask". Anything unlisted is `unavailable`, the
# conservative side.
GONE_ERRNOS = (3,)                      # ESRCH; every other failure is "unavailable", never "gone"


_LIBC = None      # the process's own libc handle, loaded at most once


def _libc():
    """This process's `libc` handle, or `None` when it cannot be loaded at all."""
    global _LIBC
    if _LIBC is None:
        try:
            _LIBC = ctypes.CDLL(None, use_errno=True)
        except OSError:
            _LIBC = False
    return _LIBC or None


# The per-direction entry points, in preference order: the interpreter's own API, then libc's symbol, then an
# allowlisted syscall number. Each direction is resolved on its own: having `pidfd_open` says nothing about
# `pidfd_send_signal`, and calling the second because the first exists was a real bug shape here.
PIDFD_API = {0: "pidfd_open", 1: "pidfd_send_signal"}


def _pidfd_number(direction: int):
    """`(kind, value)` for opening (0) or sending (1), each checked on its own."""
    name = PIDFD_API[direction]
    native = getattr(os, name, None)
    if native is not None:
        return ("native", native)
    libc = _libc()
    if libc is not None and getattr(libc, name, None) is not None:
        return ("libc", libc)
    mapping = PIDFD_SYSCALLS.get(platform.machine())
    if libc is not None and mapping:
        return ("syscall", (libc, mapping[direction]))
    return ("none", None)


def _pidfd_open(pid: int):
    """`(fd, state)`: an identity-bound handle on `pid`, or the reason there is none.

    A pidfd is the kernel's own reference to *one* process: once open it cannot be redirected to another, so the
    pid-number reuse window between "we decided this process is ours" and "we signal it" is closed. Opening is
    reported apart from existence — `PIDFD_GONE` only for ESRCH, `PIDFD_UNAVAILABLE` for every inability to ask
    (missing symbol, unlisted architecture, EPERM, EMFILE, ENOSYS …), so a failure to obtain a handle can never be
    read as evidence that a process stopped.
    """
    kind, handle = _pidfd_number(0)
    if kind == "native":
        try:
            fd = handle(int(pid))
        except OSError as error:
            return None, (PIDFD_GONE if error.errno in GONE_ERRNOS else PIDFD_UNAVAILABLE)
    elif kind == "libc":
        fd = handle.pidfd_open(int(pid), 0)
    elif kind == "syscall":
        libc, number = handle
        fd = libc.syscall(number, int(pid), 0)
    else:
        return None, PIDFD_UNAVAILABLE
    if fd >= 0:
        return fd, PIDFD_OK
    errno_value = ctypes.get_errno()
    return None, (PIDFD_GONE if errno_value in GONE_ERRNOS else PIDFD_UNAVAILABLE)


def _pidfd_signal(fd: int, sig: int) -> str:
    """Send `sig` through a pidfd: `PIDFD_OK`, `PIDFD_GONE`, or `PIDFD_UNAVAILABLE`."""
    kind, handle = _pidfd_number(1)
    if kind == "native":
        try:
            handle(int(fd), int(sig))
        except OSError as error:
            return PIDFD_GONE if error.errno in GONE_ERRNOS else PIDFD_UNAVAILABLE
        return PIDFD_OK
    elif kind == "libc":
        sent = handle.pidfd_send_signal(int(fd), int(sig), None, 0)
    elif kind == "syscall":
        libc, number = handle
        sent = libc.syscall(number, int(fd), int(sig), None, 0)
    else:
        return PIDFD_UNAVAILABLE
    if sent == 0:
        return PIDFD_OK
    return PIDFD_GONE if ctypes.get_errno() in GONE_ERRNOS else PIDFD_UNAVAILABLE


def _pidfd_exited(fd: int) -> bool:
    """Has the process behind this pidfd exited? A pidfd becomes readable exactly when it does."""
    import select
    try:
        return bool(select.select([fd], [], [], 0)[0])
    except OSError:
        return False   # cannot tell: never report "exited" from a failed probe


def _close(fd: int) -> None:
    try:
        os.close(fd)
    except OSError:
        pass


def _under(served: str, root: str) -> bool:
    """Is `served` the root itself or a directory beneath it? Whole-path containment, never a substring."""
    root = root.rstrip("/")
    return served == root or served.startswith(root + "/")


def _serves(args: str, root: str) -> bool:
    """Does this command line *name* `root` as the directory it serves? Whole-argument equality, not a substring.

    A substring test would let `/a/ta-x` match `/a/ta-x2`, so a process serving a sibling directory could be taken
    for one serving ours — the same reason `state_root_of` parses the argument instead of searching the text.
    """
    parts = args.split()
    if parts[1:2] == ["jobs-runner"]:
        return len(parts) > 2 and parts[2] == root
    for index, part in enumerate(parts):
        if part == "--state-root" and index + 1 < len(parts):
            return parts[index + 1] == root
        if part.startswith("--state-root="):
            return part.split("=", 1)[1] == root
    return False


def start_identity(pid: int):
    """`/proc/<pid>/stat` field 22 — the kernel's start time for that pid — or `None` when unreadable.

    This is the same anchor the product uses (`jobs::start_ticks`): a pid number alone never identifies a process,
    but a pid number *plus* its start time pins one. A reused number almost always has a different start time, so
    re-reading it around `acquire` rejects a stranger even on the unmarked legacy path.
    """
    try:
        stat = pathlib.Path(f"/proc/{pid}/stat").read_text(errors="replace")
    except OSError:
        return None
    # the comm field may contain spaces and parentheses: split after the last ')'
    fields = stat[stat.rfind(")") + 2:].split()
    try:
        return int(fields[19])   # field 22 overall; 20th of the post-comm fields
    except (IndexError, ValueError):
        return None


def acquire(pid: int, finder=daemon_pids, marker: str = "", expected_identity=None, served: str = ""):
    """`(pidfd, state)` for `pid`, but only while it is *still the process we decided about*.

    The decision and the signal are different moments, and a pid number is reused freely in between. The handle is
    opened first, then entitlement is re-checked on the process the handle actually names:

    * with a run marker: its own environment must still carry `RUN_MARKER=marker`;
    * without one (a hand-run audit, an older probe): `finder` must still list it **and**, when the caller saved
      one, its start identity must be unchanged — a family name alone does not identify a process;
    * when `served` is given, its command line must still name that same root.

    Anything else is `PIDFD_REFUSED`, and **no signal is ever sent by pid number**: there is no bare-pid fallback,
    so a reused number cannot be reached from this module at all.
    """
    if expected_identity == UNKNOWN_IDENTITY:
        # The caller could not establish which process this was, so nothing here can be attributed to it.
        return None, PIDFD_REFUSED
    if expected_identity is not None and start_identity(pid) != expected_identity:
        return None, PIDFD_REFUSED
    fd, state = _pidfd_open(pid)
    if state != PIDFD_OK:
        return None, state
    if marker:
        entitled = carries_marker(pid, marker)
    else:
        # No marker: the only evidence is the caller's own selection. A same-family replacement is not ours, so the
        # saved identity is required to still match. A caller that saved none gets the family check alone, which is
        # exactly the weaker claim it made.
        entitled = (not finder or any(p == pid for p, _a in finder()))
    if entitled and expected_identity is not None:
        entitled = start_identity(pid) == expected_identity   # re-read around the open, both paths
    if entitled and served:
        entitled = any(p == pid and _serves(a, served) for p, a in finder()) if finder else True
    if not entitled:
        _close(fd)
        return None, PIDFD_REFUSED
    return fd, PIDFD_OK


def _alive(pids: list[int], finder=daemon_pids) -> list[int]:
    """Which of these pids are still live processes of that family — never a pattern, never a corpse (D-144)."""
    known = {pid for pid, _args in finder()}
    return [pid for pid in pids if pid in known]


def stop(pids: list[int], finder=daemon_pids, grace: float = 5.0, kill_grace: float = 10.0,
         marker: str = "", identities: dict | None = None, report: list | None = None,
         served: dict | None = None) -> list[int]:
    """Stop these processes: SIGTERM through an identity-bound handle, then SIGKILL for what is left.

    Signals go to a process, never to a pattern: `pgrep`/`pkill -f` match any command line that *contains* the
    string, so `daemon --state-root <root>` also matched the shells whose text mentioned it and killed two of
    this session's own shells (D-144). The kill is escalated because a daemon that ignores TERM outlives its
    caller: `crash.py`'s daemon was still listed ~20 s after its probe exited (measured 2026-09-26, D-141).

    **Identity is the pidfd, not the pid number** (D-528). The handle is opened while the pid is still the one we
    decided about — start identity, ownership marker and served root are re-checked *around* that open
    ([`acquire`]) — and TERM and the escalated KILL both go through that same handle. `os.kill` is never called
    with a bare pid here, so a number reused after we decided cannot be reached.

    Returns every pid that is **not proven stopped**: still alive through its handle, refused because it was no
    longer the process we decided about, or unknown because no handle could be obtained. An inability to look
    (ENOSYS, EPERM, EMFILE, an unlisted architecture) is reported as unknown, never as "gone": a false stop receipt
    is the one outcome this module must not produce.
    """
    handles: dict[int, int] = {}
    unresolved: list[int] = []
    wanted = identities or {}
    outcomes: dict[int, dict] = {}
    roots = served or {}
    for pid in pids:
        fd, state = acquire(pid, finder, marker, expected_identity=wanted.get(pid), served=roots.get(pid, ""))
        if state == PIDFD_GONE:
            outcomes[pid] = {"state": PIDFD_GONE}
            continue                      # ESRCH: the kernel says that pid no longer exists
        if fd is None:                    # refused, or unavailable: not proven stopped
            unresolved.append(pid)
            outcomes[pid] = {"state": state}
            continue
        handles[pid] = fd
        term = _pidfd_signal(fd, signal.SIGTERM)
        outcomes[pid] = {"state": PIDFD_OK, "term": term}
        if term == PIDFD_UNAVAILABLE:
            unresolved.append(pid)

    def pending() -> bool:
        return any(not _pidfd_exited(fd) for fd in handles.values())

    deadline = time.time() + grace
    while time.time() < deadline and pending():
        time.sleep(0.25)
    for pid, fd in handles.items():
        if not _pidfd_exited(fd):
            outcomes[pid]["kill"] = _pidfd_signal(fd, signal.SIGKILL)   # same handle: identity cannot change
    deadline = time.time() + kill_grace
    while time.time() < deadline and pending():
        time.sleep(0.25)
    survivors = []
    for pid, fd in handles.items():
        exited = _pidfd_exited(fd)
        outcomes[pid]["exited"] = exited
        if not exited:
            survivors.append(pid)
    for fd in handles.values():
        _close(fd)
    if report is not None:
        report.extend({"pid": pid, **outcomes.get(pid, {})} for pid in pids)
    return survivors + unresolved


def kill(pids: list[int], finder=daemon_pids, grace: float = 10.0, marker: str = "",
         identities: dict | None = None, served: dict | None = None) -> list[int]:
    """SIGKILL these processes and return those not proven stopped: for a probe that deliberately *crashes* one.

    Not the same thing as `stop`: since D-150 the daemon handles SIGTERM and shuts itself down gracefully, so a
    probe that measures what a *crash* leaves behind (`crash.py`, `unknown_outcome.py`) must send the signal that
    has no handler. Signals still go through an identity-bound handle, never to a pattern or a bare number
    (D-144, D-528), so a reused pid number cannot be hit here either, and an unopenable handle is `unresolved`
    rather than a crash this probe can claim to have produced.
    """
    handles: dict[int, int] = {}
    unresolved: list[int] = []
    wanted = identities or {}
    roots = served or {}
    for pid in pids:
        fd, state = acquire(pid, finder, marker, expected_identity=wanted.get(pid), served=roots.get(pid, ""))
        if state == PIDFD_GONE:
            continue
        if fd is None:
            unresolved.append(pid)
            continue
        handles[pid] = fd
        if _pidfd_signal(fd, signal.SIGKILL) == PIDFD_UNAVAILABLE:
            unresolved.append(pid)

    def pending() -> bool:
        return any(not _pidfd_exited(fd) for fd in handles.values())

    deadline = time.time() + grace
    while time.time() < deadline and pending():
        time.sleep(0.25)
    survivors = [pid for pid, fd in handles.items() if not _pidfd_exited(fd)]
    for fd in handles.values():
        _close(fd)
    return survivors + unresolved


def _bind(family: str, root: pathlib.Path | None):
    """`(pids, identities, finder)` for one family at one root, captured once, for a stop/kill call.

    The wrappers below used to hand `stop`/`kill` a bare `[pid]`, which left the identity map and the root-scoped
    finder unused: a pid number reused between selection and signal was reachable. Binding the three together here
    means the legacy entry points carry the same protection as `audit`, with the same return shape.
    """
    rows = _selected(family, root)
    pids = [pid for pid, _identity, _args in rows]
    identities = {pid: identity for pid, identity, _args in rows}
    finder = (lambda: _pids(family, root))
    return pids, identities, finder


def kill_daemons(root: pathlib.Path) -> list[int]:
    """Crash the live daemons serving `root`: SIGKILL, no shutdown path (D-150)."""
    pids, identities, finder = _bind("daemon", root)
    return kill(pids, finder=finder, identities=identities)


def kill_runners(root: pathlib.Path) -> list[int]:
    """Crash the live command runners serving `root`: SIGKILL, no shutdown path (D-150)."""
    pids, identities, finder = _bind("jobs-runner", root)
    return kill(pids, finder=finder, identities=identities)


def stop_daemons(root: pathlib.Path) -> list[int]:
    """Stop the live daemons serving `root` (or all of them): what every probe registers with `atexit`.

    Each probe used to run `pkill -f "daemon --state-root <root>"` here, the hazard D-144 measured: `pkill -f`
    matches any command line that *contains* the string, so it also matched the shells whose text mentioned it
    and killed two of this session's own shells (D-148). Identity and the root-scoped finder are bound at
    selection, so the same pid number cannot be re-aimed at a replacement process.
    """
    pids, identities, finder = _bind("daemon", root)
    return stop(pids, finder=finder, identities=identities)


def stop_runners(root: pathlib.Path) -> list[int]:
    """Stop the live command runners serving `root` (or all of them): `crash.py` and `unknown_outcome.py`."""
    pids, identities, finder = _bind("jobs-runner", root)
    return stop(pids, finder=finder, identities=identities)


def snapshot(path: pathlib.Path, root: pathlib.Path | None = None) -> None:
    """Write down what the guards see, and bind this gate's registry, so a later audit can attribute a difference.

    Binding is what makes a lost registry *detectable*: the gate creates its own registry file here, so at audit
    time a missing one means the record was destroyed rather than "nothing was ever recorded". A gate that cannot
    create it fails here, before the suites run, instead of producing an unattributable audit later.
    """
    observed = observe(root)
    marker = run_marker()
    if marker:
        registry = registry_path(marker)
        try:
            registry.parent.mkdir(parents=True, exist_ok=True)
            # A fresh gate starts with a fresh name list: this also bounds the file's growth to one gate.
            registry.write_text("")
            # And a registry a day old belongs to a gate that is not running: its roots are long gone, and keeping
            # every gate's list forever would be this rule's own small leak. Only files this rule created.
            cutoff = time.time() - REGISTRY_LIFETIME_SECONDS
            for stale in registry.parent.glob(f"{REGISTRY_PREFIX}*"):
                with contextlib.suppress(OSError):
                    if stale.is_file() and stale.name != registry.name and stale.stat().st_mtime < cutoff:
                        stale.unlink()
        except OSError as error:
            raise RuntimeError(f"the gate marker is set but its registry {registry} could not be created: {error}")
        observed["registry"] = str(registry)
    pathlib.Path(path).write_text(json.dumps(observed, indent=1) + "\n")


def _served_path(args: str) -> str:
    """The directory a `daemon` or `jobs-runner` command line serves: its state root, or its job directory."""
    parts = args.split()
    if parts[1:2] == ["jobs-runner"]:
        return parts[2] if len(parts) > 2 else ""
    return state_root_of(args) if parts[1:2] == ["daemon"] else ""


def registry_path(marker: str) -> pathlib.Path:
    """This gate's registry: `$TMPDIR/.ta-leak-owner-<marker>`."""
    return pathlib.Path(os.environ.get("TMPDIR", "/tmp")) / f"{REGISTRY_PREFIX}{marker}"


def read_registry(marker: str):
    """`(roots, problem)`: the roots this gate recorded, or why that could not be established.

    `snapshot` creates the registry for a marked gate, so at audit time the file is expected to exist. An empty set
    is a fact only when the read succeeded; a missing, unreadable or malformed registry is a `problem`, because the
    fixtures may have recorded owned roots and reporting "no roots" would turn lost evidence into a clean verdict.
    """
    if not marker:
        return set(), None
    path = registry_path(marker)
    try:
        raw = path.read_bytes()
    except FileNotFoundError:
        return set(), f"missing ({path}): snapshot should have created it"
    except OSError as error:
        return set(), f"unreadable ({path}): {error}"
    if b"\0" in raw:
        return set(), f"malformed ({path}): embedded NUL byte"
    lines = [line for line in raw.split(b"\n") if line.strip()]
    bad = [line for line in lines if not line.startswith(b"/")]
    if bad:
        return set(), f"malformed ({path}): {len(bad)} record(s) are not absolute paths"
    return {line.decode("utf-8", "replace") for line in lines}, None


def registry_roots(marker: str) -> set[str]:
    """The roots this gate recorded, treating any read failure as empty (see [`read_registry`] for the diagnosis)."""
    return read_registry(marker)[0]


def registered_elsewhere(stray: str, marker: str) -> bool:
    """Is `stray` in some *other* gate's registry? Then it is positively another run's root, not ours."""
    base = pathlib.Path(os.environ.get("TMPDIR", "/tmp"))
    for path in base.glob(f"{REGISTRY_PREFIX}*"):
        if path.name == f"{REGISTRY_PREFIX}{marker}":
            continue
        try:
            if stray in {line.strip() for line in path.read_text(errors="replace").splitlines()}:
                return True
        except OSError:
            continue
    return False


def served_by_marked(stray: str, daemons: list, runners: list, marker: str) -> bool:
    """Is a process that carries this run's marker serving `stray`? Then the directory is this run's, whoever wrote
    it: a suite that crashed between `mkdir` and its owner record still leaves the daemon we can interrogate."""
    if not marker:
        return False
    for pid, _identity, args in _rows(list(daemons) + list(runners)):
        served = _served_path(args)
        if served and (served == stray or served.startswith(stray.rstrip("/") + "/")) and carries_marker(pid, marker):
            return True
    return False


def _rows(entries: list) -> list[tuple[int, int | None, str]]:
    """Normalise a snapshot/observation row to `(pid, identity, args)`, accepting the older two-field shape."""
    out = []
    for row in entries:
        if len(row) == 3:
            out.append((row[0], row[1], row[2]))
        elif len(row) == 2:
            out.append((row[0], None, row[1]))
    return out


def in_use(stray: str, daemons: list, runners: list) -> bool:
    """Is this scratch-looking directory being *used* by a live process right now?

    The stray rule's job is "a test left its temporary directory behind" (D-131), and a directory a live daemon or
    runner is serving has not been left behind: it is that process's state root or job directory. This matters for
    a concurrent session (D-528): its roots carry the same `ta-*` / `teamagents-*` names as our own, and a name
    cannot say whose it is — but "in use" can, without pretending to know the owner. It also keeps this audit from
    reporting a peer's *live* directory as our leak, while still reporting a directory whose processes are gone.
    """
    for _pid, _identity, args in _rows(list(daemons) + list(runners)):
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

    # A baseline of bare pid numbers hides a *new* process that reused an earlier peer's number, so a process counts
    # as known only when both its pid and the start identity recorded with it still match.
    def known(entry, baseline) -> bool:
        pid, identity, _args = entry
        for row in _rows(baseline):
            if row[0] == pid and (identity is None or row[1] is None or row[1] == identity):
                return True
        return False

    new_daemons = [e for e in _rows(now["daemons"]) if not known(e, before["daemons"])]
    new_runners = [e for e in _rows(now["runners"]) if not known(e, before.get("runners", []))]

    def split(entries):
        """`(owned, foreign, unattributed)` for new processes — by inherited environment, never by name.

        Absence of the marker is *not* evidence of a foreign owner: a cleared environment (`env -i`), an unreadable
        `/proc`, and an older build with no marker all look alike from here, so they are `unattributed`. `foreign`
        means positively a different run's marker.
        """
        owned, foreign, unattributed = [], [], []
        for pid, identity, args in entries:
            if not marker:
                owned.append((pid, identity, args))   # no marker set: the caller keeps the machine-wide rule
                continue
            try:
                raw = pathlib.Path(f"/proc/{pid}/environ").read_bytes()
            except OSError:
                unattributed.append((pid, identity, args))
                continue
            if f"{RUN_MARKER}={marker}".encode() in raw.split(b"\0"):
                owned.append((pid, identity, args))
            elif any(field.startswith(f"{RUN_MARKER}=".encode()) for field in raw.split(b"\0")):
                foreign.append((pid, identity, args))
            else:
                unattributed.append((pid, identity, args))   # alive, readable, and simply not marked
        return owned, foreign, unattributed

    leaked, foreign_daemons, unattributed_daemons = split(new_daemons)
    leaked_runners, foreign_runners, unattributed_runners = split(new_runners)

    new_strays = sorted(set(now["strays"]) - set(before["strays"]))
    # A directory carries no environment, so ownership comes from its owner record (written by the shared test
    # helper) or from a marked process of this run serving it. Only `leaked` fails this run:
    #   leaked      - the record names this run, or (with no marker set) the old strict rule applies
    #   held        - a live process of this run is serving it: a directory in use was not left behind
    #   unattributed - no record, or another run's, and unserved: reported and kept, ownership not established
    leaked_strays, held, unattributed_strays = [], [], []
    recorded, registry_problem = read_registry(marker)
    # Fail closed only when this gate's snapshot *bound* a registry: then a missing or unreadable one means the
    # record was destroyed after fixtures may have written to it. A baseline with no binding is a legacy or
    # hand-written snapshot, whose caller never had a registry to begin with, so its directories are reported as
    # unattributed exactly as before — compatibility, not a silent clean verdict.
    bound = bool(before.get("registry"))
    if registry_problem and bound:
        # The fixtures may have recorded owned roots, so an unreadable registry is lost evidence, not "no roots":
        # say so and fail. Silently treating it as empty would report this run's own leaks as unattributed.
        print(f"FAIL: this run's scratch registry could not be read: {registry_problem}; "
              f"snapshot bound it, so {len(new_strays)} new scratch director(ies) cannot be attributed and lost "
              f"evidence is not a clean verdict", file=out)
        for stray in new_strays:
            print(f"  unattributed (registry lost): {stray}", file=out)
        return 1
    for stray in new_strays:
        if marker and stray in recorded:
            leaked_strays.append(stray)
        elif in_use(stray, now["daemons"], now["runners"]):
            held.append(stray)
        elif marker and served_by_marked(stray, now["daemons"], now["runners"], marker):
            held.append(stray)
        elif not marker:
            # No marker: no record can be checked, so this audit keeps its previous strict meaning rather than
            # silently disabling directory detection for every existing caller.
            leaked_strays.append(stray)
        else:
            unattributed_strays.append(stray)

    for pid, _identity, args in foreign_daemons + foreign_runners:
        kind = "command runner" if any(r[0] == pid for r in foreign_runners) else "daemon"
        print(f"another run's {kind} pid {pid} appeared during this run (serving "
              f"{_served_path(args) or 'an unnamed directory'}): kept, not signalled - it carries a different "
              f"{RUN_MARKER}, so it is a concurrent session's work", file=out)
    for pid, _identity, args in unattributed_daemons + unattributed_runners:
        print(f"process pid {pid} appeared during this run and its ownership was not established (no {RUN_MARKER} "
              f"of this run, or its environment could not be read): kept, not signalled - this is not evidence "
              f"that this run leaked it", file=out)
    for stray in held:
        print(f"scratch directory {stray} appeared during this run and a live process is serving it: kept, not "
              f"counted as this run's leftover - a directory in use has not been left behind", file=out)
    for stray in unattributed_strays:
        detail = ("another gate's registry names it" if registered_elsewhere(stray, marker)
                  else "no gate registry names it")
        print(f"scratch directory {stray} appeared during this run and ownership was not established: {detail}; "
              f"kept and not called this run's leftover - a prefix is a convention, not ownership", file=out)

    # Counts are stated for the window this audit actually saw: `now` is the current machine, not the baseline.
    kept_processes = (len(foreign_daemons) + len(foreign_runners) + len(unattributed_daemons)
                      + len(unattributed_runners))
    baseline = (f"{len(_rows(before['daemons']))} daemon(s) and {len(_rows(before.get('runners', [])))} runner(s) "
                f"in the baseline")
    if not leaked and not leaked_runners and not leaked_strays:
        if kept_processes or held or unattributed_strays:
            print(f"no leak from this run: {len(now['daemons'])} daemon(s), {len(now['runners'])} runner(s) and "
                  f"{len(now['strays'])} scratch directory(ies) are on the machine now against {baseline}; "
                  f"{kept_processes} process(es) of other or unestablished ownership, {len(held)} in-use "
                  f"directory(ies) and {len(unattributed_strays)} directory(ies) of unestablished ownership were "
                  f"reported above and left alone (marker {marker or 'unset: machine-wide rule'})", file=out)
        else:
            print(f"no leak: {len(now['daemons'])} daemon(s), {len(now['runners'])} runner(s) and "
                  f"{len(now['strays'])} scratch directory(ies) are on the machine now against {baseline}, with "
                  f"nothing new (marker {marker or 'unset: machine-wide rule'})", file=out)
        # The registry is left in place, passing or failing: it is one gate's name list, and an audit is not
        # necessarily the last reader of it (the self-check audits one snapshot repeatedly, a probe re-audits, a
        # gate can be retried). Removing it here would make the next audit of a healthy gate look like lost
        # evidence. Its lifetime is bounded at the other end, in `snapshot`.
        return 0

    def outcomes(entries, finder=daemon_pids) -> str:
        """Stop `entries` and describe what was *attempted* and *observed*, per the handles' own answers."""
        report: list = []
        result = stop([pid for pid, _i, _a in entries], finder=finder, marker=marker,
                      identities={pid: identity for pid, identity, _a in entries}, report=report,
                      served={pid: _served_path(args) for pid, _i, args in entries})
        lines = []
        served = {pid: _served_path(args) for pid, _i, args in entries}
        for row in report:
            pid, state = row["pid"], row["state"]
            where = served.get(pid) or "an unnamed directory"
            if state == PIDFD_GONE:
                lines.append(f"pid {pid} (serving {where}): already gone when the audit looked, nothing was signalled")
            elif state == PIDFD_REFUSED:
                lines.append(f"pid {pid} (serving {where}): not signalled - it was no longer the process this run selected")
            elif state == PIDFD_UNAVAILABLE:
                lines.append(f"pid {pid} (serving {where}): not signalled - no identity-bound handle could be "
                             f"obtained; its state is unresolved, not stopped")
            elif row.get("exited"):
                lines.append(f"pid {pid} (serving {where}): stopped - signals attempted through a pidfd, exit observed")
            else:
                lines.append(f"pid {pid} (serving {where}): survived - signalled through a pidfd, still alive")
        return "; ".join(lines) + ("" if not result else f" [unresolved: {sorted(set(result))}]")

    if leaked:
        print(f"leaked daemon(s) appeared during the run. {outcomes(leaked)}", file=out)
    if leaked_runners:
        print(f"leaked command runner(s) appeared during the run. "
              f"{outcomes(leaked_runners, runner_pids)}", file=out)
    for stray in leaked_strays:
        # kept on purpose: a leaked state root is the evidence of the test that wrote it (D-140)
        print(f"leaked scratch directory {stray} (kept: the directory is the evidence)", file=out)
    print(f"FAIL: {len(leaked)} daemon(s), {len(leaked_runners)} command runner(s) and {len(leaked_strays)} scratch "
          "directory(ies) appeared during the run; a test that starts a daemon or a runner must stop it and remove "
          f"its own temp directory (D-111, D-131, D-226)"
          + (f"; {kept_processes} process(es) of other or unestablished ownership were reported and left running "
             f"(D-528)" if kept_processes else ""), file=out)
    return 1


def audit_ownership() -> int:
    """Every guarded scratch constructor in this tree records its owner, or this audit says which one does not.

    The stray rule only stays honest if each `ta-*` / `teamagents-*` root a test creates can be attributed. Marking
    one constructor and leaving a raw one silently converts that constructor's leaked directory from "this run's
    leftover" into "unattributed", which is a coverage *regression* even though the audit still exits 0. So the
    coverage is checked as a source inventory rather than trusted: a guarded-prefix `temp_dir()` in test code must
    either use the shared helper or write the owner file itself.

    Reported as an inventory, not a guess: the count of constructors is printed, and the limit (prefixes are the
    ones [`SCRATCH_PREFIXES`] names; a test root outside them was never covered, before or after D-528) is stated.
    """
    helper = ("owned_temp_dir", "record_ownership")
    findings, covered, exempt = [], [], []
    for path in sorted(pathlib.Path(REPO).rglob("*.rs")):
        text_path = str(path.relative_to(REPO))
        if "/target/" in text_path or text_path.endswith("test_scratch_owner.rs"):
            continue
        lines = path.read_text(errors="replace").splitlines()
        for number, line in enumerate(lines, 1):
            if "temp_dir()" not in line or ".join(" not in line:
                continue
            if not any(f'"{prefix}' in line for prefix in SCRATCH_PREFIXES):
                continue
            where = f"{text_path}:{number}"
            # The stray rule counts *directories* only (D-143), so a constructor whose format ends in a name suffix
            # (`…-{}.db`, `…-{}.log`) builds a file: it was never a stray, before or after D-528.
            literal = re.search(r'join\(format!\("([^"]*)"', line)
            if literal and not literal.group(1).rstrip().endswith("}"):
                exempt.append(where)
                continue
            window = "\n".join(lines[number - 1:number + 9])
            if any(marker in window for marker in helper):
                covered.append(where)
            elif not _is_test_code(text_path, lines, number):
                exempt.append(where)          # a product path, not a test scratch root
            else:
                findings.append(f"{where} creates a guarded scratch root without an ownership record")
    print(f"guarded scratch roots: {len(covered)} record their owner, {len(exempt)} are product paths, "
          f"{len(findings)} do not")
    print(f"coverage boundary: prefixes {', '.join(SCRATCH_PREFIXES)} under TMPDIR; a root a test names outside "
          f"them was never covered by this rule, and plain files were never counted (D-143)")
    for finding in findings:
        print(f"FAIL: {finding}")
    return 1 if findings else 0


def _is_test_code(relative: str, lines: list[str], number: int) -> bool:
    """Is this line test code? An integration test file and a `*tests.rs` module are test code from line 1.

    Missing this distinction would exempt every raw constructor in `engine/tests/` — exactly the constructors the
    coverage inventory exists to keep visible — so the file's own path decides before the `#[cfg(test)]` search.
    """
    parts = pathlib.PurePosixPath(relative).parts
    if "tests" in parts[:-1] or relative.endswith(("tests.rs", "_tests.rs")):
        return True
    return _in_test_module(lines, number)


def _in_test_module(lines: list[str], number: int) -> bool:
    """Is line `number` inside a `#[cfg(test)]` module of this file?"""
    for index in range(number - 2, -1, -1):
        if lines[index].strip().startswith("#[cfg(test)]"):
            return True
    return False


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
    # the simulated controls below swap these temporarily; keep the real ones to restore
    _pidfd_signal_before, _pidfd_exited_before = _pidfd_signal, _pidfd_exited
    # pid -> start identity at the moment this check started it. Cleanup signals only a pid whose identity is
    # unchanged, so a number the kernel has already reused cannot be signalled after its process was reaped (D-528).
    identities: dict[int, int | None] = {}

    def remember(pid: int) -> int:
        identities.setdefault(pid, start_identity(pid))
        return pid
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
        started.append(remember(daemon.pid))
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
        # (i) a directory THIS gate's registry names, and that nothing serves, is this gate's leftover
        leftover = tmp / "ta-unserved-leftover"
        leftover.mkdir()
        (tmp / f"{REGISTRY_PREFIX}{os.environ[RUN_MARKER]}").write_text(f"{leftover}\n")
        report = io.StringIO()
        if audit(stray_guard, tmp, out=report) != 1:
            findings.append(f"a scratch directory this gate owns and no live process serves must audit as a leak, "
                            f"got: {report.getvalue().strip()}")
        if str(leftover) not in report.getvalue():
            findings.append(f"the leak report must name the stray directory: {report.getvalue().strip()}")
        shutil.rmtree(leftover, ignore_errors=True)

        # (ii) a *peer's* directory (named in another gate's registry), and one no registry names, are not this
        #      gate's leftovers: they are reported and kept, and must not fail this gate. A prefix plus "it appeared
        #      after the snapshot" is not ownership — a concurrent session creates the same names under the same TMPDIR.
        peer_registry = tmp / f"{REGISTRY_PREFIX}peer-some-other-run"
        for label, record in (("ta-peer-owned", peer_registry), ("ta-unrecorded", None)):
            peer_dir = tmp / label
            peer_dir.mkdir()
            if record is not None:
                record.write_text(f"{peer_dir}\n")
            report = io.StringIO()
            verdict = audit(stray_guard, tmp, out=report)
            if verdict != 0:
                findings.append(f"a directory this gate does not own ({label}) must not fail this gate's audit, "
                                f"got: {report.getvalue().strip()}")
            if str(peer_dir) not in report.getvalue() or "not established" not in report.getvalue():
                findings.append(f"the audit must report an unattributed directory and say ownership was not "
                                f"established: {report.getvalue().strip()}")
            if not peer_dir.is_dir():
                findings.append(f"the audit removed a directory it does not own ({label})")
            shutil.rmtree(peer_dir, ignore_errors=True)

        # 4c. a lost registry is lost evidence, not "no roots": the fixtures may have recorded owned roots, so the
        #     audit must fail rather than report this run's own leak as unattributed.
        for label, break_registry in (
            ("missing", lambda registry: registry.unlink()),
            ("unreadable", lambda registry: (registry.unlink(), registry.mkdir())),
            ("malformed", lambda registry: registry.write_text("not-an-absolute-path\n")),
        ):
            lost_guard = tmp / f"lost-guard-{label}.json"
            snapshot(lost_guard, tmp)
            registry = registry_path(os.environ[RUN_MARKER])
            break_registry(registry)
            stray = tmp / f"ta-owned-during-{label}"
            stray.mkdir()
            report = io.StringIO()
            if audit(lost_guard, tmp, out=report) != 1:
                findings.append(f"a {label} registry must fail the audit, not report a clean verdict: "
                                f"{report.getvalue().strip()}")
            if "registry could not be read" not in report.getvalue():
                findings.append(f"the audit must say the registry was lost ({label}): {report.getvalue().strip()}")
            if not stray.is_dir():
                findings.append(f"the audit removed a directory while its registry was {label}")
            shutil.rmtree(stray, ignore_errors=True)
            if registry.is_dir():
                registry.rmdir()
            # restore a healthy (empty) registry so the following controls read a bound gate, not this broken one
            registry.write_text("")

        # 4d. the legacy wrappers bind identity and a root-scoped finder, so a bare pid list cannot reach a
        #     replacement. Captured, not sent: the mocked stop/kill only records its arguments.
        captured = []
        real_stop, real_kill = globals()["stop"], globals()["kill"]
        try:
            globals()["stop"] = lambda pids, finder=None, **kw: captured.append(("stop", pids, kw, finder)) or []
            globals()["kill"] = lambda pids, finder=None, **kw: captured.append(("kill", pids, kw, finder)) or []
            stop_daemons(tmp)
            stop_runners(tmp)
            kill_daemons(tmp)
            kill_runners(tmp)
        finally:
            globals()["stop"], globals()["kill"] = real_stop, real_kill
        if len(captured) != 4:
            findings.append(f"the legacy wrappers must each stop/kill once, got {len(captured)} call(s)")
        for label, pids, kw, finder in captured:
            identities = kw.get("identities")
            if not identities and pids:
                findings.append(f"{label} was called without an identity map: a bare pid list could reach a "
                                f"replacement process")
            if finder is None:
                findings.append(f"{label} was called without a root-scoped finder")
            if pids and identities and set(identities) != set(pids):
                findings.append(f"{label} identity map does not cover its pids: {identities} vs {pids}")

        # 4e. an unknown saved identity fails closed: no handle is taken and nothing is signalled.
        sent_probe: list = []
        real_signal = globals()["_pidfd_signal"]
        try:
            globals()["_pidfd_signal"] = lambda fd, sig: sent_probe.append((fd, int(sig))) or PIDFD_OK
            if stop([424247], identities={424247: UNKNOWN_IDENTITY}) != [424247]:
                findings.append("an unknown saved identity must stay unresolved, not be treated as stopped")
            if sent_probe:
                findings.append(f"an unknown saved identity was signalled: {sent_probe}")
        finally:
            globals()["_pidfd_signal"] = real_signal

        # 5. a *peer session* is not this run's leak (D-528). Same user, same binary, same `/tmp` prefixes, a
        #    different run marker. Before the marker rule, `audit` reported this daemon and SIGTERM-then-SIGKILLed
        #    it, which is how a concurrent `make check` killed a live command runner and left the journal at
        #    RUNNING — the residue that blocks that session's `finish`. This is the control that fails before the
        #    fix and must keep failing if the rule is ever weakened back to a name or to a bare difference.
        peer_guard = tmp / "peer-guard.json"
        snapshot(peer_guard, tmp)
        peer = _start_daemon(tmp / "ta-peer-root", marker=f"peer-{uuid.uuid4()}")
        started.append(remember(peer.pid))
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
        started.append(remember(owned_runner.pid))
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
        started.append(remember(peer_runner.pid))
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
            started.extend(remember(pid) for pid in forked)
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
        started.append(remember(cleared.pid))
        if not wait_until(lambda: bool(daemon_pids(cleared_root)), 20):
            findings.append("the cleared-environment control's daemon did not start")
        else:
            pid = daemon_pids(cleared_root)[0][0]
            started.append(remember(pid))
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
        started.append(remember(corpse.pid))
        wait_until(lambda: corpse.pid in {pid for pid, _args in daemon_pids()}, 20)
        corpse.kill()
        if not wait_until(lambda: (process_state(corpse.pid) or "").startswith("Z"), 20):
            findings.append(f"the zombie control did not produce a zombie (stat {process_state(corpse.pid)!r})")
        elif corpse.pid in {pid for pid, _args in daemon_pids()}:
            findings.append("a zombie was counted as a live daemon")
        stop([corpse.pid])
        with contextlib.suppress(subprocess.TimeoutExpired):
            corpse.wait(timeout=10)

        # 10. simulated identity and availability controls (D-528). These capture signals instead of sending them,
        #     so a control can prove what *would* have been signalled without touching any real process — the
        #     supervisor's review script is the same idea, and this keeps it in the gate.
        sent: list[tuple[int, int]] = []

        def record_signal(fd, sig):
            sent.append((fd, int(sig)))
            return PIDFD_OK

        # 10a. identity changed between decision and signal: `stop` must not signal a stranger. The pid still exists
        #      (so this is not "gone"), but its start identity differs from the saved one.
        sent.clear()
        real_acquire, real_open = globals()["acquire"], globals()["_pidfd_open"]
        try:
            globals()["_pidfd_open"] = lambda pid: (99, PIDFD_OK)
            globals()["acquire"] = lambda pid, finder=daemon_pids, marker="", expected_identity=None, served="": (
                (None, PIDFD_REFUSED) if expected_identity is not None else (99, PIDFD_OK))
            globals()["_pidfd_signal"] = record_signal
            globals()["_pidfd_exited"] = lambda fd: False
            refused = stop([424242], identities={424242: 12345})
            if sent:
                findings.append(f"stop signalled a pid whose start identity changed: {sent}")
            if 424242 not in refused:
                findings.append(f"a refused identity must be reported unresolved, got {refused}")

            # 10b. the handle cannot be obtained at all (ENOSYS/EPERM/EMFILE shape): unknown, never "gone"
            sent.clear()
            globals()["acquire"] = lambda pid, finder=daemon_pids, marker="", expected_identity=None, served="": (
                None, PIDFD_UNAVAILABLE)
            unresolved = stop([424243])
            if 424243 not in unresolved:
                findings.append(f"an unavailable pidfd must be reported unresolved, not treated as gone, got "
                                f"{unresolved}")
            sent.clear()
            unresolved_kill = kill([424244])
            if 424244 not in unresolved_kill:
                findings.append(f"kill must report an unavailable pidfd as unresolved, got {unresolved_kill}")

            # 10c. a real ESRCH is the only state that means "already gone"
            globals()["_pidfd_open"] = lambda pid: (None, PIDFD_GONE)
            globals()["acquire"] = lambda pid, finder=daemon_pids, marker="", expected_identity=None, served="": (
                None, PIDFD_GONE)
            if stop([424245]) or kill([424246]):
                findings.append("a pid the kernel reports as gone must not be listed as a survivor")
        finally:
            globals()["acquire"], globals()["_pidfd_open"] = real_acquire, real_open
            globals()["_pidfd_signal"] = _pidfd_signal_before
            globals()["_pidfd_exited"] = _pidfd_exited_before

        # 10d. the real path with real, disposable processes this check owns, both directions:
        #      a *wrong* saved start identity is refused and the process must survive untouched,
        #      the *correct* one is signalled and the process must die.
        owned_env = dict(os.environ, **{RUN_MARKER: os.environ[RUN_MARKER]})
        sleeper = subprocess.Popen(["/bin/sh", "-c", "sleep 60"], env=owned_env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        started.append(remember(sleeper.pid))
        if not wait_until(lambda: start_identity(sleeper.pid) is not None, 10):
            findings.append("the identity control's process has no start identity")
        else:
            truth = start_identity(sleeper.pid)
            refused = stop([sleeper.pid], marker=os.environ[RUN_MARKER], grace=1, kill_grace=1,
                           identities={sleeper.pid: truth + 1})
            if sleeper.poll() is not None:
                findings.append("a wrong saved start identity was signalled: the owned process died anyway")
            if sleeper.pid not in refused:
                findings.append(f"a refused identity must be reported unresolved through the real path, got "
                                f"{refused}")
            signalled = stop([sleeper.pid], marker=os.environ[RUN_MARKER], grace=3, kill_grace=5,
                             identities={sleeper.pid: truth})
            if sleeper.poll() is None:
                findings.append("a matching start identity was not signalled through the real path")
            if signalled:
                findings.append(f"a stopped owned process must not be reported unresolved, got {signalled}")

        # 10e. the escalated KILL, through the same handle: a process that ignores TERM must still be stopped.
        stubborn = subprocess.Popen(["/bin/sh", "-c", 'trap "" TERM; sleep 60'], env=owned_env,
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        started.append(remember(stubborn.pid))
        if not wait_until(lambda: start_identity(stubborn.pid) is not None, 10):
            findings.append("the escalation control's process has no start identity")
        else:
            survived = stop([stubborn.pid], marker=os.environ[RUN_MARKER], grace=1, kill_grace=8,
                            identities={stubborn.pid: start_identity(stubborn.pid)})
            if stubborn.poll() is None:
                findings.append("a TERM-ignoring owned process survived the KILL escalation")
            if survived:
                findings.append(f"an escalated owned process must not be reported unresolved, got {survived}")

        # 10f. a pid that never existed is the one state that reads `gone`
        if _pidfd_open(2 ** 22 + 998877)[1] != PIDFD_GONE:
            findings.append("a nonexistent pid must be reported gone through the real pidfd path")

        # 9. nothing this check did is still running
        if daemon_pids(tmp) or runner_pids(tmp):
            findings.append(f"the self-check left processes behind: daemons={daemon_pids(tmp)}, runners={runner_pids(tmp)}")
    finally:
        # The guard's own rule, applied to the guard: a control that fails must not leave a process behind. The
        # cleanup is identity-bound like everything else — each pid is stopped only while its start identity is
        # still the one recorded when this check started it — so a number the kernel reused after a control already
        # reaped its child is refused rather than signalled, and a stale numeric pid is never re-acquired (D-528).
        stop(list(dict.fromkeys(started)), identities=identities)
        _restore_environment(keep)
        shutil.rmtree(tmp, ignore_errors=True)

    # 11. the coverage inventory: every guarded scratch constructor in THIS tree records its owner (D-528). The
    #     stray rule only stays honest if each root a test makes can be attributed; marking one constructor and
    #     leaving a raw one would silently turn that constructor's leaked directory into "unattributed".
    if audit_ownership() != 0:
        findings.append("a guarded test scratch root does not record its owning gate (see above)")

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
    parser.add_argument("--audit-ownership", action="store_true",
                        help="check that every guarded test scratch root records its owning gate")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if args.audit_ownership:
        return audit_ownership()
    if not args.verb or not args.file:
        parser.error("snapshot FILE or audit FILE (or --self-check)")
    if args.verb == "snapshot":
        snapshot(pathlib.Path(args.file))
        return 0
    return audit(pathlib.Path(args.file))


if __name__ == "__main__":
    sys.exit(main())
