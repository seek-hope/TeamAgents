#!/usr/bin/env python3
"""The user's approval decision, taken in the TUI with a real model (A25, D-67's TUI half).

A dangerous call is the one place where the product asks the user for a decision, and until now that
decision was covered only twice: `v2_daemon::the_approvals_cli_lists_and_decides_a_parked_operation` (the
*CLI* against a real daemon, scripted provider) and the scripted-daemon PTY smoke (the TUI's frames). This
probe puts the two together: the real TUI, a real daemon, a real model, and a shell command that is gated
because the session runs in `approved_scope` (`require_shell_approval = !full_auto`).

1. the daemon starts **without** `--full-auto`, so every shell call needs a user decision;
2. the TUI attaches, the probe types a prompt asking for `echo approved-live > proof.txt`, and the shell
   call parks the turn: `teamagents approvals --json` reports the pending id, and the same id has to be on
   screen in the approvals box (the panel is a real surface, not decoration);
3. the probe presses `Ctrl+A` (focus the approvals box) and then `a` (approve) or `d` (deny);
4. the artifact decides: on **approve** `proof.txt` must exist with the content, the approval is `APPROVED`,
   the operation succeeded and the panel shows no pending approval; on **deny** the file must never exist,
   the approval is `DENIED`, and the operation lands `CANCELLED` with a receipt whose `class` is `denied` —
   the call fails closed and the model is told.

    python3 review/dogfood/approval.py                      # approve, DeepSeek
    python3 review/dogfood/approval.py --decision deny      # the other decision
    python3 review/dogfood/approval.py --state-dir /tmp/ta-approval --timeout 240

It is a real-model check (`DEEPSEEK_API_KEY`, native context window D-36), needs `bubblewrap` (the default
`approved_scope` shell runs inside it), and is not part of `make check`. Everything it writes stays under
`--state-dir`; it quits the TUI and stops the daemon it started.
"""
import argparse
import fcntl
import json
import os
import pathlib
import pty
import shutil
import sqlite3
import struct
import subprocess
import sys
import termios
import time

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"
sys.path.insert(0, str(REPO / "tui" / "scripts"))
from pty_screen import Screen, read_all  # noqa: E402  (the smoke's virtual terminal)

CONFIG = """# Approval probe (A25): a real model on the native context window (D-36).
skills_paths = []

[models.leader_main]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
context_window = 1000000
timeout = 180
max_retries = 2
generation_options = { reasoning_effort = "high" }
"""

PROMPT = ("Run the shell command `echo approved-live > {proof}` in the shared workspace and report its "
          "output and exit status. Do not use any other tool first.")
CONTENT = "approved-live"


def call(bin_args: list[str], env: dict, timeout: int = 60) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def pending_approvals(state_root: pathlib.Path, env: dict) -> list[dict]:
    listed = call(["approvals", "--state-root", str(state_root), "--json"], env)
    if not listed.stdout.strip().startswith("{"):
        return []
    return json.loads(listed.stdout)["approvals"]


def approvals_in_db(state_root: pathlib.Path) -> list[tuple[str, str, str]]:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    return [(row[0], row[1], row[2]) for row in db.execute(
        "SELECT id, status, operation_id FROM approvals ORDER BY rowid")]


def operations(state_root: pathlib.Path) -> list[tuple[str, str, str]]:
    """(operation id, status, receipt error class)."""
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    rows = []
    for operation_id, status, receipt in db.execute(
            "SELECT operation_id, status, receipt_json FROM operations ORDER BY rowid"):
        error_class = ""
        if receipt:
            try:
                error_class = (json.loads(receipt).get("error") or {}).get("class", "")
            except json.JSONDecodeError:
                error_class = "unparseable"
        rows.append((operation_id, status, error_class))
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--decision", default="approve", choices=["approve", "deny"])
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-approval)")
    parser.add_argument("--timeout", type=int, default=240, help="seconds to wait for the approval / the effect")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-approval")
    workspace = root / "ws"
    proof = workspace / "proof.txt"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    failures: list[str] = []
    daemon = None
    pid = fd = None
    log = open(root / "daemon.log", "w")
    try:
        daemon = subprocess.Popen([str(BIN), "daemon", "--state-root", str(state_root), "--cwd", str(workspace)],
                                  env=env, stdout=log, stderr=subprocess.STDOUT, text=True)
        deadline = time.time() + 30
        while time.time() < deadline and not (state_root / "daemon.sock").exists():
            if daemon.poll() is not None:
                failures.append(f"the daemon exited: {log.read()[-300:]}")
                return 1
            time.sleep(0.05)
        print("  daemon is up (approved_scope: every shell call needs a decision)")

        # --- the TUI, on the real session ---------------------------------------
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe(str(BIN), [str(BIN), "--state-root", str(state_root), "--cwd", str(workspace)], env)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 100, 0, 0))
        screen = Screen(100, 40)

        def painted() -> str:
            return "\n".join(screen.lines())

        def pump(seconds: float) -> None:
            chunk = read_all(fd, seconds).decode("utf-8", "replace")
            if chunk:
                screen.feed(chunk)

        def wait_gone(needle: str, timeout: float, label: str, step: float = 0.25) -> bool:
            """Wait for a needle to leave the screen — the positive presence was asserted first."""
            started = time.time()
            while time.time() - started < timeout:
                pump(step)
                if needle not in painted():
                    return True
            failures.append(f"{label}: {needle!r} is still on screen")
            return False

        def wait_for(needle: str, timeout: float, label: str, step: float = 0.25) -> bool:
            started = time.time()
            while time.time() - started < timeout:
                pump(step)
                if needle in painted():
                    return True
            failures.append(f"{label}: {needle!r} never appeared on screen")
            return False

        if not wait_for("i-leader", 60, "startup"):
            print(painted()[-800:])
            return 1
        print("  the TUI is attached to the session")

        os.write(fd, PROMPT.format(proof=proof).encode())
        pump(1.0)
        os.write(fd, b"\r")
        print("  asked for a shell call that needs the user's decision")

        # --- the call parks, and the approval is on screen -----------------------
        approval = None
        started = time.time()
        while time.time() - started < args.timeout and approval is None:
            found = pending_approvals(state_root, env)
            approval = found[0] if found else None
            if approval is None:
                time.sleep(0.5)
        if approval is None:
            print(painted()[-1200:])
            failures.append("no approval was ever requested")
            return 1
        approval_id = approval["id"]
        print(f"  approval {approval_id} is pending (tool {approval['tool']}, args {approval['preview']!r})")
        if not wait_for(approval_id, 60, "the approvals box shows the pending id"):
            print(painted()[-900:])
        else:
            print("  the approvals box on screen lists that id")

        # --- the decision, taken with the TUI's own keys -------------------------
        os.write(fd, b"\x01")  # Ctrl+A: focus the approvals box
        if not wait_for("approvals (focused)", 20, "the approvals box takes focus"):
            return 1
        os.write(fd, b"a" if args.decision == "approve" else b"d")
        pump(1.0)
        print(f"  pressed {'a (approve)' if args.decision == 'approve' else 'd (deny)'} in the approvals box")

        final = ""
        deadline = time.time() + 60
        while time.time() < deadline:
            rows = [row for row in approvals_in_db(state_root) if row[0] == approval_id]
            final = rows[0][1] if rows else "missing"
            if final in ("APPROVED", "DENIED"):
                break
            time.sleep(0.25)
        expected = "APPROVED" if args.decision == "approve" else "DENIED"
        if final != expected:
            failures.append(f"the approval landed {final!r}, expected {expected}")
        else:
            print(f"  the session recorded the approval as {final}")

        def wait_until(predicate, timeout: float, step: float = 0.5) -> bool:
            deadline = time.time() + timeout
            while time.time() < deadline:
                if predicate():
                    return True
                time.sleep(step)
            return predicate()

        if args.decision == "approve":
            # the artifact: the command really ran, and it ran in the workspace
            deadline = time.time() + args.timeout
            while time.time() < deadline and not proof.is_file():
                time.sleep(0.5)
            if proof.is_file() and proof.read_text().strip() == CONTENT:
                print(f"  proof.txt exists with {CONTENT!r}: the approved call ran")
            else:
                print(painted()[-900:])
                failures.append(f"proof.txt is missing or wrong ({proof.read_text() if proof.is_file() else 'absent'})")
            deadline = time.time() + 60
            rows = []
            while time.time() < deadline:
                rows = operations(state_root)
                if any(status == "SUCCEEDED" and not error_class for _, status, error_class in rows):
                    break
                time.sleep(0.5)
            if not any(status == "SUCCEEDED" and not error_class for _, status, error_class in rows):
                failures.append(f"no operation succeeded after the approval: {rows}")
            # the positive presence was asserted above; now the decided id has to leave the box and the
            # session's pending list. (A *second* call may legitimately ask for another decision — the probe
            # reports that instead of treating it as a failure.)
            if wait_gone(approval_id, 30, "the approvals box drops the decided id"):
                print("  the approvals box no longer lists it")
            if not wait_until(lambda: all(a["id"] != approval_id for a in pending_approvals(state_root, env)), 30):
                failures.append("the decided approval is still pending in the session")
            still_pending = [a["id"] for a in pending_approvals(state_root, env)]
            print("  the session has no pending approval" if not still_pending
                  else f"  note: the model asked for another decision ({still_pending})")
        else:
            # the command must never have run, and the operation must fail closed
            if wait_gone(approval_id, 30, "the approvals box drops the decided id"):
                print("  the approvals box no longer lists it")
            time.sleep(8.0)
            if proof.exists():
                failures.append("the denied command ran anyway: proof.txt exists")
            else:
                print("  proof.txt does not exist: the denied call never ran")
            rows = [row for row in operations(state_root) if row[2] == "denied" or row[1] == "CANCELLED"]
            if not rows:
                failures.append(f"the denied operation did not fail closed: {operations(state_root)}")
            else:
                print(f"  the operation failed closed ({[(i, s, c) for i, s, c in rows][:2]})")
            db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
            told = list(db.execute(
                "SELECT COUNT(*) FROM context_entries WHERE message_json LIKE '%denied by user%'"))
            if told[0][0] == 0:
                failures.append("the model was never told the call was denied")
            else:
                print("  the model was told the call was denied (the receipt is in its context)")
    finally:
        if fd is not None:
            try:
                os.write(fd, b"\x03")  # Ctrl+C quits the TUI
                time.sleep(0.5)
            except OSError:
                pass
        if pid:
            try:
                os.kill(pid, 9)
            except ProcessLookupError:
                pass
        if daemon is not None and daemon.poll() is None:
            daemon.terminate()
            try:
                daemon.wait(timeout=10)
            except subprocess.TimeoutExpired:
                daemon.kill()
        subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)
        log.close()
        for failure in failures:
            print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
