#!/usr/bin/env python3
"""Opt-in real-model terminal acceptance; never used by make pty.

Pass a credential-free DeepSeek Flash 1M profile and a NEW output directory.
Every attempted run keeps its terminal bytes, wire messages, frames and state.
The proxy records the real engine protocol without changing its bytes.
"""

import argparse
import codecs
from datetime import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import secrets
import select
import shutil
import signal
import sqlite3
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import tomllib
import traceback

sys.dont_write_bytecode = True
from pty_click_check import BIN, ENGINE, ROOT, Screen


def now():
    return datetime.now().astimezone().isoformat()


def save(path, value):
    staged = path.with_suffix(path.suffix + ".tmp")
    staged.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")
    staged.replace(path)


def checked_project_files(project, expected):
    """Exclude only the fixture's Git metadata, never arbitrary hidden files."""
    paths = sorted(p for p in project.iterdir() if p.name != ".git")
    names = [p.name for p in paths]
    assert names == sorted(expected), f"unexpected project entries: {names}"
    assert all(p.is_file() and not p.is_symlink() for p in paths), "expected regular project files"
    return paths


def proxy(engine):
    log = open(os.environ["TEAMAGENTS_LIVE_WIRE"], "a", buffering=1)
    lock = threading.Lock()
    child = subprocess.Popen([engine, "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)

    def record(direction, line):
        with lock:
            log.write(json.dumps({"at": now(), "monotonic": time.monotonic(), "direction": direction,
                                  "message": json.loads(line)}, ensure_ascii=False) + "\n")

    def send():
        try:
            for line in sys.stdin.buffer:
                record("request", line)
                child.stdin.write(line)
                child.stdin.flush()
        except BrokenPipeError:
            pass
        finally:
            child.stdin.close()

    threading.Thread(target=send, daemon=True).start()
    try:
        for line in child.stdout:
            record("response", line)
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
    finally:
        if child.poll() is None:
            child.terminate()
        child.wait(timeout=10)
    return child.returncode


class Terminal:
    def __init__(self, case, label, resumed=False):
        self.case, self.label = case, label
        self.screen = Screen(110, 36)
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.wire_path = case.out / (label + ".wire.jsonl")
        self.wire_path.touch()
        self.wire = []
        self.wire_offset = 0
        self.wire_pending = b""
        self.raw = open(case.out / (label + ".terminal.bin"), "wb")
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            env = dict(os.environ, TERM="xterm-256color", XDG_CONFIG_HOME=str(case.out / "config"),
                       XDG_STATE_HOME=str(case.out / "state"), TEAMAGENTS_LIVE_WIRE=str(self.wire_path))
            args = [str(case.tui), "--engine", str(case.out / "engine-proxy"), "--cwd", str(case.project)]
            if resumed:
                args.extend(["--resume", case.session_dir().name])
            else:
                args.extend(["--team", str(case.out / "team.json")])
            os.chdir(case.project)
            os.execvpe(str(case.tui), args, env)
        self.closed = False
        self.resize(110, 36)

    def resize(self, cols, rows):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.screen = Screen(cols, rows)

    def pump(self, seconds=0.05):
        ready, _, _ = select.select([self.fd], [], [], seconds)
        if ready:
            try:
                data = os.read(self.fd, 1 << 20)
            except OSError:
                data = b""
            self.raw.write(data)
            self.raw.flush()
            self.screen.feed(self.decoder.decode(data))
        with self.wire_path.open("rb") as stream:
            stream.seek(self.wire_offset)
            chunk = stream.read()
        self.wire_offset += len(chunk)
        lines = (self.wire_pending + chunk).split(b"\n")
        self.wire_pending = lines.pop()
        self.wire.extend(json.loads(line) for line in lines if line.strip())

    def keys(self, keys, label):
        with (self.case.out / "input.jsonl").open("a") as stream:
            stream.write(json.dumps({"at": now(), "monotonic": time.monotonic(), "terminal": self.label,
                                     "label": label, "bytes_hex": keys.hex()}, ensure_ascii=False) + "\n")
        os.write(self.fd, keys)

    def paste(self, text, label, submit=False):
        self.keys(b"\x1b[200~" + text.encode() + b"\x1b[201~", label)
        if submit:
            self.keys(b"\r", label + "-submit")

    def text(self):
        return "\n".join(self.screen.lines())

    def capture(self, label):
        (self.case.out / "frames" / (self.label + "-" + label + ".txt")).write_text(self.text() + "\n")

    def wait(self, label, condition, timeout=600):
        start = time.monotonic()
        while time.monotonic() - start < timeout:
            self.pump()
            result = condition()
            if result:
                self.capture(label)
                self.case.check(label, wait_s=round(time.monotonic() - start, 3))
                return result
        self.capture("FAILED-" + label)
        raise AssertionError(f"{label}: not observed within {timeout}s")

    def expect(self, label, needle, keys=None):
        if keys:
            self.keys(keys, label)
        return self.wait(label, lambda: needle in self.text(), 8)

    def pushed(self, kind):
        return [row for row in self.wire if row["message"].get("push") == kind]

    def delta_text(self):
        """Return the visible model text reconstructed from token-sized deltas."""
        return "".join(row["message"].get("text", "") for row in self.pushed("delta"))

    def approval_row_visible(self, filename):
        """Whether the focused approvals table contains the requested row.

        Chat history also repeats the tool name and filename, so checking only
        for those strings can pass before the asynchronous state poll paints
        the table.  The selected table row has the renderer's `▌` marker; use
        that marker plus the operation and filename as the synchronization
        point before sending a decision key.
        """
        return any(line.startswith("│▌leader ") and "shell" in line and filename in line
                   for line in self.screen.lines())

    @staticmethod
    def _request_matches(message, method):
        if message.get("method") == method:
            return True
        params = message.get("params")
        return isinstance(params, dict) and params.get("method") == method

    def reply(self, method, after):
        """Observe a reply to a new TUI request, not an earlier cached response."""
        rows = self.wire[after:]
        ids = {row["message"]["id"] for row in rows if row["direction"] == "request"
               and self._request_matches(row["message"], method)}
        for row in reversed(rows):
            message = row["message"]
            if row["direction"] == "response" and message.get("id") in ids:
                assert not message.get("error"), f"{method} failed: {message['error']}"
                return message.get("result")
        return None

    def review_diff(self, label, filename):
        after = len(self.wire)
        self.slash("/review")
        self.expect(label + "-open", "Changes: leader")
        report = self.wait(label + "-loaded", lambda: self.reply("review", after), 30)
        assert report["complete"] is True, "workspace review is incomplete"
        paths = [change["path"] for change in report["changes"]]
        assert filename in paths, f"review omitted {filename}: {paths}"
        # The reopened list contains other newly created files before note.txt.
        # Navigate the real picker instead of assuming its first row is note.txt.
        self.wait(label + "-file-visible", lambda: any(
            line.startswith("│modified") and filename in line for line in self.screen.lines()), 30)
        after = len(self.wire)
        self.keys(b"\x1b[B" * paths.index(filename) + b"\r", label + "-select-file")
        report = self.wait(label + "-detail-loaded", lambda: self.reply("review", after), 30)
        assert report["detail"]["path"] == filename and not report["detail"]["truncated"]
        self.expect(label + "-diff", "-original input")
        assert "+更新完成" in self.text()

    def approvals_panel(self, label):
        # Tab labels also exist behind modal overlays. Wait for the panel's
        # own hint, not a partial frame containing just the word Approvals.
        self.expect(label, "Pending approvals: a=allow once", b"\x07")

    def submitted(self):
        return [row for row in self.wire if row["direction"] == "request"
                and row["message"].get("method") == "user_message"]

    def slash(self, command):
        self.keys(b"\x0e", "focus-composer")
        self.paste(command, command, submit=True)

    def close(self):
        self.keys(b"\x11", "quit")
        start = time.monotonic()
        code = None
        while time.monotonic() - start < 12:
            self.pump()
            pid, status = os.waitpid(self.pid, os.WNOHANG)
            if pid:
                code = os.waitstatus_to_exitcode(status)
                break
        assert code == 0, f"TUI quit did not exit successfully: {code}"
        self.closed = True
        self.pump(0)
        self.raw.close()
        os.close(self.fd)
        raw = (self.case.out / (self.label + ".terminal.bin")).read_bytes()
        assert b"\x1b[?1049l" in raw and b"\x1b[?2004l" in raw, "terminal modes were not restored"
        self.case.check(self.label + "-clean-exit", duration_s=round(time.monotonic() - start, 3))

    def abort(self):
        if not self.closed:
            try:
                os.killpg(self.pid, signal.SIGTERM)
                os.waitpid(self.pid, 0)
            except ProcessLookupError:
                pass
            self.raw.close()
            os.close(self.fd)
            self.closed = True


class Case:
    def __init__(self, config, output):
        self.out = output.resolve()
        assert not self.out.exists(), "output must be a new directory"
        catalog = tomllib.loads(config.read_text())
        assert set(catalog) == {"models"} and set(catalog["models"]) == {"leader_main"}, "use an isolated one-profile config"
        profile = catalog["models"]["leader_main"]
        assert profile["model"] == "deepseek-flash" and profile["context_window"] == 1_000_000
        assert profile["generation_options"]["reasoning_effort"] == "high"
        assert profile["timeout"] == 120 and profile["max_retries"] == 5
        assert profile["api_key_env"] == "DEEPSEEK_API_KEY" and os.environ.get(profile["api_key_env"])
        assert set(profile) == {"provider", "protocol", "model", "api_key_env", "timeout", "max_retries",
                                "context_window", "generation_options"}, "unexpected or inline credential fields"
        self.out.mkdir(parents=True)
        for name in ["bin", "frames", "config/teamagents", "project"]:
            (self.out / name).mkdir(parents=True)
        self.project = self.out / "project"
        self.engine = self.out / "bin/teamagents"
        self.tui = self.out / "bin/teamagents-tui"
        shutil.copy2(ENGINE, self.engine)
        shutil.copy2(BIN, self.tui)
        shutil.copy2(config, self.out / "config/teamagents/config.toml")
        shutil.copy2(__file__, self.out / "driver.py")
        shutil.copy2(Path(__file__).with_name("pty_click_check.py"), self.out / "pty_click_check.py")
        wrapper = self.out / "engine-proxy"
        wrapper.write_text("#!/usr/bin/env python3\nimport os,sys\nos.execv(sys.executable," + repr(
            [sys.executable, str(self.out / "driver.py"), "--proxy", str(self.engine)]) + ")\n")
        wrapper.chmod(0o700)
        self.seed = "seed-" + secrets.token_hex(12) + "\n"
        (self.project / "seed.txt").write_text(self.seed)
        (self.project / "note.txt").write_text("original input\n")
        # Keep the live workspace inside its own repository.  The evidence
        # directory lives under this repository's ignored review/tmp tree;
        # without a nested Git root, review would correctly ask the parent
        # repository for tracked/unignored paths and see an empty scope.
        subprocess.run(["git", "init", "--quiet", str(self.project)], check=True)
        subprocess.run(["git", "-C", str(self.project), "add", "seed.txt", "note.txt"], check=True)
        subprocess.run([
            "git", "-C", str(self.project),
            "-c", "user.name=TeamAgents live acceptance",
            "-c", "user.email=teamagents-live@example.invalid",
            "commit", "--quiet", "-m", "initial acceptance inputs",
        ], check=True)
        save(self.out / "team.json", {"leader_id": "leader", "agents": [{"id": "leader", "name": "验收成员",
             "role": "leader", "runtime_kind": "deepagents", "model_profile": "leader_main", "tool_bindings": ["files", "shell"],
             "instructions": "独立执行用户的文件任务；遵守用户对具体工具参数、等待、拒绝和取消的要求。"}],
             "limits": {"turn_active_timeout_s": 600}})
        self.report = {"started_at": now(), "status": "running", "checks": [], "model": profile["model"],
                       "native_context_window": 1_000_000, "context_source": "用户确认；docs/DECISIONS.md D-36",
                       "reasoning_effort": "high", "profile_sha256": hashlib.sha256(config.read_bytes()).hexdigest(),
                       "binary_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [self.engine, self.tui]},
                       "seed_sha256": hashlib.sha256(self.seed.encode()).hexdigest()}
        save(self.out / "result.json", self.report)

    def check(self, label, **details):
        self.report["checks"].append({"at": now(), "check": label, "ok": True, **details})
        save(self.out / "result.json", self.report)
        print(json.dumps({"check": label, **details}, ensure_ascii=False), flush=True)

    def session_dir(self):
        paths = list((self.out / "state/teamagents/sessions").glob("*/team.db"))
        assert len(paths) == 1, "expected one isolated session"
        return paths[0].parent

    def rows(self, table):
        with sqlite3.connect((self.session_dir() / "team.db").as_uri() + "?mode=ro", uri=True) as con:
            con.row_factory = sqlite3.Row
            return [dict(row) for row in con.execute("SELECT * FROM " + table)]

    def pending(self):
        return [row for row in self.rows("approvals") if row["status"] == "PENDING"]

    def active(self):
        return [row for row in self.rows("turn_runs") if row["status"] in ["RUNNING", "QUEUED", "WAITING_APPROVAL", "WAITING_TASK"]]

    def complete(self):
        return self.rows("sessions")[0]["goal_state"] == "done" and not self.active()

    def file_is(self, name, expected):
        path = self.project / name
        return path.is_file() and not path.is_symlink() and path.read_text() == expected

    def save_state(self, label):
        save(self.out / (label + "-state.json"), {table: self.rows(table) for table in
             ["sessions", "turn_runs", "tasks", "approvals", "events", "agent_runtime"]})

    def run(self):
        terminal = None
        try:
            terminal = Terminal(self, "initial")
            terminal.expect("boot", "TeamAgents")
            prompt = ("这是实际文件验收，请独立完成。先用 read_file 读取 seed.txt，记住里面的唯一一行。\n"
                      "为验证流式界面，先向用户输出 LIVE_ROW_01 到 LIVE_ROW_40 共40行，每行写一句约20个中文字的检查说明，逐行编号。\n"
                      "随后执行一次 Shell：printf 'started\\n' > running.txt; sleep 25; printf 'ready\\n' >> running.txt，timeout=60，network=false。\n"
                      "等命令结束后，按照我执行期间补充的要求修改 note.txt；如果还没有补充，先等待，不要自行猜测。保留其他文件。")
            terminal.paste(prompt, "initial-multiline-draft")
            terminal.expect("chinese-draft-visible", "保留其他文件")
            assert not terminal.submitted(), "bracketed paste submitted without Enter"
            self.check("paste-remains-draft")
            terminal.keys(b"\r", "submit-initial")
            terminal.wait("stream-started", lambda: "LIVE_ROW_01" in terminal.delta_text())
            before_deltas = len(terminal.pushed("delta"))
            terminal.expect("tasks-during-stream", "▍Tasks", b"\x14")
            draft = "中文草稿\n尚未发送"
            terminal.paste(draft, "draft-during-stream")
            terminal.expect("input-during-stream", "尚未发送")
            terminal.wait("stream-continues-after-input", lambda: len(terminal.pushed("delta")) > before_deltas, 30)
            assert len(terminal.submitted()) == 1
            terminal.keys(b"\x7f" * len(draft), "clear-unsent-draft")
            terminal.wait("shell-active", lambda: self.file_is("running.txt", "started\n") and self.active())
            assert self.file_is("note.txt", "original input\n")
            terminal.approvals_panel("approvals-while-running")
            terminal.keys(b"\x0e", "return-to-input")
            supplement = ("补充要求：note.txt 必须恰好是下面两行并带末尾换行：\n更新完成\n编码检查：中文与多行\n"
                          "记住最初 seed.txt 的值，但不要把它写到其他文件。完成后汇报 PTY_EDIT_DONE。")
            terminal.paste(supplement, "supplement-draft")
            terminal.resize(58, 24)
            terminal.expect("narrow-multiline-draft", "PTY_EDIT_DONE")
            assert self.file_is("running.txt", "started\n"), "long tool finished before supplement test"
            terminal.resize(110, 36)
            terminal.expect("wide-draft-restored", "PTY_EDIT_DONE")
            self.save_state("before-supplement")
            terminal.keys(b"\r", "submit-while-running")
            terminal.wait("supplement-received", lambda: len(terminal.submitted()) == 2, 8)
            terminal.wait("edit-completed", lambda: self.file_is("note.txt", "更新完成\n编码检查：中文与多行\n") and self.complete())
            self.save_state("edit-completed")
            terminal.review_diff("review", "note.txt")
            terminal.approvals_panel("leave-review-for-approval")
            terminal.slash("/history")
            terminal.expect("history-open", "Member history")
            terminal.wait("history-loaded", lambda: "leader · IDLE" in terminal.text(), 30)
            terminal.expect("history-sources", "Conversation tree", b"\r")
            terminal.expect("history-tree", "leader / tree", b"\r")
            terminal.approvals_panel("history-to-approval")
            for name, decision, content in [("approval.txt", b"a", "approved-once\n"), ("denied.txt", b"d", "denied\n")]:
                terminal.keys(b"\x0e", "approval-test-compose")
                command = f"printf '{content.rstrip()}\\n' >> {name}"
                request = (f"验收批准流程。仅调用 Shell 一次，command 精确为 `{command}`，network=true，timeout=20。"
                           "这是工具参数测试，只写本地文件，不发送网络请求。不要改用其他工具代写。"
                           "等待批准后继续；如被拒绝，不要重试或绕过，只报告拒绝并结束。最后请明确完成本轮目标。")
                terminal.paste(request, name + "-request", submit=True)
                pending = terminal.wait(name + "-pending", self.pending)
                assert len(pending) == 1 and not (self.project / name).exists()
                approval_id = pending[0]["approval_id"]
                scope = json.loads(pending[0]["requested_scope"])
                assert scope["tool"] == "shell" and scope["args"]["network"] is True
                assert scope["args"]["command"] == command
                self.save_state(name + "-before-decision")
                terminal.approvals_panel(name + "-approval-panel")
                # Ctrl+G changes the focus immediately, but the panel is fed by
                # the next asynchronous state poll.  The old approval row can
                # still be painted for a moment (and an empty panel is also a
                # valid intermediate frame).  Do not send the decision until
                # the exact current row is visible; otherwise `d` is consumed
                # with no selected row and the real-model run waits forever.
                terminal.wait(
                    name + "-approval-row",
                    lambda: self.pending() and terminal.approval_row_visible(name),
                    30,
                )
                after = len(terminal.wire)
                terminal.keys(decision, name + "-decision")
                receipt = terminal.wait(name + "-decision-accepted", lambda: terminal.reply("submit", after), 30)
                expected = "once" if decision == b"a" else "deny"
                assert receipt["action_id"] == f"ui-approval-{approval_id}-{expected}"
                assert receipt["ok"] is True and receipt["result"]["approval_id"] == approval_id
                assert receipt["result"]["status"] == ("APPROVED_ONCE" if decision == b"a" else "DENIED")
                terminal.wait(name + "-completed", lambda: self.complete() and not self.pending(), 60)
                record = next(row for row in self.rows("approvals") if row["approval_id"] == approval_id)
                if decision == b"a":
                    assert self.file_is(name, content) and record["status"] == "EXPIRED"
                else:
                    assert not (self.project / name).exists() and record["status"] == "DENIED"
                self.check(name + "-effect", approval_status=record["status"], operation_hash=record["operation_hash"])
                self.save_state(name + "-after-decision")
            terminal.keys(b"\x0e", "cancel-test-compose")
            terminal.paste("现在验证停止：只执行一次 Shell，command 为 `printf 'begin\\n' >> interruption.txt; sleep 60; printf 'after\\n' >> interruption.txt`，"
                           "timeout=90，network=false。等待命令返回。", "cancel-request", submit=True)
            terminal.wait("cancel-tool-active", lambda: self.file_is("interruption.txt", "begin\n") and self.active())
            self.save_state("before-cancel")
            active_ids = {row["run_id"] for row in self.active()}
            terminal.keys(b"\x1b", "cancel-with-escape")
            terminal.wait("cancel-terminal", lambda: not self.active(), 15)
            assert self.file_is("interruption.txt", "begin\n")
            cancelled = [row for row in self.rows("turn_runs") if row["run_id"] in active_ids]
            assert cancelled and all(row["status"] == "CANCELLED" for row in cancelled)
            self.save_state("after-cancel")
            terminal.paste("取消的回合已经结束，不要重跑刚才的命令。仅用 write_file 写 resumed.txt，内容恰好为 恢复完成 加末尾换行，随后完成本轮目标。",
                           "continue-after-cancel", submit=True)
            terminal.wait("continues-after-cancel", lambda: self.file_is("resumed.txt", "恢复完成\n") and self.complete())
            assert self.file_is("interruption.txt", "begin\n")
            terminal.close()
            self.save_state("before-reopen")
            expected_files = ["approval.txt", "interruption.txt", "note.txt", "resumed.txt", "running.txt", "seed.txt"]
            files = checked_project_files(self.project, expected_files)
            assert sum(p.read_bytes().count(self.seed.strip().encode()) for p in files) == 1
            (self.project / "seed.txt").unlink()
            expected_files.remove("seed.txt")
            self.check("recall-source-removed", remaining_project_files=[
                p.name for p in checked_project_files(self.project, expected_files)])
            terminal = Terminal(self, "reopened", resumed=True)
            terminal.expect("reopen", "TeamAgents")
            terminal.review_diff("reopened-review", "note.txt")
            terminal.approvals_panel("reopened-exit-review")
            terminal.keys(b"\x0e", "recall-compose")
            terminal.paste("会话已重新打开，seed.txt 已被验收移除。不要读取工作区、不要重跑之前的命令。"
                           "从本会话最初 read_file seed.txt 的工具结果回忆唯一一行，仅用 write_file 写 recall.txt，"
                           "内容必须恰好是那一行（含末尾换行），最后汇报 PTY_RECALL_DONE 并完成目标。", "recall-request", submit=True)
            terminal.wait("reopened-history-recalled", lambda: self.file_is("recall.txt", self.seed) and self.complete())
            assert not any(row["message"].get("tool") in ["read_file", "shell", "read_history"] for row in terminal.pushed("tool"))
            assert self.file_is("approval.txt", "approved-once\n") and self.file_is("interruption.txt", "begin\n")
            assert not (self.project / "denied.txt").exists()
            terminal.slash("/history")
            terminal.expect("reopened-history-browser", "Member history")
            terminal.wait("reopened-history-loaded", lambda: "leader · IDLE" in terminal.text(), 30)
            terminal.expect("reopened-history-sources", "Conversation tree", b"\r")
            terminal.close()
            self.save_state("final")
            files = checked_project_files(self.project, expected_files + ["recall.txt"])
            usage = json.loads((self.session_dir() / "members/leader/usage.json").read_text())
            self.report.update(status="passed", finished_at=now(), session_id=self.session_dir().name, usage=usage)
            self.report["files"] = {p.name: {"bytes": p.stat().st_size, "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
                                    for p in files}
        except Exception as error:
            self.report.update(status="failed", finished_at=now(), error=str(error), traceback=traceback.format_exc())
            if terminal:
                terminal.capture("failure")
                terminal.abort()
            try:
                self.save_state("failure")
            except Exception:
                pass
        save(self.out / "result.json", self.report)
        print(json.dumps({"status": self.report["status"], "checks": len(self.report["checks"]),
                          "error": self.report.get("error"), "output": str(self.out)}, ensure_ascii=False), flush=True)
        return 0 if self.report["status"] == "passed" else 1


def self_test():
    screen = Screen(20, 4)
    screen.feed("\x1b[2J\x1b[2;3H中")
    screen.feed("文e\u0301\x1b[3;")
    screen.feed("1Hnext\x1b]0;title\x07")
    assert screen.lines()[1] == "  中文e\u0301"
    assert screen.cells[1][3] == "" and screen.cells[1][5] == ""
    assert screen.lines()[2] == "next"
    screen.feed("\x1b[2;3H\x1b[K")
    assert screen.lines()[1] == ""
    terminal = Terminal.__new__(Terminal)
    terminal.screen = Screen(110, 6)
    terminal.screen.feed("▌ shell denied.txt\r\n│Nothing waiting for approval\r\n")
    assert not terminal.approval_row_visible("denied.txt"), "stream text cannot stand in for a selected row"
    terminal.screen.feed("│▌leader  shell  approval.txt\r\n")
    assert not terminal.approval_row_visible("denied.txt"), "old approvals must not satisfy the new row"
    terminal.screen.feed("│▌leader  shell  denied.txt\r\n")
    assert terminal.approval_row_visible("denied.txt")
    terminal.wire = [
        {"direction": "request", "message": {"id": 1, "method": "submit"}},
        {"direction": "response", "message": {"id": 1, "result": {"old": True}}},
    ]
    assert terminal.reply("submit", 2) is None
    terminal.wire.extend([
        {"direction": "request", "message": {"id": 2, "method": "submit"}},
        {"direction": "response", "message": {"id": 1, "result": {"late": True}}},
    ])
    assert terminal.reply("submit", 2) is None
    terminal.wire.append({"direction": "response", "message": {"id": 2, "result": {"ok": True}}})
    assert terminal.reply("submit", 2) == {"ok": True}
    terminal.wire = [{"direction": "response", "message": {"push": "delta", "text": text}}
                     for text in ["LIVE_", "ROW_", "01"]]
    assert terminal.delta_text() == "LIVE_ROW_01"
    with tempfile.TemporaryDirectory(prefix="teamagents-live-probe-") as directory:
        project = Path(directory)
        (project / ".git").mkdir()
        (project / "note.txt").write_text("input\n")
        assert [p.name for p in checked_project_files(project, ["note.txt"])] == ["note.txt"]
        (project / ".unexpected").write_text("do not silently exclude hidden files")
        try:
            checked_project_files(project, ["note.txt"])
        except AssertionError:
            pass
        else:
            raise AssertionError("unexpected hidden file was ignored")
    print("验收探针自检通过：终端解析、批准选中行、请求回执、增量拼接与 Git 工作区清单。")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--proxy":
        sys.exit(proxy(sys.argv[2]))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--config", type=Path)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if args.self_test:
        self_test()
    else:
        if not args.config or not args.out:
            parser.error("--config and a new --out are required for real model calls")
        sys.exit(Case(args.config, args.out).run())
