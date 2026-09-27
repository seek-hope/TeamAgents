#!/usr/bin/env python3
"""`exec --stream-json`: the events while the run waits, then the report (D-249).

The headless entry point printed one report at the end, so a CI job or a wrapper script could only learn what
happened *after* the run — the shape Codex's `codex exec --json` and Hermes' streamed tool output exist to
avoid. This probe drives the real binary over the streaming mode and pins the contract:

1. **the lines arrive while the run is still going** — the first line is read, and the process is still
   running; a mode that collected the events and printed them at the end would fail exactly here, which is what
   the local service's hold on the second turn (3 s) makes deterministic;
2. **the lines are the session's committed events, in log order, each once** — strictly increasing `sequence`
   numbers on `{"type":"event","event":{…}}` lines, which is the prefix rule the daemon model pins
   (`verification/tla/V2Daemon.tla`); a stream that skipped or repeated one would break it;
3. **the last line is the report**, in an envelope (`{"type":"report","report":{…}}`), its `watermark` naming
   the last delivered sequence — and the run's exit code is the documented one;
4. **the report is the object `--json` prints**: a plain run in a session of its own yields the same field set
   (the envelope is not a second contract);
5. **a consumer that closes stdout does not break the run** (`| head`): the run ends with its own outcome and
   no write error, because a closed reader is not a failed run;
6. **the two output shapes and the wrong verb are refused by name** (`--json --stream-json`, and
   `--stream-json` outside `exec`), before anything starts.

No credential and no network: a local chat-completions server answers each first request with a `shell` tool
call and every later one with a `finish` (keyed on the conversation carrying a tool result, so several
sessions can share it).

    python3 review/dogfood/stream_json.py
    python3 review/dogfood/stream_json.py --state-dir /tmp/ta-stream-json
"""
import argparse
import atexit
import json
import os
import pathlib
import select
import shutil
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "review"))   # the shared pid-based stop (D-148)
import leak_guard  # noqa: E402

BIN = REPO / "engine/target/debug/teamagents"
KEY_VAR = "TEAMAGENTS_STREAM_PROBE_KEY"
HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
# The second turn is held, so the probe can look at the process *while* the run is still going (D-249).
HOLD_S = 3.0


def stream(*deltas: dict) -> bytes:
    """One scripted chat-completions SSE response from delta objects."""
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


SHELL_TURN = tool_call("shell", {"command": "true", "timeout": 30})
FINISH_TURN = tool_call("finish", {"status": "success", "summary": "the streamed run finished"})


class FakeChat(BaseHTTPRequestHandler):
    """The first turn of a conversation asks for a shell call; any turn after a tool result finishes.

    Keyed on the conversation rather than on a request counter, so every scenario's session can share one
    server (a counter would see several sessions as one long conversation).
    """

    protocol_version = "HTTP/1.0"
    seen = 0

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        FakeChat.seen += 1
        try:
            conversation = json.loads(body.decode("utf-8", "replace")).get("messages", [])
        except json.JSONDecodeError:
            conversation = []
        has_tool_result = any(message.get("role") == "tool" for message in conversation)
        if has_tool_result:
            time.sleep(HOLD_S)   # the hold the incremental-delivery assertion needs
            reply = FINISH_TURN
        else:
            reply = SHELL_TURN
        self.wfile.write(reply)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def config(port: int) -> str:
    return f"""# Streaming probe (D-249): a local chat-completions server, no credentials, no network.
skills_paths = []

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "stream-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
max_retries = 0
"""


def exec_args(state_root: pathlib.Path, workspace: pathlib.Path, mode: str, prompt: str) -> list:
    return [str(BIN), "exec", mode, "--full-auto", "--state-root", str(state_root), "--cwd", str(workspace),
            "--timeout", "60", prompt]


def read_line(child: subprocess.Popen, seconds: float) -> str:
    """One line from the child's stdout, or `""` when it does not arrive within `seconds`."""
    ready, _, _ = select.select([child.stdout], [], [], seconds)
    if not ready:
        return ""
    return child.stdout.readline().decode("utf-8", "replace")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-stream-json)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    root = pathlib.Path(args.state_dir or "/tmp/ta-stream-json")
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    shutil.rmtree(root, ignore_errors=True)
    workspace = root / "ws"
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeChat)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    (root / "config/teamagents/config.toml").write_text(config(port))
    env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           "PATH": os.environ.get("PATH", "/usr/bin:/bin"), KEY_VAR: "not-a-key", "HOME": str(root)}
    failures: list[str] = []

    # 1. the streaming run: read the first line while the process is still going
    streaming_root = root / "streaming"
    child = subprocess.Popen(exec_args(streaming_root, workspace, "--stream-json", "run the command"),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    atexit.register(leak_guard.stop_daemons, streaming_root)
    first = read_line(child, 30)
    alive_at_first_line = child.poll() is None
    print(f"1. the first line arrived while the run was going: {alive_at_first_line} → {first.strip()[:90]!r}")
    if not first.strip():
        failures.append("no first line arrived within 30 s")
    if not alive_at_first_line:
        failures.append("the first line only arrived after the run ended: the mode buffered the whole stream")
    rest = child.stdout.read().decode("utf-8", "replace")
    stderr_text = child.stderr.read().decode("utf-8", "replace")
    code = child.wait()
    lines = [line for line in (first + rest).splitlines() if line.strip()]
    parsed = []
    for line in lines:
        try:
            parsed.append(json.loads(line))
        except json.JSONDecodeError as error:
            failures.append(f"a streamed line is not one JSON object ({error}): {line[:120]!r}")

    # 2. the lines are the log in order, each once
    events = parsed[:-1]
    sequences = []
    for line in events:
        if line.get("type") != "event":
            failures.append(f"a line before the report is not an event: {line}")
            continue
        sequences.append(line["event"]["sequence"])
    ordered = all(b > a for a, b in zip(sequences, sequences[1:]))
    print(f"2. {len(events)} event line(s), sequences={sequences}, strictly increasing={ordered}")
    if not sequences:
        failures.append("the stream carried no event at all")
    if not ordered:
        failures.append(f"the streamed sequences are not a strictly increasing run: {sequences}")

    # 3. the last line is the report, and it names the watermark the lines reached
    report_lines = [line for line in parsed if line.get("type") == "report"]
    report = report_lines[0].get("report", {}) if report_lines else {}
    print(f"3. one report line: {len(report_lines) == 1}, end={report.get('end')!r}, "
          f"watermark={report.get('watermark')!r}, exit={code}")
    if len(report_lines) != 1 or parsed[-1].get("type") != "report":
        failures.append(f"the stream must end with exactly one report line: {[l.get('type') for l in parsed]}")
    if report.get("end") != "completed":
        failures.append(f"the scripted run settles SUCCEEDED: end={report.get('end')!r} stderr={stderr_text[-200:]!r}")
    if sequences and report.get("watermark") != sequences[-1]:
        failures.append(f"the report must name the last streamed sequence: {report.get('watermark')} vs {sequences[-1]}")
    if code != 0:
        failures.append(f"a completed run exits 0: exit={code} stderr={stderr_text[-200:]!r}")

    # 4. the report is the object `--json` prints: the same field set, in a session of its own
    plain_root = root / "plain"
    plain = subprocess.run(exec_args(plain_root, workspace, "--json", "run the command"),
                           capture_output=True, text=True, env=env, timeout=180)
    atexit.register(leak_guard.stop_daemons, plain_root)
    plain_lines = [line for line in plain.stdout.splitlines() if line.strip()]
    plain_report = json.loads(plain_lines[0]) if plain_lines else {}
    same_fields = sorted(plain_report) == sorted(report)
    print(f"4. --json printed {len(plain_lines)} line(s), same field set as the streamed report: {same_fields} "
          f"({len(report)} field(s))")
    if len(plain_lines) != 1:
        failures.append(f"--json prints exactly one object: {plain_lines[:2]}")
    if not same_fields:
        failures.append(f"the streamed report must be the `--json` report: "
                        f"{sorted(set(plain_report) ^ set(report))}")

    # 5. a consumer that closes stdout ends the stream, not the run (`| head`)
    early_root = root / "early"
    child = subprocess.Popen(exec_args(early_root, workspace, "--stream-json", "run the command"),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
    atexit.register(leak_guard.stop_daemons, early_root)
    first = read_line(child, 30)
    child.stdout.close()   # the reader goes away, as `| head` does
    stderr_text = child.stderr.read().decode("utf-8", "replace")
    code = child.wait()
    complained = any(word in stderr_text for word in ("Broken pipe", "writing the event stream", "writing the report"))
    print(f"5. closed consumer: first line read={bool(first.strip())}, exit={code}, complained={complained}")
    if code != 0:
        failures.append(f"a closed consumer must not change the run's outcome (0): exit={code} stderr={stderr_text!r}")
    if complained:
        failures.append(f"a closed consumer is not an error: stderr={stderr_text!r}")

    # 6. the refusals, before anything starts
    both = subprocess.run([str(BIN), "exec", "--json", "--stream-json", "hi"], capture_output=True, text=True,
                          env=env, timeout=60)
    wrong_verb = subprocess.run([str(BIN), "instances", "--stream-json"], capture_output=True, text=True,
                                env=env, timeout=60)
    print(f"6. refusals: both={both.returncode} ({both.stderr.strip().splitlines()[:1]}), "
          f"wrong verb={wrong_verb.returncode} ({wrong_verb.stderr.strip().splitlines()[:1]})")
    if both.returncode != 2 or "--json" not in both.stderr or "--stream-json" not in both.stderr:
        failures.append(f"the two shapes must be refused together, by name: {both.returncode} {both.stderr!r}")
    if wrong_verb.returncode != 2 or "--stream-json" not in wrong_verb.stderr:
        failures.append(f"a verb without a stream must refuse the flag by name: "
                        f"{wrong_verb.returncode} {wrong_verb.stderr!r}")

    for state_root in (streaming_root, plain_root, early_root):
        leak_guard.stop_daemons(state_root)
    left = (len(leak_guard.daemon_pids(root)), len(leak_guard.runner_pids(root)))
    print(f"7. cleaned up: daemons/runners left = {left} (the local service saw {FakeChat.seen} request(s))")
    if left != (0, 0):
        failures.append(f"the probe left processes behind: {left}")
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
