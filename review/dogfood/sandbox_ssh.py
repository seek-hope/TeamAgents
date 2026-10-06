#!/usr/bin/env python3
"""The ssh sandbox backend, against a **real sshd** (D-437).

`[permissions] sandbox = "ssh"` moves a session's shell to another host over `ssh`. The rule that survives from
bubblewrap and Docker is the one `verification/tla/V2Isolation.tla` models — *the command runs under the
configured backend or it does not run at all* — and this probe measures both halves over the real wire stack,
credential-free (a local sshd on a high port, temp keys under the scratch root, and a local chat-completions
server that plays the model):

1. **it really goes to the remote**: the scripted `shell` call writes a marker file in a directory that exists
   only on the remote side of the configuration (``sandbox_ssh_workdir``), and that file appears there and *not*
   in the session's local working directory;
2. **an unreachable host is a refusal, never a local shell**: with the sshd stopped, the same call fails and the
   marker still does not appear locally — the fail-closed property, and the one a silent fallback would break.

The ssh target is `user@127.0.0.1` with ``sandbox_ssh_options`` carrying the port and the key, because this is one
machine playing two roles: that is what those options are for (a non-default port, a specific key, a jump host)
and they are placed after the product's own `-o` list, where OpenSSH's first-value-wins keeps `BatchMode=yes` in
force.

    python3 review/dogfood/sandbox_ssh.py
    python3 review/dogfood/sandbox_ssh.py --state-dir /tmp/ta-sandbox-ssh
"""
import argparse
import atexit
import getpass
import json
import os
import pathlib
import shutil
import sqlite3
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # the shared pid-based stop (D-148)
import leak_guard  # noqa: E402

BIN = REPO / "engine/target/debug/teamagents"
SSHD = "/usr/sbin/sshd"
KEY_VAR = "TEAMAGENTS_SSH_PROBE_KEY"
MARKER = "remote-marker.txt"
HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"


def stream(*deltas: dict) -> bytes:
    body = b"".join(f"data: {json.dumps(delta)}\n\n".encode() for delta in deltas) + b"data: [DONE]\n\n"
    return HEAD + body


def tool_call(name: str, arguments: dict) -> bytes:
    return stream(
        {"choices": [{"delta": {"tool_calls": [{"index": 0, "id": f"call-{name}", "type": "function",
                                                "function": {"name": name,
                                                             "arguments": json.dumps(arguments)}}]}}]},
        {"choices": [{"delta": {}, "finish_reason": "tool_calls"}],
         "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}},
    )


class FakeChat(BaseHTTPRequestHandler):
    """A scripted model: **armed** by the probe, it answers the first request of that turn with one `shell` call and
    everything after it with a `finish`. Arming per turn is what makes the two halves comparable — a fake that
    keyed off "does the conversation have a tool message" answered the *second* turn with `finish`, so the
    fail-closed step ran no shell call at all and reported success."""

    protocol_version = "HTTP/1.0"
    shell_pending = threading.Event()

    @classmethod
    def arm(cls) -> None:
        cls.shell_pending.set()

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        try:
            conversation = json.loads(body.decode("utf-8", "replace")).get("messages", [])
        except json.JSONDecodeError:
            conversation = []
        armed = FakeChat.shell_pending.is_set()
        if armed:
            FakeChat.shell_pending.clear()
        if not armed and any(message.get("role") == "tool" for message in conversation):
            reply = tool_call("finish", {"status": "success", "summary": "the remote write is done"})
        else:
            reply = tool_call("shell", {"command": f"pwd > {MARKER} && cat {MARKER}", "timeout": 60})
        self.wfile.write(reply)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def config(port: int, ssh_port: int, key: pathlib.Path, remote_dir: pathlib.Path) -> str:
    return f"""# Sandbox-backend probe (D-437): a local chat-completions server and a local sshd, no credentials.
skills_paths = []

[permissions]
# D-437: the sandbox applies in `approved_scope` (the default). `full_auto` means the host on purpose — the
# session's mode belongs to the session (D-41) — so this probe stays in the mode the sandbox exists for and takes
# the approval path a headless client has: `teamagents approvals approve --id …` (D-67).
sandbox = "ssh"
sandbox_ssh_target = "{getpass.getuser()}@127.0.0.1"
sandbox_ssh_workdir = "{remote_dir}"
sandbox_ssh_options = ["-p", "{ssh_port}", "-i", "{key}", "-o", "UserKnownHostsFile=/dev/null"]

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "ssh-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
max_retries = 0
"""


def free_port() -> int:
    import socket

    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def start_sshd(root: pathlib.Path, ssh_port: int) -> subprocess.Popen:
    """A real sshd as this user, on a high port, with keys that live and die with the probe's scratch root."""
    ssh_dir = root / "sshd"
    ssh_dir.mkdir(parents=True, exist_ok=True)
    for name in ("id", "hostkey"):
        if not (ssh_dir / name).exists():
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(ssh_dir / name)],
                           check=True, capture_output=True)
    shutil.copy(ssh_dir / "id.pub", ssh_dir / "authorized_keys")
    for path in ("authorized_keys", "id", "hostkey"):
        (ssh_dir / path).chmod(0o600)
    (ssh_dir / "sshd_config").write_text(
        f"Port {ssh_port}\nListenAddress 127.0.0.1\nHostKey {ssh_dir}/hostkey\n"
        f"AuthorizedKeysFile {ssh_dir}/authorized_keys\nStrictModes no\nUsePAM no\n"
        f"PasswordAuthentication no\nPubkeyAuthentication yes\nPermitRootLogin no\n"
        f"PidFile {ssh_dir}/sshd.pid\nLogLevel ERROR\n"
    )
    log = (ssh_dir / "sshd.log").open("w")
    process = subprocess.Popen([SSHD, "-f", str(ssh_dir / "sshd_config"), "-D", "-e"],
                               stdout=log, stderr=subprocess.STDOUT)
    # wait for the listener rather than sleeping a fixed time: the proof is a connection, not a delay
    for _ in range(100):
        probe = subprocess.run(["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new",
                                "-o", "UserKnownHostsFile=/dev/null", "-o", "ConnectTimeout=2",
                                "-p", str(ssh_port), "-i", str(ssh_dir / "id"),
                                f"{getpass.getuser()}@127.0.0.1", "true"], capture_output=True, text=True)
        if probe.returncode == 0:
            return process
        time.sleep(0.1)
    process.terminate()
    raise SystemExit(f"the probe's sshd never answered; see {ssh_dir / 'sshd.log'}")


def run_turn(state_root: pathlib.Path, workspace: pathlib.Path, env: dict) -> tuple[int, str]:
    """One turn; in `approved_scope` the scripted `shell` call parks, so this expects exit 3."""
    done = subprocess.run(
        [str(BIN), "exec", "--json", "--state-root", str(state_root), "--cwd", str(workspace),
         "--timeout", "120", "write the marker on the remote"],
        capture_output=True, text=True, env=env, timeout=240)
    return done.returncode, (done.stdout or "") + (done.stderr or "")


def pending_approval(state_root: pathlib.Path, env: dict) -> str:
    """The id of the parked operation, read the way a headless client reads it (D-67)."""
    listed = subprocess.run([str(BIN), "approvals", "--json", "--state-root", str(state_root)],
                            capture_output=True, text=True, env=env, timeout=60)
    body = json.loads(listed.stdout) if listed.stdout.strip().startswith("{") else {}
    rows = body.get("approvals") or []
    if not rows:
        raise SystemExit(f"no approval is pending: {listed.stdout[-200:]!r} {listed.stderr[-200:]!r}")
    return rows[0]["id"]


def approve(state_root: pathlib.Path, env: dict, approval_id: str) -> tuple[int, str]:
    done = subprocess.run([str(BIN), "approvals", "approve", "--id", approval_id, "--state-root", str(state_root)],
                          capture_output=True, text=True, env=env, timeout=60)
    return done.returncode, (done.stdout or "") + (done.stderr or "")


def newest_receipt(state_root: pathlib.Path) -> str:
    """The most recent operation's receipt, read from the product's own store (`operations.receipt_json`)."""
    connection = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    try:
        row = connection.execute(
            "SELECT receipt_json FROM operations ORDER BY rowid DESC LIMIT 1").fetchone()
        return (row[0] or "") if row else ""
    finally:
        connection.close()


def wait_for_receipt(state_root: pathlib.Path, needle: str, seconds: float = 30.0) -> str:
    deadline = time.time() + seconds
    receipt = ""
    while time.time() < deadline:
        receipt = newest_receipt(state_root)
        if needle in receipt:
            return receipt
        time.sleep(0.2)
    return receipt


def wait_for_marker(path: pathlib.Path, seconds: float = 30.0) -> bool:
    """Wait for a file to appear — the dispatched command's effect, not a sleep-and-hope."""
    deadline = time.time() + seconds
    while time.time() < deadline:
        if path.exists():
            return True
        time.sleep(0.1)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-sandbox-ssh)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not pathlib.Path(SSHD).is_file():
        raise SystemExit(f"{SSHD} is missing: this probe needs a real sshd to measure the backend")
    root = pathlib.Path(args.state_dir or "/tmp/ta-sandbox-ssh")
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    shutil.rmtree(root, ignore_errors=True)
    workspace, state_root, remote_dir = root / "ws", root / "root", root / "remote"
    for directory in (workspace, remote_dir):
        directory.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    failures: list[str] = []

    ssh_port = free_port()
    sshd = start_sshd(root, ssh_port)
    atexit.register(lambda: sshd.terminate())
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeChat)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    (root / "config/teamagents/config.toml").write_text(
        config(server.server_address[1], ssh_port, root / "sshd/id", remote_dir))
    env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           "PATH": os.environ.get("PATH", "/usr/bin:/bin"), KEY_VAR: "not-a-key", "HOME": str(root)}
    atexit.register(leak_guard.stop_daemons, state_root)

    remote_marker, local_marker = remote_dir / MARKER, workspace / MARKER
    try:
        # 1. the shell call parks for approval (the mode the sandbox applies in), the headless client approves it
        # the way the CLI documents, and the command then runs on the remote side of the configuration.
        FakeChat.arm()
        code, output = run_turn(state_root, workspace, env)
        print(f"1. exec={code} (approved_scope parks the shell call)")
        if code != 3:
            failures.append(f"a shell call in approved_scope must park for approval: exit={code} {output[-300:]!r}")
        approval_id = pending_approval(state_root, env)
        decided, decided_output = approve(state_root, env, approval_id)
        print(f"2. approvals approve --id {approval_id}: exit={decided} {decided_output.strip()[:80]!r}")
        if decided != 0:
            failures.append(f"approving the parked call failed: {decided} {decided_output[-200:]!r}")
        ran = wait_for_marker(remote_marker)
        print(f"3. after the approval: remote marker={remote_marker.exists()} local marker={local_marker.exists()}")
        if not ran:
            failures.append(f"the approved command did not run in the configured remote directory: {remote_dir}")
        elif remote_marker.read_text().strip() != str(remote_dir):
            failures.append(f"the remote `pwd` is not the configured directory: {remote_marker.read_text()!r}")
        if local_marker.exists():
            failures.append("the command ran locally: the marker is in the session's own workspace")

        # 4. an unreachable host refuses instead of running locally — the fail-closed rule the model checks. The
        # call still parks for approval (the sandbox is probed at *dispatch*, not at the gate), so the probe
        # approves it and reads what the product recorded when the command could not run.
        sshd.terminate()
        sshd.wait(timeout=10)
        remote_marker.unlink(missing_ok=True)
        local_marker.unlink(missing_ok=True)
        FakeChat.arm()
        code, output = run_turn(state_root, workspace, env)
        print(f"4. exec={code} with the sshd stopped; local marker={local_marker.exists()} "
              f"remote marker={remote_marker.exists()}")
        if code == 3:
            approval_id = pending_approval(state_root, env)
            decided, decided_output = approve(state_root, env, approval_id)
            print(f"4b. approvals approve --id {approval_id}: exit={decided}")
            if decided != 0:
                failures.append(f"approving the call against an unreachable host failed: {decided_output[-200:]!r}")
            # The refusal may come from either half, and both are fail-closed: the pre-flight probe
            # (`IsolationUnavailable`, what `doctor` reports) or ssh's own connect error (`ssh: connect to host …
            # Connection refused`, exit 255 — the command *did* go to ssh). What must never happen is a local run.
            receipt = wait_for_receipt(state_root, '"ok":false')
            print(f"4c. operation receipt: {receipt[:200]!r}")
            if '"ok":false' not in receipt:
                failures.append(f"the operation did not record a failure: {receipt[:300]!r}")
            elif "ssh" not in receipt and "IsolationUnavailable" not in receipt:
                failures.append(f"the failure does not name ssh or the isolation: {receipt[:300]!r}")
        elif code != 0:
            # an earlier refusal is also fine, as long as it is the *product* refusing
            if "IsolationUnavailable" not in output:
                failures.append(f"the turn failed without naming the isolation: {output[-300:]!r}")
        time.sleep(1.0)
        if local_marker.exists():
            failures.append("an unreachable ssh host ran the command LOCALLY — the fail-closed rule is broken")
        if remote_marker.exists():
            failures.append("a marker appeared although the ssh host was already stopped")
    finally:
        server.shutdown()
        sshd.terminate()
        leak_guard.stop_daemons(state_root)

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
