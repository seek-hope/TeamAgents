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
import ast
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
HERE = REPO / "review/dogfood"

sys.path.insert(0, str(REPO / "review"))   # the shared guard, also used by `make test` (D-147)
import leak_guard  # noqa: E402

# (file, extra arguments, why it is in this set) — the credential-free set: no model, no credential
OFFLINE = [
    ("boundary.py", [], "the state roots the CLI refuses (A33/A34)"),
    ("budget.py", [], "a ceiling below one request is refused before any model call (A18)"),
    ("truncation.py", [], "A19's whole path over a real socket: truncated before output retries, after it fails"),
    ("max_retries.py", [], "a profile's `max_retries` is ignored: the budget is the session's constant (D-240)"),
    ("input_latency.py", [], "per-keystroke composer latency against the scripted daemon"),
    ("tui_panels.py", [], "the instances panel's keys against a real daemon (D-95)"),
    ("geometry.py", [], "the TUI under an extreme terminal size and a mid-run resize (D-233)"),
    ("tui_reconnect.py", [], "the TUI through a daemon kill and restart (A28, D-99)"),
    ("providers.py", ["--self-check"], "the A27 probe's task-result rule, without a model"),
    ("shutdown.py", [], "a graceful stop with a command in flight, and the recovery after it (DESIGN §9/D-152)"),
]

# The set that takes a model. Each probe is the live half of an acceptance item or a decision; `--state-dir`
# keeps a failure's evidence (the harness passes one under its own root and keeps it when a probe fails).
MODELS = [
    ("run.py", ["--task", "edit-integrity"], "a fixture task the probe verifies itself, outside the agent"),
    ("crash.py", [], "a daemon crash replays nothing (A08/A11/A12)"),
    ("unknown_outcome.py", [], "the unverifiable crash parks the task and never replays the effect (A09)"),
    ("cancel.py", [], "the terminate lever really stops a running command (A13/D-88)"),
    ("lifecycle_run.py", [], "what pause and terminate do to a run that is already waiting (D-98)"),
    ("deadline.py", [], "the goal deadline end to end: the daemon refuses, the client says why (A35/D-97)"),
    ("job_identity.py", [], "the running job's identity, a duplicate GO and a guessed token (A15/A10)"),
    ("checks.py", [], "a required check no workspace state can satisfy blocks the goal (A16/D-50/D-146)"),
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
    ("protocols.py", [], "every wire protocol accepted against a real service (DESIGN §7/D-151)"),
]

# Files in this directory that are not probes (none today: every module here is one, and a module that
# accumulates helpers should be named here with its reason).
NOT_PROBES: set = set()

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


def harness_root_name() -> str:
    """The one place a run's scratch root name is built: unique, never pid-only (D-156).

    `mkdtemp` gives the uniqueness *and* creates the directory (`TMPDIR`, so the harness stays out of a
    sandboxed `/tmp`). The self-check calls this twice — and removes what it made — because the defect this
    replaced was a name two runs could share, not a missing directory.
    """
    return tempfile.mkdtemp(prefix="teamagents-probe-harness-", dir=os.environ.get("TMPDIR", "/tmp"))


def needs_credentials(set_name: str) -> bool:
    """Does this set need a model credential? `--self-check` states the answer for every set."""
    return set_name in ("models", "all")


# The two ways a probe run can leak — a live session daemon and a scratch directory — are guarded by
# `review/leak_guard.py`, which `make test` uses too, so the harness's counting rule and its stopping rule are
# the same rule as the suite's (D-147). It also carries the two measurements that shaped it: a daemon is
# recognised by its first *argument*, because `ps` prints the binary's path first and a prefix test matches
# nothing (D-144), and a zombie is not a leak because pid 1 does not reap here (D-144); scratch is directories
# only (D-143). This harness used to hold its own copy, and that copy was the one that matched nothing.
daemons = leak_guard.daemons
daemon_pids = leak_guard.daemon_pids
strays = leak_guard.strays


def stop_daemons(state_dir: pathlib.Path) -> None:
    """Stop whatever serves a probe's state root: a killed probe cannot do it itself.

    Registered with `atexit` by each probe, so this is the harness's backstop for a probe that could not run
    its own cleanup (D-138). SIGTERM first, then SIGKILL for what is left (a daemon from a timed-out probe
    outlived the guard's window, D-141); both go to the pid the predicate found, so nothing else is signalled.
    """
    leak_guard.stop([pid for pid, _args in leak_guard.daemon_pids(state_dir)])


def kill_by_pattern_lines(text: str) -> list:
    """1-based lines where *executable* code mentions killing by pattern, or `[]`.

    Docstrings are skipped (`ast` knows them), and so is this checker's own word list — it has to name the words
    to look for them. What survives is a string the code could hand to a shell.
    """
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return []  # a file that does not parse is some other audit's finding
    prose = set()
    for node in ast.walk(tree):
        body = getattr(node, "body", None)
        if not isinstance(node, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)) or not body:
            continue
        first = body[0]
        if isinstance(first, ast.Expr) and isinstance(first.value, ast.Constant) and isinstance(first.value.value, str):
            prose.update(range(first.lineno, (first.end_lineno or first.lineno) + 1))
    own = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == "kill_by_pattern_lines":
            own.update(range(node.lineno, (node.end_lineno or node.lineno) + 1))
    return [
        node.lineno
        for node in ast.walk(tree)
        if isinstance(node, ast.Constant)
        and isinstance(node.value, str)
        and node.lineno not in prose
        and node.lineno not in own
        and any(word in node.value for word in ("pkill", "killall", "kill -f"))
    ]


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
    first, second = harness_root_name(), harness_root_name()
    shutil.rmtree(first, ignore_errors=True)
    shutil.rmtree(second, ignore_errors=True)
    if first == second or pathlib.Path(first).name.endswith(str(os.getpid())):
        findings.append(f"the harness root must not be a name two runs can share: {first!r} / {second!r}")
    if set(SETS) != set(TIMEOUTS):
        findings.append(f"every set needs a per-probe budget: sets={sorted(SETS)} timeouts={sorted(TIMEOUTS)}")
    if needs_credentials("offline") or not needs_credentials("models") or not needs_credentials("all"):
        findings.append("the credential requirement is wrong: only the model sets need one")
    # D-155: a probe file that no set runs is a probe nothing ever measures — `deadline.py` (A35's live half,
    # documented in this directory's README) was in neither set, so `make probe-models` had never run it.
    # If a file here is *not* a probe (a helper module), add it to NOT_PROBES with a reason rather than
    # leaving the gap silent.
    on_disk = {path.name for path in HERE.glob("*.py")} - {"probes.py"} - NOT_PROBES
    listed = {name for name, _extra, _why in OFFLINE + MODELS}
    for missing in sorted(on_disk - listed):
        findings.append(f"{missing} is in neither set: nothing runs it (add it, or list it in NOT_PROBES)")
    for extra in sorted(listed - on_disk):
        findings.append(f"{extra} is in a set and the file does not exist")
    # D-197: a probe is evidence for something outside itself, and the documents that say what it evidences are
    # the acceptance ledger and the decision log — the probe's own registry entry is a description, not a claim.
    # A probe neither of them cites is evidence nobody reads; the sweep that looked for that (D-197) found all 33
    # cited, which is exactly why the rule can be kept cheap now.
    evidence = "".join((REPO / name).read_text() for name in ("docs/ACCEPTANCE.md", "docs/DECISIONS.md"))
    for name in sorted(on_disk):
        if name not in evidence:
            findings.append(f"{name} is cited by neither docs/ACCEPTANCE.md nor docs/DECISIONS.md: an evidence "
                            "probe no acceptance row and no decision names is evidence nobody reads")
    # D-176: the docs state how many probes a set holds, and the number rots (it said "seven credential-free
    # ones" while the set had eight). The phrase is the contract: the digits must equal the set size.
    documented = (REPO / "review" / "dogfood" / "README.md").read_text()
    phrase = f"the {len(OFFLINE)} credential-free ones"
    if phrase not in documented:
        findings.append(
            f"review/dogfood/README.md must say {phrase!r} (the offline set has {len(OFFLINE)} entries; "
            f"if the set changed, update the sentence)"
        )
    # D-233: the sentence that lists the credential-free probes must name every one of them. It had gone stale —
    # "Seven of these probes…" named seven and the set had eight, and no rule read the sentence (the digit phrase
    # above was the only checked statement of the same fact).
    for name, _args, _description in OFFLINE:
        if name not in documented:
            findings.append(f"review/dogfood/README.md does not name {name}, which is in the offline set: the "
                            "sentence that lists them is how a reader knows what runs without a credential")
    # D-240: the offline set size is stated a third time, in the audit index's `probes.py` row, and nothing read
    # it — adding the `max_retries.py` probe found the row still saying "now **9** probes" at ten.
    index = (REPO / "review" / "README.md").read_text()
    stated = f"now **{len(OFFLINE)}** probes"
    if stated not in index:
        findings.append(f"review/README.md must state {stated!r} in the `dogfood/probes.py` row (the offline set "
                        f"has {len(OFFLINE)} entries; if the set changed, update the row)")
    # D-148/D-144: a probe stops what it started **by pid**. Killing by pattern is the hazard those entries
    # measured, and the prose that says so is fine — what must not exist is an *executable* mention, which is
    # what the token scan below leaves once comments and docstrings are gone.
    for path in sorted((REPO / "review").rglob("*.py")):
        if "review/tmp/" in str(path.relative_to(REPO)) + "/":
            continue
        for line in kill_by_pattern_lines(path.read_text(errors="replace")):
            findings.append(
                f"{path.relative_to(REPO)}:{line}: an executable kill-by-pattern mention "
                f"(stop a process by pid — review/leak_guard.py)"
            )

    keep = os.environ.get("TMPDIR")
    try:
        with tempfile.TemporaryDirectory() as tmp:
            os.environ["TMPDIR"] = tmp
            (pathlib.Path(tmp) / "ta-a-directory").mkdir()
            (pathlib.Path(tmp) / "ta-a-file").write_text("")   # files with the prefix appear from elsewhere
            counted = strays()
            if counted != {str(pathlib.Path(tmp) / "ta-a-directory")}:
                findings.append(f"the stray guard must count directories only, got {sorted(counted)}")
    finally:
        if keep is None:
            os.environ.pop("TMPDIR", None)
        else:
            os.environ["TMPDIR"] = keep

    for finding in findings:
        print("FAIL:", finding)
    if not findings:
        print(f"self-check ok: selection, budgets, the stray guard, the documented set sizes, the citation "
              f"of every probe by an acceptance row or a decision, and the no-kill-by-pattern rule over "
              f"{len(SETS)} sets")
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
            # the count is printed here because two documents state it (D-176): a stale number is how the
            # README said "seven" about a set of eight
            print(f"  {label} — {len(probes)} probe(s):")
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
    #
    # The name carries a random component, never just the pid: in a sandbox with a PID namespace per tool call
    # every run gets the *same* low pid, so a later, clean run deleted an earlier failing run's kept evidence by
    # naming and then removing the same path — measured 2026-09-27, when D-143's kept session (read and quoted
    # an hour earlier) was gone (D-156).
    timeout = TIMEOUTS[args.set]
    harness_root = pathlib.Path(harness_root_name())
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

    # A signal returns before the daemon it was sent to is gone, so give a stopped daemon a moment to leave
    # before calling it a leak: the guard must not report a process that is on its way out (measured: a failing
    # probe's daemon was still counted, and the run was red for it — D-141). A probe that passed relies on its
    # own `atexit`, which escalates TERM then KILL since D-148; if the count has still grown after the wait the harness applies the
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
