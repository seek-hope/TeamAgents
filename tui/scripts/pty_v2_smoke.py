#!/usr/bin/env python3
"""PTY smoke for the v2 conversation interface (R19-b②/③): a fake v2 session
daemon (Unix-socket JSON lines, §9 greeting + scripted reads) plus a real
terminal. Boots teamagents-tui --daemon, types a message, asserts the
submit_input frame and the event-driven history refresh on screen, then
checks Leader-only routing, interrupt versus exit, approval ownership,
bracketed paste and resizing (D-445).

Requires: built tui binary (tui/target/debug/teamagents-tui).
"""
import atexit, fcntl, json, os, pty, re, shutil, socket, struct, subprocess, sys, tempfile, termios, threading, time, unicodedata

BIN = os.path.join(os.path.dirname(__file__), "..", "target", "debug", "teamagents-tui")
ENV = dict(os.environ, TERM="xterm-256color")

INSTANCES = [
    {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY"},
    {"id": "i-worker", "lifecycle": "ACTIVE", "phase": "WAITING"},
]
TASKS = [{"id": "t-smoke", "goal_id": "g1", "assignee": "i-worker", "status": "RUNNING"}]
GRANTS = [{"subject": "i-leader", "action": "manage", "resource_scope": "session", "revoked": False}]
GOAL = {"status": "ACTIVE", "known_usage": {"prompt": 7, "completion": 3, "total": 10},
        "unknown_usage": 0, "limits": {"max_total_tokens": 1000}}
# D-433: the goal *list* (the `goals` read), which is what `/goals` prints — one settled goal whose model wrote no
# prose (so its summary is the only account of the work, D-431) and one ACTIVE goal the user closes.
GOALS = [
    {"id": "goal-s-main", "status": "SUCCEEDED", "known_usage": {"total": 41},
     "limits": {"max_total_tokens": 1000}, "attached_instances": [],
     "summary": "answered the question and wrote the report"},
    {"id": "turn-2", "status": "ACTIVE", "known_usage": {"total": 0}, "limits": {},
     "attached_instances": ["i-leader"], "summary": None},
]


class FakeDaemon:
    """Minimal v2 daemon: greeting first, then method-dispatched replies."""

    def __init__(self, sock_path):
        self.sock_path = sock_path
        self.sequence = 0
        self.events = []
        self.history = []
        self.instances = [dict(i) for i in INSTANCES]
        self.tasks = [dict(t) for t in TASKS]
        self.goals = [dict(g) for g in GOALS]
        self.approvals = []
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
                    out = {"snapshot": {"instances": self.instances, "goal": GOAL}, "watermark": self.sequence}
                elif method == "history":
                    out = {"instance_id": params.get("instance_id", ""), "entries": list(self.history)}
                elif method == "approvals":
                    out = {"approvals": list(self.approvals)}
                elif method == "previews":
                    out = {"previews": []}
                elif method in ("approve", "deny") and frame.get("command_id"):
                    self.approvals = [a for a in self.approvals if a["id"] != params.get("approval_id")]
                    out = {"decided": True}
                elif method == "interrupt_instance" and frame.get("command_id"):
                    out = {"interrupted": False, "reason": "no model request is in flight"}
                elif method == "tasks":
                    out = {"tasks": list(self.tasks)}
                elif method == "goals":
                    out = {"goals": list(self.goals)}
                elif method == "cancel_goal" and frame.get("command_id"):
                    goal_id = params.get("goal_id", "")
                    for goal in self.goals:
                        if goal["id"] == goal_id:
                            goal["status"] = "CANCELLED"
                    if goal_id not in [goal["id"] for goal in self.goals]:
                        f.write(self.reply(request_id, False, f"no such goal: {goal_id}"))
                        f.flush()
                        continue
                    out = {"goal_id": goal_id, "status": "CANCELLED"}
                elif method == "grants":
                    out = {"grants": list(GRANTS)}
                elif method == "events":
                    since = params.get("since", 0)
                    out = {"events": [e for e in self.events if e["sequence"] > since],
                           "watermark": self.sequence, "resync_required": False}
                elif method == "set_lifecycle" and frame.get("command_id"):
                    target = params.get("instance_id", "")
                    lifecycle = params.get("lifecycle", "")
                    for inst in self.instances:
                        if inst["id"] == target:
                            inst["lifecycle"] = lifecycle
                    self.emit("instance_lifecycle", target, {"lifecycle": lifecycle, "reason": "tui"})
                    out = {"instance_id": target, "lifecycle": lifecycle}
                elif method == "cancel_task" and frame.get("command_id"):
                    task_id = params.get("task_id", "")
                    for task in self.tasks:
                        if task["id"] == task_id:
                            task["status"] = "CANCELLED"
                    self.emit("task_cancelled", "i-worker", {"task_id": task_id, "reason": "tui"})
                    out = {"task_id": task_id, "status": "CANCELLED"}
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


sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pty_screen import Screen, read_all  # virtual terminal + PTY reader for diff-rendered frames


def main():
    workdir = tempfile.mkdtemp(prefix="ta-v2-pty-")
    # `make pty` runs this once per gate run; a workdir left in TMPDIR accumulates one per run (D-131),
    # and the socket inside it is the fake daemon's. Removed however this function returns.
    atexit.register(shutil.rmtree, workdir, ignore_errors=True)
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
    scr = Screen(96, 36)

    def expect(needle, label):
        text = "\n".join(scr.lines())
        if needle not in text:
            failures.append(f"{label}: {needle!r} not on screen\n{text[:2000]}")

    scr.feed(read_all(fd, 4.0).decode("utf-8", "replace"))
    expect("Ready", "status phase")
    expect("10/1000", "status budget")
    expect("Message", "composer")
    expect("Enter", "footer")
    expect("Ctrl+C interrupt", "interrupt hint")

    # Ctrl+J inserts a newline in the composer (0x0A in raw mode), the composer
    # box grows, and the whole two-line text is what gets submitted
    os.write(fd, "first-line".encode())
    time.sleep(0.2)
    os.write(fd, b"\x0a")
    time.sleep(0.2)
    os.write(fd, "second-line".encode())
    time.sleep(0.3)
    scr.feed(read_all(fd, 3.0).decode("utf-8", "replace"))
    expect("first-line", "composer first line")
    expect("second-line", "composer second line")
    os.write(fd, b"\r")
    time.sleep(0.4)
    scr.feed(read_all(fd, 3.0).decode("utf-8", "replace"))
    multiline = [f for f in daemon.frames
                 if f.get("method") == "submit_input" and f.get("params", {}).get("text") == "first-line\nsecond-line"]
    if not multiline:
        sent = [f.get("params", {}).get("text") for f in daemon.frames if f.get("method") == "submit_input"]
        failures.append(f"Ctrl+J did not submit one two-line prompt; submitted: {sent!r}")

    # D-77: the composer's history is real. Up (the terminal's own escape sequence)
    # recalls what was just sent, and down brings the (empty) draft back — both
    # through the composer's keys, which is what a user actually presses.
    os.write(fd, b"\x1b[A")
    time.sleep(0.3)
    scr.feed(read_all(fd, 2.0).decode("utf-8", "replace"))
    os.write(fd, b"\r")
    time.sleep(0.4)
    recalled = [f for f in daemon.frames
                if f.get("method") == "submit_input" and f.get("params", {}).get("text") == "first-line\nsecond-line"]
    if len(recalled) != 2:
        failures.append(f"Up did not recall the submitted prompt: {len(recalled)} sends of it (want 2)")
    os.write(fd, b"\x1b[B")
    time.sleep(0.3)
    sent_before = len([f for f in daemon.frames if f.get("method") == "submit_input"])
    os.write(fd, b"\r")
    time.sleep(0.4)
    sent_after = len([f for f in daemon.frames if f.get("method") == "submit_input"])
    if sent_after != sent_before:
        failures.append("an empty composer (after Down) submitted something")

    # typing + Enter submits one submit_input business frame whose command id
    # carries the envelope (§9), and the input event drives a history refresh
    os.write(fd, "hello-v2".encode())
    time.sleep(0.3)
    os.write(fd, b"\r")
    scr.feed(read_all(fd, 4.0).decode("utf-8", "replace"))
    expect("hello-v2", "conversation after refresh")
    submitted = [f for f in daemon.frames if f.get("method") == "submit_input"]
    if not submitted:
        failures.append("no submit_input frame reached the daemon")
    else:
        frame = submitted[0]
        if not frame.get("command_id", "").startswith("input-env-"):
            failures.append(f"submit_input command_id missing the envelope: {frame.get('command_id')!r}")
        if frame.get("params", {}).get("instance_id") != "i-leader":
            failures.append(f"submit_input targeted {frame.get('params', {}).get('instance_id')!r}")

    # Retired navigation cannot redirect input or open a management surface.
    os.write(fd, b"\x0e\tstill-leader\r")
    scr.feed(read_all(fd, 1.0).decode("utf-8", "replace"))
    if any(f["params"]["instance_id"] != "i-leader" for f in daemon.frames if f["method"] == "submit_input"):
        failures.append("a message was redirected away from the Leader")

    # Approval arrival does not steal typing; focus is explicit, including paste.
    os.write(fd, b"draft-survives-resize")
    daemon.approvals = [{"id": "ap-1", "operation_id": "op-1", "tool": "shell", "preview": "make check"}]
    scr.feed(read_all(fd, 1.5).decode("utf-8", "replace"))
    expect("Ctrl+A review", "pending approval hint")
    os.write(fd, b"\x01\x1b[200~not-in-draft\x1b[201~")
    scr.feed(read_all(fd, 0.5).decode("utf-8", "replace"))
    for cols, rows in [(140, 36), (40, 18), (140, 36)]:
        scr = Screen(cols, rows)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
        expect("draft-survives-resize", "resized draft")
    os.write(fd, b"d")
    scr.feed(read_all(fd, 0.5).decode("utf-8", "replace"))
    decisions = [f for f in daemon.frames if f["method"] == "deny"]
    if len(decisions) != 1 or decisions[0]["params"]["approval_id"] != "ap-1":
        failures.append("approval did not explicitly deny the selected id")
    os.write(fd, b"\x1b[<0;3;34M\x1b[<0;3;34m\r")
    scr.feed(read_all(fd, 1.0).decode("utf-8", "replace"))
    if not any(f["method"] == "submit_input" and f["params"]["text"] == "draft-survives-resize" for f in daemon.frames):
        failures.append("approval or resize lost or modified the draft")

    os.write(fd, b"/tasks\r")
    scr.feed(read_all(fd, 0.5).decode("utf-8", "replace"))
    expect("Management: teamagents --help", "retired command guidance")
    os.write(fd, b"\x03")
    scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
    expect("Nothing interrupted", "interrupt receipt is read honestly")
    if not any(f["method"] == "interrupt_instance" and f["params"]["instance_id"] == "i-leader" for f in daemon.frames):
        failures.append("Ctrl+C did not interrupt the Leader")
    if os.waitpid(pid, os.WNOHANG)[0]:
        failures.append("Ctrl+C exited the TUI")
    forbidden = {"tasks", "grants", "goals", "set_lifecycle", "cancel_task", "cancel_goal"}
    if any(f["method"] in forbidden for f in daemon.frames):
        failures.append("the TUI still accesses management surfaces")

    # Exit detaches only; it sends no daemon-side business command.
    before_exit = sum("command_id" in f for f in daemon.frames)
    os.write(fd, b"\x04")
    scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
    done, status = os.waitpid(pid, os.WNOHANG)
    for _ in range(40):
        if done == pid:
            break
        time.sleep(0.05)
        done, status = os.waitpid(pid, os.WNOHANG)
    if done != pid:
        os.kill(pid, 15)
        os.waitpid(pid, 0)
        failures.append("Ctrl+D did not exit")
    elif os.waitstatus_to_exitcode(status) != 0:
        failures.append("TUI exit was not clean")
    if before_exit != sum("command_id" in f for f in daemon.frames):
        failures.append("exit sent a business command")

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
