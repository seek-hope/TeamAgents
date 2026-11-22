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
PROFILE = os.environ.get("TA_PTY_TERM", "xterm-256color")
TERM_PROGRAM = os.environ.get("TA_PTY_TERM_PROGRAM", "")
ENV = dict(os.environ, TERM=PROFILE, TEAMAGENTS_GRAPHICS="auto")
if TERM_PROGRAM:
    ENV["TERM_PROGRAM"] = TERM_PROGRAM

INSTANCES = [
    {"id": "i-leader", "lifecycle": "ACTIVE", "phase": "READY", "model": "deepseek-flash", "effort": "high"},
    {"id": "i-worker", "lifecycle": "ACTIVE", "phase": "WAITING"},
]
TASKS = [{"id": "t-smoke", "work_id": "g1", "assignee": "i-worker", "status": "RUNNING"}]
GRANTS = [{"subject": "i-leader", "action": "manage", "resource_scope": "session", "revoked": False}]
WORK = {"status": "ACTIVE", "known_usage": {"prompt": 7, "completion": 3, "total": 10},
        "unknown_usage": 0, "limits": {"max_total_tokens": 1000}}
# D-433: the work *list* (the `work_scopes` read), which is what `/work_scopes` prints — one settled work whose model wrote no
# prose (so its summary is the only account of the work, D-431) and one ACTIVE work the user closes.
WORK_SCOPES = [
    {"id": "work-s-main", "status": "SUCCEEDED", "known_usage": {"total": 41},
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
        self.previews = []
        self.history = []
        self.member_history = {"i-worker": [{"epoch": 0, "idx": 1, "kind": "assistant",
            "message": {"content": "worker-private-pty-marker"}}]}
        self.member_failures = 0
        self.member_reads = 0
        self.roles = {"leader_id": "i-leader", "leader": "lead", "workers": {"mode": "follow"}}
        self.instances = [dict(i) for i in INSTANCES]
        self.tasks = [dict(t) for t in TASKS]
        self.work_scopes = [dict(g) for g in WORK_SCOPES]
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
            f.write((json.dumps({"server": "teamagents-daemon", "protocol_version": 2,
                                 "session_id": "s-test", "state_root": "/tmp/fake-root"}) + "\n").encode())
            f.flush()
            for raw in f:
                frame = json.loads(raw.decode())
                self.frames.append(frame)
                request_id = frame.get("request_id")
                method = frame.get("method")
                params = frame.get("params", {})
                if method == "checkpoint":
                    out = {"snapshot": {"instances": self.instances, "tasks": self.tasks, "work": WORK,
                                        "environment": {"workspace": os.path.dirname(self.sock_path)},
                                        "role_models": self.roles, "models": [
                                            {"key": "lead", "model": "deepseek-flash", "provider": "fixture"},
                                            {"key": "worker-key", "model": "deepseek-flash", "provider": "second"}]},
                           "watermark": self.sequence}
                elif method == "set_role_model" and frame.get("command_id"):
                    if params != {"role": "workers", "model": "worker-key"}:
                        raise ValueError(f"unexpected role selection: {params}")
                    self.roles["workers"] = {"mode": "pinned", "key": params["model"]}
                    out = self.roles
                elif method == "history":
                    instance = params.get("instance_id", "")
                    if instance != "i-leader":
                        self.member_reads += 1
                        if self.member_failures:
                            self.member_failures -= 1
                            f.write(self.reply(request_id, False, "fixture member history unavailable"))
                            f.flush()
                            continue
                    out = {"instance_id": instance, "entries": list(self.history if instance == "i-leader"
                        else self.member_history.get(instance, []))}
                elif method == "approvals":
                    out = {"approvals": list(self.approvals)}
                elif method == "previews":
                    out = {"previews": list(self.previews)}
                elif method == "input_paths":
                    out = {"prefix": params.get("prefix", ""),
                           "paths": ["src/main.rs", "src/mod.rs"], "limited": False}
                elif method in ("approve", "deny") and frame.get("command_id"):
                    self.approvals = [a for a in self.approvals if a["id"] != params.get("approval_id")]
                    out = {"decided": True}
                elif method == "interrupt_instance" and frame.get("command_id"):
                    out = {"interrupted": False, "reason": "no model request is in flight"}
                elif method == "tasks":
                    out = {"tasks": list(self.tasks)}
                elif method == "work_scopes":
                    out = {"work_scopes": list(self.work_scopes)}
                elif method == "cancel_work" and frame.get("command_id"):
                    work_id = params.get("work_id", "")
                    for work in self.work_scopes:
                        if work["id"] == work_id:
                            work["status"] = "CANCELLED"
                    if work_id not in [work["id"] for work in self.work_scopes]:
                        f.write(self.reply(request_id, False, f"no such work: {work_id}"))
                        f.flush()
                        continue
                    out = {"work_id": work_id, "status": "CANCELLED"}
                elif method == "grants":
                    out = {"grants": list(GRANTS)}
                elif method == "events":
                    since = params.get("since", 0)
                    out = {"events": [e for e in self.events if e["sequence"] > since],
                           "watermark": self.sequence, "head_watermark": self.sequence,
                           "has_more": False, "resync_required": False}
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
                elif method == "rewind_points" and params.get("files"):
                    out = {"mode": "conversation_and_files", "scope": "git_tracked_workspace", "envelope": "pty-point",
                           "epoch": 0, "head": 4, "revision": 2, "files": [{"path": "sample", "status": "snapshot_difference"}],
                           "preview_digest": "pty-token", "apply_available": True}
                elif method == "rewind_points":
                    out = {"mode": "conversation", "envelope": "pty-point", "head": 4, "revision": 2,
                           "epoch": 0, "draft": {"text": "/quit"}}
                elif method == "rewind_conversation" and frame.get("command_id"):
                    expected = {"mode": "conversation", "envelope": "pty-point", "head": 4, "revision": 2, "epoch": 0}
                    if params.get("mode") == "conversation_and_files":
                        expected.update(mode="conversation_and_files", preview_digest="pty-token")
                    assert params == expected, params
                    out = {"envelope": "pty-point", "draft": {"text": "/quit"}, "submitted": False}
                elif method == "send_rewind_draft" and frame.get("command_id"):
                    out = {"applied": True}
                elif method == "request_compaction" and frame.get("command_id"):
                    out = {"queued": True, "epoch": 0}
                elif method == "submit_input" and params.get("after") == "work" and frame.get("command_id"):
                    out = {"queued": True, "after": "work", "deferred": True, "applied": False}
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
        os.execvpe(BIN, [BIN, "--daemon", daemon.sock_path], dict(ENV, SHELL="/bin/sh", ENV="/dev/null"))
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 96, 0, 0))
    failures = []
    scr = Screen(96, 36)

    def expect(needle, label):
        text = "\n".join(scr.lines())
        if needle not in text:
            failures.append(f"{label}: {needle!r} not on screen\n{text[:2000]}")

    scr.feed(read_all(fd, 4.0).decode("utf-8", "replace"))
    expect("READY", "sidebar status phase")
    expect("10/1000", "status budget")
    expect("Message", "composer")
    expect("Enter", "footer")
    expect("? shortcuts", "ready help hint")
    expect("Ctrl+S sessions", "session browser hint")
    if "newline" in scr.lines()[-1] or "Ctrl+J" in scr.lines()[-1]:
        failures.append("Ready footer still advertises newline")
    expect("Ctrl+Q quit", "quit hint")
    expect("deepseek-flash(fixture) · high", "active profile")
    if "Ctrl+C interrupt" in "\n".join(scr.lines()):
        failures.append("Ready footer prioritizes an inactive interrupt")

    # Help and model overlays own keys, paste and the caret. They must not
    # submit or put hidden text into the underlying draft.
    os.write(fd, b"?")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    expect("Shortcuts", "help overlay")
    expect("Input editing", "grouped help")
    os.write(fd, b"\x1b[200~hidden paste\x1b[201~\r")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    if any(f.get("method") == "submit_input" for f in daemon.frames):
        failures.append("help dispatched input")
    os.write(fd, b"\x1b")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    def click(text):
        for y, line in enumerate(scr.lines(), 1):
            if text in line:
                x = line.index(text) + 1
                os.write(fd, f"\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m".encode())
                return
        raise AssertionError(f"missing click target {text}")

    click("[Model]")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    expect("Choose a chat model", "model overlay")
    expect("deepseek-flash(fixture)", "chat model label")
    os.write(fd, b"\t\x1b[200~deepseek-flash(second)\x1b[201~\x1b[C")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    expect("Choose a model for Workers", "Workers role")
    expect("> deepseek-flash(second)", "pasted model label selects the intended account")
    if any(f.get("method") == "set_role_model" for f in daemon.frames):
        failures.append("filtering submitted a selection")
    click("> deepseek-flash(second)")
    click("[Save]")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    if daemon.roles["workers"] != {"mode": "pinned", "key": "worker-key"}:
        failures.append("confirmation did not submit the Worker pin")
    os.write(fd, b"/model\r")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    os.write(fd, b"\x1b[200~another hidden paste\x1b[201~")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    click("[Back]")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    if "hidden paste" in "\n".join(scr.lines()):
        failures.append("an overlay accepted paste into its hidden draft")
    os.write(fd, b"one two\x1b\x7f\x19")  # Alt+Backspace, Ctrl+Y
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    expect("one two", "word deletion and restore")
    os.write(fd, b"\x15")  # Ctrl+U clears the line without submitting
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))

    # Shift+Enter uses the CSI-u modifier report; it grows the composer without
    # submitting. Ctrl+J is retired and must not add another line or send text.
    os.write(fd, "first-line".encode())
    time.sleep(0.2)
    os.write(fd, b"\x0a\x1b[13;2u")
    time.sleep(0.2)
    os.write(fd, "second-line".encode())
    time.sleep(0.3)
    scr.feed(read_all(fd, 3.0).decode("utf-8", "replace"))
    expect("first-line", "composer first line")
    expect("second-line", "composer second line")
    expect("Ctrl+S sessions", "nonempty draft session hint")
    if "newline" in scr.lines()[-1] or "Ctrl+J" in scr.lines()[-1]:
        failures.append("Draft footer still advertises newline")
    if any(f.get("method") == "submit_input" for f in daemon.frames):
        failures.append("newline editing submitted the draft before Enter")
    os.write(fd, b"\r")
    time.sleep(0.4)
    scr.feed(read_all(fd, 3.0).decode("utf-8", "replace"))
    multiline = [f for f in daemon.frames
                 if f.get("method") == "submit_input" and f.get("params", {}).get("text") == "first-line\nsecond-line"]
    if not multiline:
        sent = [f.get("params", {}).get("text") for f in daemon.frames if f.get("method") == "submit_input"]
        failures.append(f"Shift+Enter did not submit one two-line prompt; submitted: {sent!r}")

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

    # Grok-style @file references complete locally, stage only the selected path,
    # and submit the attachment together with the unchanged surrounding prompt.
    os.write(fd, b"@src/m\t")
    scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
    expect("src/main.rs", "@file completion")
    os.write(fd, b"\x1b[B\r")
    scr.feed(read_all(fd, 0.5).decode("utf-8", "replace"))
    expect("staged from @reference", "@file selection receipt")
    os.write(fd, b"inspect\r")
    scr.feed(read_all(fd, 1.0).decode("utf-8", "replace"))
    mentions = [f for f in daemon.frames if f.get("method") == "submit_input"
                and f.get("params", {}).get("attachments") == ["src/mod.rs"]]
    if len(mentions) != 1 or mentions[0]["params"].get("text") != "@src/mod.rs inspect":
        failures.append("@file reference did not preserve its prompt and selected attachment")

    # Retired navigation cannot redirect input or open a management surface.
    os.write(fd, b"\x0e\tstill-leader\r")
    scr.feed(read_all(fd, 1.0).decode("utf-8", "replace"))
    if any(f["params"]["instance_id"] != "i-leader" for f in daemon.frames if f["method"] == "submit_input"):
        failures.append("a message was redirected away from the Leader")

    # Team navigation is read-only; a failed member read retries without another event.
    commands_before_browse = sum("command_id" in f for f in daemon.frames)
    daemon.member_failures = 1
    os.write(fd, b"team-draft\x1b[17~\x1b[Bh")  # F6, Down, member history
    deadline = time.monotonic() + 6
    while time.monotonic() < deadline and "worker-private-pty-marker" not in "\n".join(scr.lines()):
        scr.feed(read_all(fd, 0.2).decode("utf-8", "replace"))
    expect("worker-private-pty-marker", "independent member history")
    expect("team-draft", "member browsing keeps draft")
    expect("Message Leader", "member browsing keeps recipient")
    if daemon.member_reads < 2:
        failures.append("member history refresh did not retry its failed read")
    os.write(fd, b"\x1b[200~hidden-member-paste\x1b[201~")
    scr.feed(read_all(fd, 0.3).decode("utf-8", "replace"))
    if commands_before_browse != sum("command_id" in f for f in daemon.frames):
        failures.append("member navigation dispatched a business command")
    os.write(fd, b"\x1b")
    scr.feed(read_all(fd, 0.3).decode("utf-8", "replace"))
    os.write(fd, b"\x15")
    scr.feed(read_all(fd, 0.3).decode("utf-8", "replace"))

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
    os.write(fd, b"\x1b[<0;30;34M\x1b[<0;30;34m\r")
    scr.feed(read_all(fd, 1.0).decode("utf-8", "replace"))
    if not any(f["method"] == "submit_input" and f["params"]["text"] == "draft-survives-resize" for f in daemon.frames):
        failures.append("approval or resize lost or modified the draft")

    os.write(fd, b"/tasks\r")
    scr.feed(read_all(fd, 0.5).decode("utf-8", "replace"))
    expect("t-smoke", "task observation")
    expect("RUNNING", "task outcome")
    # Observe each input on screen before changing the terminal geometry or sending F6.
    os.write(fd, b"\x1b")
    deadline = time.monotonic() + 6
    while time.monotonic() < deadline and "Enter send" not in scr.lines()[-1]:
        scr.feed(read_all(fd, 0.1).decode("utf-8", "replace"))
    expect("Enter send", "task panel returns to the composer")
    scr = Screen(80, 30)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 80, 0, 0))
    deadline = time.monotonic() + 6
    while time.monotonic() < deadline and "Enter send" not in scr.lines()[-1]:
        scr.feed(read_all(fd, 0.1).decode("utf-8", "replace"))
    expect("Enter send", "narrow composer focus before F6")
    os.write(fd, b"\x1b[17~")
    deadline = time.monotonic() + 6
    while time.monotonic() < deadline and "Up/Down member" not in "\n".join(scr.lines()):
        scr.feed(read_all(fd, 0.1).decode("utf-8", "replace"))
    expect("Up/Down member", "narrow member list owns focus")
    task_reads = sum(f["method"] == "tasks" for f in daemon.frames)
    daemon.emit("task_progress", "t-smoke", {})
    scr.feed(read_all(fd, 1.3).decode("utf-8", "replace"))
    if task_reads != sum(f["method"] == "tasks" for f in daemon.frames):
        failures.append("hidden task view kept polling\n" + "\n".join(scr.lines()))
    os.write(fd, b"\x1b")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    expect("t-smoke", "returning task view consumes refresh debt")
    if task_reads >= sum(f["method"] == "tasks" for f in daemon.frames):
        failures.append("visible task view lost refresh debt")
    scr = Screen(140, 36)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 140, 0, 0))
    scr.feed(read_all(fd, 0.3).decode("utf-8", "replace"))
    os.write(fd, b"\x03")
    scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
    expect("Nothing interrupted", "interrupt receipt is read honestly")
    if not any(f["method"] == "interrupt_instance" and f["params"]["instance_id"] == "i-leader" for f in daemon.frames):
        failures.append("Ctrl+C did not interrupt the Leader")
    if os.waitpid(pid, os.WNOHANG)[0]:
        failures.append("Ctrl+C exited the TUI")
    forbidden = {"grants", "work_scopes", "set_lifecycle", "cancel_task", "cancel_work", "spawn_instance", "delegate_task"}
    if any(f["method"] in forbidden for f in daemon.frames):
        failures.append("observation accessed an unrelated surface or dispatched management work")
    os.write(fd, b"\x1b")
    scr.feed(read_all(fd, 0.3).decode("utf-8", "replace"))

    # D-447: Markdown, streamed prefixes, compact tool output and local expansion in a real terminal.
    scr = Screen(140, 48)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 48, 140, 0, 0))
    os.write(fd, b"display-draft")
    daemon.history = [
        {"kind": "assistant", "message": {"tool_calls": [{"id": "render-call", "function": {
            "name": "shell", "arguments": json.dumps({"command": "printf render-proof"})}}]}},
        {"kind": "tool_result", "message": {"tool_call_id": "render-call",
            "content": "\n".join(f"output-{i:02}" for i in range(12))}},
        {"kind": "assistant", "message": {"content": "## Render proof\n\n**Readable answer** with `inline_code`.\n\n```rust\n    let kept = true;\n```\n\n| Check | Result |\n|---|---|\n| Layout | pass |"}},
    ]
    daemon.previews = [{"instance_id": "i-leader", "text": "### Stream origin\n\n" + "continuing words " * 35}]
    daemon.emit("response_imported", "i-leader", {})
    scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
    expect("Render proof", "Markdown heading")
    expect("Readable answer", "emphasis")
    expect("    let kept = true;", "code indentation")
    expect("Stream origin", "stream prefix beyond the old 400-character tail")
    shown = "\n".join(scr.lines())
    if "## Render proof" in shown or "**Readable answer**" in shown or "output-05" in shown:
        failures.append("Markdown stayed raw or compact output did not fold")
    commands = sum("command_id" in f for f in daemon.frames)
    daemon.previews = []
    os.write(fd, b"\x0f")
    scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
    expect("output-05", "expanded tool output")
    expect("display-draft", "expansion preserves the composer")
    if commands != sum("command_id" in f for f in daemon.frames):
        failures.append("tool expansion sent a business command")

    # D-450: completion fills a local command; it cannot submit or interrupt by itself.
    os.write(fd, b"\x17/inte")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    expect("/interrupt", "local completion candidate")
    commands_before_completion = sum("command_id" in f for f in daemon.frames)
    os.write(fd, b"\t")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    expect("/interrupt", "filled local command")
    if commands_before_completion != sum("command_id" in f for f in daemon.frames):
        failures.append("completion dispatched a command")
    os.write(fd, b"\r")
    scr.feed(read_all(fd, 0.5).decode("utf-8", "replace"))
    if commands_before_completion + 1 != sum("command_id" in f for f in daemon.frames):
        failures.append("explicit submit did not dispatch the completed command exactly once")
    os.write(fd, b"/s\x1b")
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    expect("/s", "cancelled completion keeps the draft")

    # Both commands acknowledge queued work; neither claims the provider already acted.
    # Use a wide conversation pane so the receipt assertion is independent of wrapping.
    scr = Screen(220, 48)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 48, 220, 0, 0))
    os.write(fd, b"\x17")  # clear the retained /s draft
    scr.feed(read_all(fd, 0.4).decode("utf-8", "replace"))
    new_frames = len(daemon.frames)
    os.write(fd, b"/compact preserve units\r")
    scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
    expect("Compaction requested, not yet completed", "manual compaction receipt")
    os.write(fd, b"/later check units\r")
    scr.feed(read_all(fd, 0.8).decode("utf-8", "replace"))
    expect("waiting for the current work to settle", "after-work receipt")
    writes = [f for f in daemon.frames[new_frames:] if "command_id" in f]
    if ([f["method"] for f in writes] != ["request_compaction", "submit_input"] or
            writes[0]["params"] != {"instance_id": "i-leader", "focus": "preserve units"} or
            any(writes[1]["params"].get(k) != v for k, v in
                {"after": "work", "text": "check units", "instance_id": "i-leader"}.items())):
        failures.append("maintenance and after-work input did not use separate, Leader-bound commands")

    # The operator shell runs locally in the reported workspace. A failed view and an
    # interactive shell never send input. Only an explicit Enter sends reviewed stdout.
    before_shell = len(daemon.frames)
    def shell_step(data, wait_for, label):
        os.write(fd, data)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            raw = read_all(fd, 0.2)
            scr.feed(raw.decode("utf-8", "replace"))
            if (isinstance(wait_for, bytes) and wait_for in raw or
                    isinstance(wait_for, str) and wait_for in "\n".join(scr.lines())):
                return
        failures.append(f"{label}: terminal did not reach {wait_for!r}")

    shell_step(b"/shell view printf private-view; exit 7\r", "exit status: 7", "view returned to TUI")
    expect("private-view", "private shell stdout")
    expect("exit status: 7", "shell failure restores the TUI")
    shell_step(b"/shell\r", b"\x1b[?1049l", "switched to the shell terminal")
    shell_step(b"pwd > operator-cwd; exit\n", "Local shell exit status: 0", "interactive shell exited")
    marker = os.path.join(workdir, "operator-cwd")
    if not os.path.isfile(marker) or open(marker).read().strip() != os.path.realpath(workdir):
        failures.append("interactive shell did not use the session workspace")
    shell_step(b"/shell draft printf /quit\r", "Review the draft before sending", "shell output staged")
    expect("Review the draft before sending", "shell review boundary")
    if any("command_id" in f for f in daemon.frames[before_shell:]):
        failures.append("local shell sent a business command without review")
    os.write(fd, b"\r")
    scr.feed(read_all(fd, 0.6).decode("utf-8", "replace"))
    writes = [f for f in daemon.frames[before_shell:] if "command_id" in f]
    if len(writes) != 1 or writes[0]["method"] != "submit_input" or writes[0]["params"]["text"] != "Shell output:\n/quit":
        failures.append("reviewed shell output did not submit once as plain Leader input")

    # Follow the preview and the editable draft all the way to the socket. A slash in
    # recovered text is input, not a local command; Esc may discard without sending.
    before_rewind = len(daemon.frames)
    shell_step(b"/rewind preview pty-point\r", "Conversation only.", "preview rewind boundary")
    shell_step(b"/rewind apply\r", "Rewind draft reopened", "reopen prompt")
    shell_step(b"\x1b", "Local rewind draft discarded", "dismiss unsent draft")
    if [f["method"] for f in daemon.frames[before_rewind:] if "command_id" in f] != ["rewind_conversation"]:
        failures.append("preview, apply or discard submitted an unreviewed prompt")
    os.write(fd, b"/rewind draft\r")
    scr.feed(read_all(fd, 0.6).decode("utf-8", "replace"))
    os.write(fd, b" edited\r")
    scr.feed(read_all(fd, 0.6).decode("utf-8", "replace"))
    sent = [f for f in daemon.frames[before_rewind:] if f["method"] == "send_rewind_draft"]
    if len(sent) != 1 or sent[0]["params"].get("text") != "/quit edited" or sent[0]["params"].get("source") != "pty-point":
        failures.append("reviewed rewind draft did not send exactly once as edited text")

    shell_step(b"/rewind files pty-point\r", "snapshot_difference", "preview tracked file changes")
    shell_step(b"/rewind apply\r", "Rewind draft reopened", "restore tracked files and prompt")
    if [f["params"].get("preview_digest") for f in daemon.frames if f["method"] == "rewind_conversation"] != [None, "pty-token"]:
        failures.append("combined rewind did not confirm the displayed file preview")

    # Exit detaches only; it sends no daemon-side business command.
    before_exit = sum("command_id" in f for f in daemon.frames)
    os.write(fd, b"\x11")
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
        failures.append("Ctrl+Q did not quit")
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
    print(f"pty v2 smoke ({PROFILE}): ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
