#!/usr/bin/env python3
"""A real-model check that a message travels around a team (A02's live half).

A02's offline evidence is the control plane (`control::messages_flow_across_an_authorized_ring`) plus the
authority surface D-61 gave the user. What a user does is different: they ask a Leader to build a team, and
then the members *talk*. This probe builds the smallest ring that is still a ring — A → B → C → A — and puts
one token through it:

  * the user asks the Leader to hire two workers (the product path that creates members);
  * the user grants both `message@session` (a spawned worker holds no authority of its own, §5.1/D-61);
  * the user tells relay-b to `send` `RING-<token>` to relay-c, and relay-c to `send` it back to the Leader;
  * the artifact is the Leader's own conversation: the token has to arrive there as a *member's* message,
    with the runtime's own attribution (`message_sent` names the sender; nothing is read out of the prose).

Every hop is one small turn for one member — no model has to orchestrate the ring, it only has to use `send`
once — which is what makes the run re-runnable rather than a coin flip. The probe reads the session database
and the daemon's history surface to see who said what, and stops the daemon it started.

    python3 review/dogfood/team_ring.py
    python3 review/dogfood/team_ring.py --state-dir /tmp/ta-ring --timeout 300

Real model (`DEEPSEEK_API_KEY`, native window D-36); everything it writes stays under `--state-dir`.
"""
import argparse
import atexit
import json
import os
import pathlib
import shutil
import sqlite3
import socket as socket_module
import subprocess
import sys
import time
import uuid

REPO = pathlib.Path(__file__).resolve().parents[2]
BIN = REPO / "engine/target/debug/teamagents"

CONFIG = """# Team-ring dogfood (A02): one provider, three instances, one token around the ring.
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

HIRE = """Hire exactly two teammates. Spawn one worker with the instructions "follow the instructions your
teammates send you, then finish", and spawn a second worker with the same instructions. Then end your turn and
report the ids of the two teammates you hired."""

RELAY = """Use the `send` tool once: send exactly the text `{token}` to the teammate whose instance id is
`{recipient}`. Then finish your turn with the word relayed."""


def call(bin_args: list[str], env: dict, timeout: int = 300) -> subprocess.CompletedProcess:
    return subprocess.run([str(BIN), *bin_args], capture_output=True, text=True, env=env, timeout=timeout)


def protocol(socket_path: pathlib.Path, method: str, params: dict, command_id: str | None = None) -> dict:
    """One documented request/reply round trip (§9); a client per call, like the TUI's reconnect path."""
    connection = socket_module.socket(socket_module.AF_UNIX)
    connection.settimeout(30)
    connection.connect(str(socket_path))
    stream = connection.makefile("rw")
    stream.readline()  # greeting
    request = {"protocol_version": 1, "request_id": f"probe-{uuid.uuid4()}", "method": method, "params": params}
    if command_id:
        request["command_id"] = command_id
    stream.write(json.dumps(request) + "\n")
    stream.flush()
    reply = json.loads(stream.readline())
    connection.close()
    return reply


def entries(state_root: pathlib.Path, instance: str) -> list[dict]:
    """The instance's context as the session stores it: kind + rendered message."""
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    rows = list(
        db.execute(
            "SELECT kind, message_json FROM context_entries WHERE instance_id = ?1 ORDER BY epoch, idx", [instance]
        )
    )
    out = []
    for kind, message in rows:
        try:
            body = json.loads(message)
        except json.JSONDecodeError:
            body = {"content": message}
        out.append({"kind": kind, "content": json.dumps(body)})
    return out


def messages_sent(state_root: pathlib.Path) -> list[tuple[str, str]]:
    """(sender, recipient) for every `message_sent` event, in order.

    The recipient is the event's *scope* (`event(…, "message_sent", recipient, …)`); the payload carries the
    sender and the envelope id.
    """
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    rows = list(db.execute("SELECT scope, payload_json FROM events WHERE kind = 'message_sent' ORDER BY sequence"))
    return [(json.loads(payload)["sender"], scope) for scope, payload in rows]


def operations(state_root: pathlib.Path) -> list[tuple[str, str]]:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    return list(db.execute("SELECT operation_id, status FROM operations ORDER BY rowid"))


def instances(state_root: pathlib.Path) -> list[str]:
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    return [row[0] for row in db.execute("SELECT id FROM instances ORDER BY id")]


def wait_until(predicate, timeout: float, step: float = 0.5) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(step)
    return predicate()


def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started (it is detached on purpose, §9)."""
    subprocess.run(["pkill", "-f", f"daemon --state-root {state_root}"], capture_output=True)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-ring)")
    parser.add_argument("--timeout", type=int, default=300, help="seconds each hop gets")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    if not os.environ.get("DEEPSEEK_API_KEY", "").strip():
        raise SystemExit("DEEPSEEK_API_KEY is not set in this environment")

    root = pathlib.Path(args.state_dir or "/tmp/ta-ring")
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(CONFIG)
    state_root = root / "root"
    env = {**os.environ, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    common = ["--state-root", str(state_root)]
    token = f"RING-{uuid.uuid4().hex[:8]}"
    failures: list[str] = []
    daemon = None
    log = open(root / "daemon.log", "w")
    try:
        daemon = subprocess.Popen([str(BIN), "daemon", *common, "--cwd", str(workspace)], env=env,
                                  stdout=log, stderr=subprocess.STDOUT, text=True)
        socket_path = state_root / "daemon.sock"
        if not wait_until(lambda: socket_path.exists() or daemon.poll() is not None, 30):
            failures.append("the daemon never bound its socket")
            return 1
        if not socket_path.exists():
            failures.append(f"the daemon exited: {log.read()[-400:]}")
            return 1
        print("  daemon is up")

        # --- the user asks the Leader for a team (the product path that hires) -----
        hired = call(["exec", *common, "--json", "--timeout", str(args.timeout), "--cwd", str(workspace), HIRE], env)
        report = json.loads(hired.stdout) if hired.stdout.strip().startswith("{") else {}
        members = [name for name in instances(state_root) if name != "i-leader"]
        if len(members) != 2:
            words = (report.get("reply") or hired.stdout or hired.stderr or "").strip()
            failures.append(f"the Leader hired {len(members)} workers (expected 2): {words[:300]}")
            return 1
        relay_b, relay_c = members
        print(f"  the team: leader i-leader, relay_b={relay_b}, relay_c={relay_c} (token {token})")

        # --- a spawned worker holds nothing; the user grants the messaging (D-61) --
        for member in (relay_b, relay_c):
            granted = call(["authority", *common, "grant", "--subject", member, "--action", "message",
                            "--scope", "session", "--json"], env)
            if granted.returncode != 0:
                failures.append(f"the message grant to {member} was refused: {granted.stdout or granted.stderr}")
                return 1
        print("  the user granted message@session to both workers")

        # --- the ring, one hop at a time -----------------------------------------
        # Each hop is one small turn for one member. A turn that did not send is re-asked (up to three times):
        # the claim is about delivery, not about one turn's obedience, and the delivery itself is what is
        # observed — the token appearing in the recipient's context as a `message` entry.
        def relay(sender: str, recipient: str) -> bool:
            for _ in range(3):
                hop = protocol(socket_path, "submit_input",
                               {"instance_id": sender, "envelope_id": f"probe-{uuid.uuid4()}",
                                "text": RELAY.format(token=token, recipient=recipient)},
                               f"input-{uuid.uuid4()}")
                if not hop.get("ok"):
                    print(f"  the input to {sender} was refused: {hop.get('error')}")
                    return False
                # the artifact is the delivered envelope in the *recipient's* context, rendered by the runtime as
                # `[message from <sender>] <text>`: attribution and payload read from the session, never from the
                # sender's prose
                if wait_until(
                    lambda: any(
                        token in entry["content"] and f"[message from {sender}]" in entry["content"]
                        for entry in entries(state_root, recipient)
                    ),
                    args.timeout,
                ):
                    return True
            return False

        if relay(relay_b, relay_c):
            print(f"  hop 1: {relay_b} → {relay_c} delivered (the token is in {relay_c}'s context as a message)")
        else:
            failures.append(f"hop 1 never arrived: {relay_c}'s context has no `message` with the token")
            return 1
        if relay(relay_c, "i-leader"):
            print("  hop 2: relay_c → i-leader delivered (the ring closed: the Leader's context carries the token)")
        else:
            failures.append("hop 2 never arrived: the Leader's context has no `message` with the token")

        # --- who said what, from the runtime's own record ------------------------
        sent = messages_sent(state_root)
        print(f"  message_sent events (sender, recipient): {sent}")
        if (relay_b, relay_c) not in sent:
            failures.append(f"no message_sent from {relay_b} to {relay_c}: {sent}")
        if (relay_c, "i-leader") not in sent:
            failures.append(f"no message_sent from {relay_c} to the leader: {sent}")
        print(f"  operations: {operations(state_root)}")
    finally:
        if daemon is not None:
            stop_daemon(state_root)
        log.close()
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
