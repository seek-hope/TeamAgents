#!/usr/bin/env python3
"""PTY smoke for the v2 conversation interface (R19-b②): a fake v2 session
daemon (Unix-socket JSON lines, §9 greeting + scripted reads) plus a real
terminal. Boots teamagents-tui --daemon, types a message, asserts the
submit_input frame and the event-driven history refresh on screen.

Requires: built tui binary (tui/target/debug/teamagents-tui).
"""
import fcntl, json, os, pty, re, select, socket, struct, subprocess, sys, tempfile, termios, threading, time

BIN = os.path.join(os.path.dirname(__file__), "..", "target", "debug", "teamagents-tui")
ENV = dict(os.environ, TERM="xterm-256color")

INSTANCES = [
    {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY"},
    {"id": "i-worker", "lifecycle": "ACTIVE", "phase": "WAITING"},
]
GOAL = {"status": "ACTIVE", "known_usage": {"prompt": 7, "completion": 3, "total": 10},
        "unknown_usage": 0, "limits": {"max_total_tokens": 1000}}


class FakeDaemon:
    """Minimal v2 daemon: greeting first, then method-dispatched replies."""

    def __init__(self, sock_path):
        self.sock_path = sock_path
        self.sequence = 0
        self.events = []
        self.history = []
        self.frames = []
        self.error = None
        self.thread = threading.Thread(target=self.serve, daemon=True)

    def start(self):
        self.thread.start()

    def reply(self, request_id, ok, payload):
        body = {"request_id": request_id}
        if ok:
            body["ok"] = True
            body["result"] = payload
        else:
            body["ok"] = False
            body["error"] = payload
        return (json.dumps(body) + "\n").encode()

    def emit(self, kind, scope, payload):
        self.sequence += 1
        self.events.append({"sequence": self.sequence, "kind": kind, "scope": scope, "payload": payload})

    def serve(self):
        try:
            if os.path.exists(self.sock_path):
                os.unlink(self.sock_path)
            listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            listener.bind(self.sock_path)
            listener.listen(4)
            while True:
                conn, _ = listener.accept()
                threading.Thread(target=self.serve_conn, args=(conn,), daemon=True).start()
        except Exception as e:  # pragma: no cover - surfaced in main
            self.error = f"fake daemon listener: {e}"

    def serve_conn(self, conn):
        try:
            f = conn.makefile("rwb")
            f.write((json.dumps({"server": "teamagents-daemon", "protocol_version": 1,
                                 "session_id": "s-test", "state_root": "/tmp/fake-root"}) + "\n").encode())
            f.flush()
            for raw in f:
                frame = json.loads(raw.decode())
                self.frames.append(frame)
                request_id = frame.get("request_id")
                method = frame.get("method")
                params = frame.get("params", {})
                if method == "checkpoint":
                    out = {"snapshot": {"instances": INSTANCES, "goal": GOAL}, "watermark": self.sequence}
                elif method == "history":
                    out = {"instance_id": params.get("instance_id", ""), "entries": list(self.history)}
                elif method == "approvals":
                    out = {"approvals": []}
                elif method == "events":
                    since = params.get("since", 0)
                    out = {"events": [e for e in self.events if e["sequence"] > since],
                           "watermark": self.sequence, "resync_required": False}
                elif method == "submit_input" and frame.get("command_id"):
                    text = params.get("text", "")
                    self.history.append({"idx": len(self.history) + 1, "kind": "user",
                                         "message": {"role": "user", "content": text}})
                    self.emit("input", params.get("instance_id", ""),
                              {"envelope_id": params.get("envelope_id", ""), "applied": True})
                    out = {"applied": True, "epoch": 0}
                else:
                    f.write(self.reply(request_id, False, f"unknown method {method!r}"))
                    f.flush()
                    continue
                f.write(self.reply(request_id, True, out))
                f.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass
        except Exception as e:  # pragma: no cover - surfaced in main
            self.error = f"fake daemon connection: {e}"


def read_all(fd, timeout=1.5):
    out = b""
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.1)
        if r:
            try:
                out += os.read(fd, 65536)
            except OSError:
                break
    return out


def screen(raw: bytes) -> str:
    txt = raw.decode("utf-8", "replace")
    txt = re.sub(r"\x1b\][^\x07]*\x07", "", txt)
    txt = re.sub(r"\x1b\[[0-9;?]*[a-zA-Z]", "", txt)
    txt = re.sub(r"\x1b[()][0-9A-B]", "", txt)
    txt = re.sub(r"\x1b[=>]", "", txt)
    return txt


def main():
    workdir = tempfile.mkdtemp(prefix="ta-v2-pty-")
    daemon = FakeDaemon(os.path.join(workdir, "daemon.sock"))
    daemon.start()
    for _ in range(100):
        if os.path.exists(daemon.sock_path):
            break
        if daemon.error:
            print(f"FAIL: {daemon.error}")
            return 1
        time.sleep(0.02)

    pid, fd = pty.fork()
    if pid == 0:
        os.execvpe(BIN, [BIN, "--daemon", daemon.sock_path], ENV)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 96, 0, 0))
    failures = []

    def expect(needle, label, raw):
        if needle not in screen(raw):
            failures.append(f"{label}: {needle!r} not on screen\n{screen(raw)[:2000]}")

    boot = read_all(fd, 4.0)
    expect("s-test", "status session", boot)
    expect("i-leader [READY]", "status instance phase", boot)
    expect("ACTIVE", "status goal", boot)
    expect("10/1000", "status budget", boot)
    expect("i-leader", "composer target", boot)
    expect("Enter", "footer", boot)

    # typing + Enter submits one submit_input business frame whose command id
    # carries the envelope (§9), and the input event drives a history refresh
    os.write(fd, "你好v2".encode())
    time.sleep(0.3)
    os.write(fd, b"\r")
    shown = read_all(fd, 4.0)
    expect("你好v2", "conversation after refresh", shown)
    submitted = [f for f in daemon.frames if f.get("method") == "submit_input"]
    if not submitted:
        failures.append("no submit_input frame reached the daemon")
    else:
        frame = submitted[0]
        if not frame.get("command_id", "").startswith("input-env-"):
            failures.append(f"submit_input command_id missing the envelope: {frame.get('command_id')!r}")
        if frame.get("params", {}).get("instance_id") != "i-leader":
            failures.append(f"submit_input targeted {frame.get('params', {}).get('instance_id')!r}")

    # Ctrl+C quits
    os.write(fd, b"\x03")
    read_all(fd, 1.0)
    try:
        os.waitpid(pid, os.WNOHANG)
    except ChildProcessError:
        pass

    if daemon.error:
        failures.append(daemon.error)
    if failures:
        print("FAIL:")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("pty v2 smoke: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
