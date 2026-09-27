#!/usr/bin/env python3
"""`instruction_files`: the user's rules reach **every** member's prompt (D-102's promise, delivered by D-246).

D-102 found the key accepted, validated and unread — a canary in an instruction file was absent from the leader's
prompt while `doctor` promised it reached every member — and this probe pinned that truth so the promise could
not come back silently. This is the same probe *after the flip*: the driver composes the files into every
instance's system text (`config::instruction_text` → `team_kernel`), so the assertions are now the positive ones,
and they are made against the **requests the provider actually received** rather than against a profile row.

It needs no model and no credential: a local chat-completions server answers the leader's first request with a
`spawn` call (whose `instructions` carry a member marker) and every later request with a `finish`, and the probe
reads the captured bodies:

* the rules file holds the canary (the probe's own premise);
* `doctor`'s row says the files reach every member's prompt, and counts the bytes;
* the **leader's** system message carries the canary under the heading that names the file;
* the **child's** system message carries it too, after its own profile instructions (the marker) — the "every
  member" half that only a real request can show.

    python3 review/dogfood/instructions.py
    python3 review/dogfood/instructions.py --state-dir /tmp/ta-instructions
"""
import argparse
import atexit
import json
import os
import pathlib
import shutil
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # shared pid-based stop (D-148)
import leak_guard  # noqa: E402
BIN = REPO / "engine/target/debug/teamagents"
KEY_VAR = "TEAMAGENTS_INSTRUCTION_PROBE_KEY"
CANARY = "CANARY_INSTRUCTION_9F2A"
MEMBER_MARKER = "MEMBER_INSTRUCTIONS_7C1B"
GUIDANCE = "run-and-fix loop"  # the phrase D-262 added to LEADER_INSTRUCTIONS

HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
SPAWN = HEAD + (
    'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-spawn","function":{"name":"spawn",'
    '"arguments":' + json.dumps(json.dumps({"instance_id": "i-worker", "instructions": MEMBER_MARKER + ": you are "
                                            "a worker", "task": "Say the single word ok."})) + '}}]}}]}\n\n'
    'data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,'
    '"completion_tokens":3,"total_tokens":8}}\n\n'
    'data: [DONE]\n\n'
).encode()
FINISH = HEAD + (
    'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-finish","function":{"name":"finish",'
    '"arguments":' + json.dumps(json.dumps({"status": "success", "summary": "ok"})) + '}}]}}]}\n\n'
    'data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,'
    '"completion_tokens":3,"total_tokens":8}}\n\n'
    'data: [DONE]\n\n'
).encode()


class FakeChat(BaseHTTPRequestHandler):
    """The leader's first request gets the spawn call, everything after it a finish; every body is kept."""

    protocol_version = "HTTP/1.0"
    seen: list = []

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        try:
            self.seen.append(json.loads(body.decode("utf-8", "replace")))
        except json.JSONDecodeError:
            self.seen.append({"unparseable": True})
        self.wfile.write(SPAWN if len(self.seen) == 1 else FINISH)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def config(port: int, rules: pathlib.Path) -> str:
    return f"""# Instruction-file probe (D-102/D-246): a local chat-completions server, no credentials, no network.
skills_paths = []
instruction_files = ["{rules}"]

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "instructions-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
"""


def system_texts(seen: list) -> list:
    """The system message of every captured request, in order."""
    out = []
    for body in seen:
        for message in body.get("messages", []) if isinstance(body, dict) else []:
            if message.get("role") == "system":
                out.append(message.get("content") or "")
    return out


def judgement(systems: list, doctor_row: str) -> list:
    """The probe's rule, as a function of what it measured — `--self-check` exercises it without a session."""
    failures = []
    if not systems:
        failures.append("no request carried a system message, so the check proves nothing")
    for index, text in enumerate(systems):
        if CANARY not in text:
            failures.append(f"request {index + 1}'s system prompt does not carry the canary: {text[:120]!r}")
        if f"instruction file:" not in text:
            failures.append(f"request {index + 1}'s system prompt has no heading naming the file: {text[:120]!r}")
    for index, text in enumerate(systems):
        # D-258: the runtime's own settlement rule reaches every member as well, and only a real prompt shows it
        if "Settlement rule:" not in text:
            failures.append(f"request {index + 1}'s system prompt does not carry the settlement rule: {text[:120]!r}")
    # D-262: the product's leader prompt carries the measured delegation guidance; a worker's does not
    leaders = [text for text in systems if MEMBER_MARKER not in text]
    if not leaders:
        failures.append("no leader's prompt was seen, so the leader's own guidance is not what this run checked")
    for index, text in enumerate(leaders):
        if GUIDANCE not in text:
            failures.append(f"a leader's prompt does not carry the delegation guidance: {text[:120]!r}")
    if not any(MEMBER_MARKER in text for text in systems):
        failures.append("no child's prompt was seen, so 'every member' is not what this run checked")
    if "[ok  ] instruction files" not in doctor_row or "reach every member's prompt" not in doctor_row:
        failures.append(f"doctor does not say the files reach every member's prompt: {doctor_row.strip()!r}")
    return failures


def self_check() -> int:
    """Exercise `judgement` on the measured shapes and on the pre-D-246 shape it must report."""
    findings = []
    rule = "Settlement rule: settle a task you were given, or say why not"
    lead = f"{GUIDANCE}: delegate what needs it\n\n{rule}\n\n<!-- instruction file: /rules.md -->\n{CANARY}: say canary"
    good = [f"{MEMBER_MARKER}: you are a worker\n\n{rule}\n\n<!-- instruction file: /rules.md -->\n{CANARY}: say canary", lead]
    row = "[ok  ] instruction files          2 file(s), 84 byte(s) reach every member's prompt"
    if judgement(good, row):
        findings.append("the shapes this build produces must pass")
    if not judgement([f"<!-- instruction file: /rules.md -->\n{CANARY}"], row):
        findings.append("a run that never saw a child's prompt must be reported")
    if not judgement(["you are a worker", "you are a worker"], row):
        findings.append("the pre-D-246 shape (no rules in the prompt) must be reported")
    if not judgement(good, "[WARN] instruction files  1 declared, not applied"):
        findings.append("a doctor row that does not promise the delivery must be reported")
    if not judgement([good[0].replace(rule, "settle later"), good[1]], row):
        findings.append("a prompt without the settlement rule must be reported (D-258)")
    if not judgement([good[0], good[1].replace(GUIDANCE, "delegate freely")], row):
        findings.append("a leader prompt without the delegation guidance must be reported (D-262)")
    for finding in findings:
        print(f"FAIL: {finding}")
    if not findings:
        print("self-check ok: the rule passes the measured shapes and reports the pre-D-246 one")
    return 1 if findings else 0


def stop_daemon(state_root: pathlib.Path) -> None:
    """Stop the daemon this probe started (registered with `atexit`, like every other probe here)."""
    leak_guard.stop_daemons(state_root)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-instructions)")
    parser.add_argument("--self-check", action="store_true", help="check the rule only, with no session")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if self_check():
        return 1
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")

    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeChat)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    root = pathlib.Path(args.state_dir or "/tmp/ta-instructions")
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    workspace, rules, state_root = root / "ws", root / "AGENTS.md", root / "root"
    atexit.register(stop_daemon, state_root)
    failures: list[str] = []
    try:
        shutil.rmtree(root, ignore_errors=True)
        workspace.mkdir(parents=True)
        (root / "config/teamagents").mkdir(parents=True)
        rules.write_text(f"# Project instructions\n{CANARY}: always answer with the single word canary.\n")
        (root / "config/teamagents/config.toml").write_text(config(port, rules))
        if CANARY not in rules.read_text():
            return 1
        env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
               "PATH": "/usr/bin:/bin", KEY_VAR: "not-a-key", "HOME": str(root)}
        doctor = subprocess.run([str(BIN), "doctor", "--state-root", str(state_root)], capture_output=True,
                                text=True, env=env, timeout=60)
        row = next((line for line in doctor.stdout.splitlines() if "instruction files" in line), "")
        print(f"   doctor: {row.strip()}")
        run = subprocess.run(
            [str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json", "--timeout", "60",
             "--cwd", str(workspace), "Say the single word ok, then hand the same to a worker."],
            capture_output=True, text=True, env=env, timeout=180,
        )
        systems = system_texts(list(FakeChat.seen))
        print(f"   one run: exit={run.returncode} requests={len(FakeChat.seen)} system prompts={len(systems)}")
        for index, text in enumerate(systems):
            where = "child" if MEMBER_MARKER in text else "leader"
            print(f"     {index + 1}. {where}: {len(text)} chars, canary={'yes' if CANARY in text else 'NO'}")
        failures += judgement(systems, row)
        if run.returncode not in (0, 1):
            failures.append(f"the run should end with a report, exit={run.returncode}")
    finally:
        server.shutdown()
        FakeChat.seen = []

    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
