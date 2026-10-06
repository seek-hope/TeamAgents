#!/usr/bin/env python3
"""The artifact census, the on-demand sweep, and what the sweep can actually reach (D-253).

DESIGN §4.4 schedules artifact collection and history retention "separately" and protects "live references and
evaluation evidence"; this build scheduled collection at a *driver's boot* only (D-191), which ACCEPTANCE's
known gap recorded: a state root whose last driver never boots again keeps its `DELETING` rows and their bytes,
and "a cadence — an interval, or a maintenance verb to run on demand — is the user's call". `teamagents
artifacts [list|gc]` is that on-demand half. This probe runs it against a real session and measures four things:

1. **the census** — a real session's artifacts with their kind, size, completeness, owning fact and whether the
   bytes are really on disk (a fact that was invisible before this verb);
2. **the lock discipline** — with the session live, `gc` refuses, names the coordinator that holds the state
   root and the lever to stop it (`teamagents daemon --stop`, D-248);
3. **the sweep with no session** — after the daemon is stopped, `gc` runs, reports what it claimed, freed and
   skipped, and leaves the catalog consistent;
4. **what it cannot reach, with the number that says so** — every artifact a real session stages carries an
   owner (`driver.rs`: a model response's request, a tool output's operation). **Since D-436 the sweep releases a
   hold whose owner has finished**, so the census after a sweep shows no owner; what still keeps the bytes is the
   *reference* (the event that points at the artifact), and whether that may expire with `[retention] history_days`
   is the policy question D-253 records. Before D-436 nothing ever cleared the owner, and this probe measured the
   catalog size before and after the sweep and prints the unchanged number. `V2Artifact.tla` says the same
   structurally (a LIVE, unreferenced artifact exists there only under the counterfactual `GcIgnoresHolders`),
   which is why the probe asserts the *finding* instead of pretending the sweep collected something. Whether
   those bytes should expire with `[retention] history_days` (they are "ordinary history" in §9's words) or
   under their own knob is a policy the user has not stated.

No credential and no network: a local chat-completions server answers the first turn with a `shell` call whose
output is large enough to become an artifact, and the turn after the tool result with a `finish`.

    python3 review/dogfood/artifacts.py
    python3 review/dogfood/artifacts.py --state-dir /tmp/ta-artifacts
"""
import argparse
import sqlite3
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
sys.path.insert(0, str(REPO / "review"))   # the shared pid-based stop (D-148)
import leak_guard  # noqa: E402

BIN = REPO / "engine/target/debug/teamagents"
KEY_VAR = "TEAMAGENTS_ARTIFACTS_PROBE_KEY"
HEAD = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
# Just over the driver's MAX_OUTPUT (200 KB), so the tool result is published as an artifact.
BIG = "sh -c 'head -c 300000 /dev/zero | tr \"\\0\" \"x\"'"


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
    protocol_version = "HTTP/1.0"

    def do_POST(self):  # noqa: N802 (http.server's name)
        length = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(length) if length else b""
        try:
            conversation = json.loads(body.decode("utf-8", "replace")).get("messages", [])
        except json.JSONDecodeError:
            conversation = []
        if any(message.get("role") == "tool" for message in conversation):
            reply = tool_call("finish", {"status": "success", "summary": "the big output was captured"})
        else:
            reply = tool_call("shell", {"command": BIG, "timeout": 60})
        self.wfile.write(reply)
        self.wfile.flush()
        self.close_connection = True

    def log_message(self, *args):  # keep the probe's output clean
        pass


def config(port: int) -> str:
    return f"""# Artifacts probe (D-253, D-436): a local chat-completions server, no credentials, no network.
skills_paths = []

# D-436: an artifact is ordinary history, so a *finished* owner's hold is released once the artifact is older than
# this window — the same window that sweeps the events referencing it. 0 (the default) releases nothing.
[retention]
history_days = 30

[models.leader_main]
provider = "openai"
protocol = "openai"
model = "artifacts-probe"
base_url = "http://127.0.0.1:{port}/v1"
api_key_env = "{KEY_VAR}"
context_window = 128000
timeout = 30
max_retries = 0
"""


def artifacts(state_root: pathlib.Path, env: dict, verb: str) -> tuple[int, dict, str]:
    done = subprocess.run([str(BIN), "artifacts", verb, "--json", "--state-root", str(state_root)],
                          capture_output=True, text=True, env=env, timeout=120)
    report = json.loads(done.stdout) if done.stdout.strip().startswith("{") else {}
    return done.returncode, report, done.stderr


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-artifacts)")
    args = parser.parse_args()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")
    root = pathlib.Path(args.state_dir or "/tmp/ta-artifacts")
    if not args.state_dir:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
    shutil.rmtree(root, ignore_errors=True)
    workspace, state_root = root / "ws", root / "root"
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeChat)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    (root / "config/teamagents/config.toml").write_text(config(server.server_address[1]))
    env = {"XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state"),
           "PATH": os.environ.get("PATH", "/usr/bin:/bin"), KEY_VAR: "not-a-key", "HOME": str(root)}
    atexit.register(leak_guard.stop_daemons, state_root)
    failures: list[str] = []

    # a real session, with a tool output big enough to be published as an artifact
    done = subprocess.run(
        [str(BIN), "exec", "--full-auto", "--json", "--state-root", str(state_root), "--cwd", str(workspace),
         "--timeout", "120", "give me a big output"],
        capture_output=True, text=True, env=env, timeout=180)
    code, report, err = artifacts(state_root, env, "list")
    rows = report.get("artifacts", [])
    present = sum(1 for row in rows if row.get("bytes_present"))
    print(f"1. exec={done.returncode} catalog: {report.get('count')} artifact(s), {report.get('bytes')} bytes, "
          f"{present} with their bytes on disk")
    if done.returncode != 0 or not rows:
        failures.append(f"the scripted session published no artifact: exit={done.returncode} {report} "
                        f"{done.stderr[-200:]!r}")
    if any(row.get("size", 0) > 200_000 and not row.get("bytes_present") for row in rows):
        failures.append(f"a large artifact's bytes are missing from disk: {rows}")

    # the census names what owns each artifact — the reason the sweep below has nothing to do
    owners = [row.get("owner_ref") for row in rows]
    print(f"2. owners: {owners}")
    if not all(owners):
        failures.append(f"a real session's artifact with no owner (the collector's only candidate): {rows}")

    # with the session live the sweep refuses, and says how to stop it
    code, _report, err = artifacts(state_root, env, "gc")
    print(f"3. gc while the session is live: exit={code} {err.strip()[:90]!r}")
    if code != 1 or "already has a coordinator" not in err or "daemon --stop" not in err:
        failures.append(f"the sweep must refuse beside a live session and name the lever: {code} {err!r}")

    # with no session it runs. D-436's premise: the artifacts have to be *older than the window* for their
    # finished owner's hold to be released, and a probe cannot wait 30 days — so it backdates them, which is the
    # same "arrange the premise in the file the product reads" the sessions-prune probe does.
    stopped = subprocess.run([str(BIN), "daemon", "--stop", "--state-root", str(state_root)],
                             capture_output=True, text=True, env=env, timeout=60)
    print(f"4. daemon --stop: exit={stopped.returncode} {stopped.stdout.strip()[:60]!r}")
    before = report.get("bytes", 0)
    connection = sqlite3.connect(str(state_root / "session.sqlite"))
    connection.execute("UPDATE artifacts SET created = created - 40 * 86400")
    connection.commit()
    connection.close()
    print("4b. backdated the artifacts 40 days (the config asks for a 30-day window)")
    code, swept, err = artifacts(state_root, env, "gc")
    print(f"5. gc with no session: exit={code} collected={swept.get('collected')} "
          f"freed={swept.get('freed_bytes')} catalog_bytes={swept.get('bytes')} (before: {before})")
    if code != 0:
        failures.append(f"the sweep must run with no session: {code} {err!r}")
    if not swept.get("collected"):
        failures.append("a finished owner's aged artifact was not collected: the D-436 release did not happen")
    if not swept.get("freed_bytes"):
        failures.append(f"nothing was freed although an artifact was collected: {swept}")
    # D-436: the sweep released the finished owners' holds and took what nothing referenced any more. What stays
    # is what a *reference* still points at — and whether such a reference may expire with `[retention]
    # history_days` is the rest of D-253's policy question, not this decision's.
    code, listed, err = artifacts(state_root, env, "list")
    roles = [row.get("owner_ref") for row in (listed.get("artifacts") or [])]
    print(f"6. owners after the sweep: {roles} (the finished owners' holds are released, D-436)")
    if code != 0:
        failures.append(f"the census after the sweep must work: {code} {err!r}")
    if any(role is not None for role in roles):
        failures.append(f"a finished owner still holds its artifact after the sweep: {roles}")
    left = [row for row in (listed.get("artifacts") or []) if row.get("bytes_present")]
    taken = len(rows) - len(listed.get("artifacts") or [])
    print(f"7. after the sweep: {taken} of {len(rows)} artifacts collected, {len(left)} still on disk "
          f"(released holds + the references that keep the rest is the D-436 rule)")
    if taken != len(swept.get("collected") or []):
        failures.append(f"the census does not agree with the sweep: {taken} vs {swept.get('collected')}")

    leak_guard.stop_daemons(state_root)
    left = (len(leak_guard.daemon_pids(root)), len(leak_guard.runner_pids(root)))
    print(f"8. cleaned up: daemons/runners left = {left}")
    if left != (0, 0):
        failures.append(f"the probe left processes behind: {left}")
    for failure in failures:
        print("FAIL:", failure)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
