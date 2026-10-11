#!/usr/bin/env python3
"""D-451 deterministic coding workflows through real CLI/daemon/runner and loopback HTTP.

No credentials or real models. All mutations and signals target this probe's isolated child processes.
The same --bin option exercises an installed release artifact without changing the user's installation.
"""
import argparse
import base64
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
FIX = "python3 -c \"from pathlib import Path; p=Path('calculator.py'); p.write_text(p.read_text().replace('a - b', 'a + b'))\""
TEST = "python3 -B test_calc.py"


def tool(name, args):
    return {"tool_calls": [{"index": 0, "id": f"call-{name}", "type": "function",
                            "function": {"name": name, "arguments": json.dumps(args)}}]}


def shell(command):
    return tool("shell", {"command": command, "timeout": 30})


def finish():
    return tool("finish", {"status": "success", "summary": "scripted candidate"})


def wait(predicate, what, seconds=20):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(0.02)
    raise AssertionError(f"timed out: {what}")


class Fixture:
    def __init__(self, binary, tag, script, gate=None):
        self.binary = binary
        self.temp = tempfile.TemporaryDirectory(prefix=f"teamagents-maturity-{tag}-", dir="/var/tmp")
        self.dir = Path(self.temp.name)
        self.ws = self.dir / "workspace"
        self.state = self.dir / "state"
        self.ws.mkdir()
        (self.dir / "home").mkdir()
        config = self.dir / "config/teamagents"
        config.mkdir(parents=True)
        self.seen, self.errors, self.children = [], [], []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                try:
                    request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                    owner.seen.append(request)
                    index = len(owner.seen) - 1
                    if gate is not None and index == 0:
                        assert gate.wait(30), "fixture gate timed out"
                    assert index < len(script), "script exhausted"
                    delta = script[index]
                    delta = json.loads(json.dumps(delta))
                    if "tool_calls" in delta:
                        delta["tool_calls"][0]["id"] += f"-{index}"
                    ending = "tool_calls" if "tool_calls" in delta else "stop"
                    chunks = [{"choices": [{"delta": delta}]},
                              {"choices": [{"delta": {}, "finish_reason": ending}],
                               "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8}}]
                    body = ("".join("data: " + json.dumps(c) + "\n\n" for c in chunks) + "data: [DONE]\n\n").encode()
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    pass
                except Exception as error:
                    owner.errors.append(str(error))
                    self.send_error(500, "fixture error")

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        config.joinpath("config.toml").write_text(f'''skills_paths = []
[models.leader_main]
provider = "openai"
protocol = "openai"
model = "deterministic-maturity-fixture"
base_url = "http://127.0.0.1:{self.server.server_port}/v1"
api_key_env = "TEAMAGENTS_MATURITY_FIXTURE_KEY"
context_window = 128000
images = true
timeout = 40
max_retries = 0
''')
        self.env = {"PATH": os.environ["PATH"], "HOME": str(self.dir / "home"),
                    "XDG_CONFIG_HOME": str(self.dir / "config"), "XDG_STATE_HOME": str(self.dir / "xdg-state"),
                    "LANG": "C.UTF-8", "TERM": "xterm-256color", "TEAMAGENTS_MATURITY_FIXTURE_KEY": "fake-local-key"}
        self.daemon_log = (self.dir / "daemon.log").open("w+")
        self.daemon = None
        self.gate = gate
        self.start_daemon()

    def argv(self, *args):
        return [str(self.binary), "--state-root", str(self.state), *args]

    def start_daemon(self):
        self.daemon = subprocess.Popen(self.argv("daemon", "--cwd", str(self.ws), "--full-auto"),
                                       env=self.env, stdout=self.daemon_log, stderr=self.daemon_log)
        self.children.append(self.daemon)
        wait(lambda: self.daemon.poll() is not None or (self.state / "daemon.sock").exists(), "daemon startup")
        if self.daemon.poll() is not None:
            self.daemon_log.seek(0)
            raise AssertionError(self.daemon_log.read())
        wait(lambda: self.run("status", "--json", check=False).returncode == 0, "daemon ready")

    def run(self, *args, check=True):
        result = subprocess.run(self.argv(*args), env=self.env, capture_output=True, text=True, timeout=60)
        if check:
            assert result.returncode == 0, (args, result.returncode, result.stdout, result.stderr)
        return result

    def data(self, *args):
        return json.loads(self.run(*args, "--json").stdout)

    def submit(self, text, *args):
        child = subprocess.Popen(self.argv("exec", "--json", "--timeout", "40", *args, text),
                                 env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.children.append(child)
        return child

    def collect(self, child, success=True):
        out, err = child.communicate(timeout=50)
        assert (child.returncode == 0) == success, (child.returncode, out, err)
        return json.loads(out)

    def sql(self, query):
        with contextlib.closing(sqlite3.connect(f"file:{self.state}/session.sqlite?mode=ro", uri=True)) as db:
            return db.execute(query).fetchall()

    def evidence(self, tag):
        assert not self.errors, self.errors
        requests = json.dumps(self.seen, sort_keys=True).encode()
        facts = {"scenario": tag, "requests": len(self.seen), "request_sha256": hashlib.sha256(requests).hexdigest(),
                 "goals": self.sql("SELECT status FROM goals ORDER BY rowid"),
                 "operations": self.sql("SELECT status FROM operations ORDER BY rowid"),
                 "check_rounds": self.sql("SELECT COUNT(*) FROM events WHERE kind='check_round_registered'")[0][0],
                 "repairs": self.sql("SELECT COUNT(*) FROM events WHERE kind='completion_repair'")[0][0]}
        print(json.dumps(facts), flush=True)

    def close(self):
        if self.gate is not None:
            self.gate.set()
        self.run("daemon", "--stop", check=False)
        for child in self.children:
            if child.poll() is None:
                try:
                    child.wait(timeout=6)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=6)
        # All runner groups for this session must be gone before owned scratch removal.
        leftovers = self.run("runners", "--json", check=False)
        assert leftovers.returncode == 0, leftovers.stderr
        rows = json.loads(leftovers.stdout)["runners"]
        assert all(row["runner"] == "gone" and not row.get("service") for row in rows), rows
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        self.daemon_log.close()
        self.temp.cleanup()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        if args[0] is not None:
            print(json.dumps({"fixture_errors":self.errors,
                "last_messages": self.seen[-1].get("messages", [])[-6:] if self.seen else [],
                "repairs":self.sql("SELECT payload_json FROM events WHERE kind='completion_repair'")}), flush=True)
        self.close()


def coding(binary, repair=False, exhaust=False):
    tag = "checks-exhaust" if exhaust else "checks-repair" if repair else "read-modify-test"
    steps = [finish()] * 3 if exhaust else [finish(), shell(FIX), finish()] if repair else [shell("cat calculator.py"), shell(FIX), shell(TEST), finish()]
    with Fixture(binary, tag, steps) as f:
        f.ws.joinpath("calculator.py").write_text("def add(a, b):\n    return a - b\n")
        f.ws.joinpath("test_calc.py").write_text("from calculator import add\nassert add(2, 3) == 5\nprint('tests passed')\n")
        child = f.submit("Read, repair and test calculator.py", "--accept", "calculator=" + TEST)
        report = f.collect(child, success=not exhaust)
        assert report["goal_status"] == ("BLOCKED" if exhaust else "SUCCEEDED"), report
        if not exhaust:
            subprocess.run(["python3", "-B", "test_calc.py"], cwd=f.ws, check=True)
            assert "a + b" in f.ws.joinpath("calculator.py").read_text()
        else:
            assert "a - b" in f.ws.joinpath("calculator.py").read_text()
        repairs = f.sql("SELECT COUNT(*) FROM events WHERE kind='completion_repair'")[0][0]
        assert repairs == (2 if exhaust else 1 if repair else 0), repairs
        f.evidence(tag)


def crash(binary):
    command = "printf 'once\\n' >> effects.log; touch started; sleep 2; printf done > result.txt"
    with Fixture(binary, "crash", [shell(command), finish()]) as f:
        client = f.submit("Execute one durable side effect")
        wait(lambda: f.ws.joinpath("started").exists(), "running tool marker")
        f.daemon.kill()  # exact owned process, SIGKILL: this is not graceful shutdown
        f.daemon.wait(timeout=5)
        client.communicate(timeout=10)  # connection loss is an expected client outcome
        f.start_daemon()
        wait(lambda: f.sql("SELECT status FROM goals") == [("SUCCEEDED",)], "recovered goal", 30)
        assert f.ws.joinpath("effects.log").read_text() == "once\n"
        assert f.ws.joinpath("result.txt").read_text() == "done"
        journals = list(f.state.glob("instances/*/jobs/*/journal.json"))
        assert journals, "no real runner journal"
        assert all(json.loads(p.read_text())["starts"] == 1 for p in journals)
        assert len(f.seen) == 2
        f.evidence("daemon-sigkill-recovery")


def queued(binary):
    gate = threading.Event()
    with Fixture(binary, "queue", [{"content": "first reply"}, finish()], gate) as f:
        name = "attachment space \u03bb.txt"
        f.ws.joinpath(name).write_text("immutable attachment original")
        image = "one.png"
        f.ws.joinpath(image).write_bytes(base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6PbUAAAAASUVORK5CYII="))
        paths = f.data("inputs", "paths", "--query", "attachment")
        assert name in json.dumps(paths, ensure_ascii=False), paths
        first = f.submit("hold this request")
        wait(lambda: len(f.seen) == 1, "first model request")
        edited = f.submit("original queued", "--attach", name, "--attach", image)
        cancelled = f.submit("cancel this queued input")
        rows = wait(lambda: (r if len(r := f.data("inputs", "list")["inputs"]) == 2 else None), "two pending inputs")
        by_text = {row["payload"]["text"]: row for row in rows}
        a, b = by_text["original queued"], by_text["cancel this queued input"]
        f.data("inputs", "edit", "--id", a["envelope_id"], "--revision", str(a["revision"]), "--prompt", "revised queued")
        f.data("inputs", "cancel", "--id", b["envelope_id"], "--revision", str(b["revision"]))
        refusal = f.run("inputs", "edit", "--id", a["envelope_id"], "--revision", "0", "--prompt", "stale", check=False)
        assert refusal.returncode == 1
        f.ws.joinpath(name).unlink()
        f.ws.joinpath(image).unlink()
        cancelled_report = f.collect(cancelled, success=False)
        assert "cancelled" in json.dumps(cancelled_report).lower(), cancelled_report
        gate.set()
        f.collect(first)
        f.collect(edited)
        assert len(f.seen) == 2, len(f.seen)
        sent = json.dumps(f.seen[-1]["messages"], ensure_ascii=False)
        assert "immutable attachment original" in sent and "revised queued" in sent, sent
        assert "cancel this queued input" not in sent and "original queued" not in sent
        assert "data:image/png;base64," in sent and "image_url" in sent
        revisions = f.sql("SELECT revision,action FROM input_revisions ORDER BY rowid")
        assert (1, "edited") in revisions and (1, "cancelled") in revisions, revisions
        assert f.sql("SELECT COUNT(*) FROM input_attachments")[0][0] == 2
        assert f.data("inputs", "list")["inputs"] == []
        f.evidence("queued-attachment-edit-cancel")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bin", type=Path, default=ROOT / "engine/target/debug/teamagents")
    args = parser.parse_args()
    binary = args.bin.resolve()
    print(json.dumps({"binary": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                      "model": "deterministic loopback fixture", "real_model_calls": 0}), flush=True)
    coding(binary)
    coding(binary, repair=True)
    coding(binary, exhaust=True)
    crash(binary)
    queued(binary)
    print("PASS: five deterministic local workflow cases; no real-model quality claim", flush=True)


if __name__ == "__main__":
    main()
