"""View widgets: chat log, team/task/shared/log panels, status bar.

Widgets never hold authoritative state: they render what the session runtime
already committed and react to UI-only messages.
"""

from __future__ import annotations

import json
import time

from rich.text import Text
from rich.markdown import Markdown as RichMarkdown
from textual.containers import Horizontal, Vertical
from textual.message import Message
from textual.widgets import DataTable, Input, Markdown, RichLog, Static, TextArea

from ..models import ActionKind, TaskStatus, TeamAction, TURN_TERMINAL_STATUSES
from ..storage import Store
from .theme import BODY_STYLE, ERROR_STYLE, LABEL_STYLE, NOTICE_STYLE, SUCCESS_STYLE


class PanelReady(Message):
    """A lazily-mounted panel is ready for its first refresh."""


class ChatLog(Vertical):
    """Scrolling conversation: user input, Leader replies, system notices."""

    #: label -> (label style, body style)
    STYLES = {
        "你": (LABEL_STYLE, BODY_STYLE),
        "Leader": (LABEL_STYLE, BODY_STYLE),
        "system": (LABEL_STYLE, NOTICE_STYLE),
    }

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self._entries: list[tuple[str, str]] = []
        self._width = 0

    def compose(self):
        yield RichLog(highlight=False, markup=False, wrap=True, min_width=1, id="chat-stream")
        yield Static("", id="chat-live", markup=False)

    def show_stream(self, text: str) -> None:
        live = self.query_one("#chat-live", Static)
        live.display = bool(text)
        live.update(RichMarkdown(text) if text else "")

    def clear(self) -> None:
        self._entries.clear()
        self.query_one("#chat-stream", RichLog).clear()
        self.show_stream("")

    def on_resize(self, event) -> None:
        if event.size.width != self._width:
            self._width = event.size.width
            self.call_after_refresh(self._reflow)

    def _reflow(self) -> None:
        log = self.query_one("#chat-stream", RichLog)
        log.clear()
        for who, text in self._entries:
            self._render_entry(who, text)
        log.scroll_end(animate=False)

    def write_line(self, who: str, text: str) -> None:
        self._entries.append((who, text))
        self._render_entry(who, text)

    def _render_entry(self, who: str, text: str) -> None:
        log = self.query_one("#chat-stream", RichLog)
        label_style, body_style = self.STYLES.get(
            who, (LABEL_STYLE, NOTICE_STYLE if who.startswith("system") else BODY_STYLE))
        if who.startswith("✗") or who.startswith("⚠"):
            label_style, body_style = ERROR_STYLE, ERROR_STYLE
        elif who.endswith("（完成）"):
            label_style, body_style = SUCCESS_STYLE, BODY_STYLE
        if who:
            prefix = "›" if who == "你" else "•"
            log.write(Text(f"{prefix} {who}", style=label_style))
        if who == "Leader":
            log.write(RichMarkdown(text or ""))
            log.write("")
            return
        for line in (text or "").splitlines() or [""]:
            if line.startswith(("✗", "⚠")):
                log.write(Text(line, style=ERROR_STYLE))
            else:
                log.write(Text(line, style=body_style))
        log.write("")


class PromptInput(TextArea):
    """Multi-line input: Enter sends, Shift+Enter / Ctrl+J insert a newline."""

    BINDINGS = [
        ("ctrl+j,shift+enter", "newline", "换行"),
    ]

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.prompt_history: list[str] = []
        self.prompt_history_index = 0
        self.draft = ""

    def record_submission(self, text: str) -> None:
        # Same recall contract as Codex chat_composer_history: skip adjacent
        # duplicates and restore the unsent draft after navigating down.
        if text and (not self.prompt_history or self.prompt_history[-1] != text):
            self.prompt_history.append(text)
        self.prompt_history_index = len(self.prompt_history)

    def reset_history(self) -> None:
        self.prompt_history.clear()
        self.prompt_history_index = 0
        self.draft = ""
        self.text = ""

    def on_text_area_changed(self, event: TextArea.Changed) -> None:
        self.styles.height = min(8, max(3, self.wrapped_document.height + 2))

    def recall(self, direction: int) -> None:
        if not self.prompt_history:
            return
        if self.prompt_history_index == len(self.prompt_history):
            self.draft = self.text
        self.prompt_history_index = min(len(self.prompt_history), max(0, self.prompt_history_index + direction))
        self.load_text(self.draft if self.prompt_history_index == len(self.prompt_history)
                       else self.prompt_history[self.prompt_history_index])
        self.move_cursor(self.document.end)

    class Submitted(Message):
        def __init__(self, text: str) -> None:
            super().__init__()
            self.text = text

    def action_submit_prompt(self) -> None:
        text = self.text.strip()
        if not text:
            return
        self.text = ""
        self.post_message(self.Submitted(text))

    def action_newline(self) -> None:
        self.insert("\n")

    def on_key(self, event) -> None:
        """TextArea owns Enter; intercept it here (Shift+Enter keeps the newline)."""
        if event.key == "enter":
            self.action_submit_prompt()
            event.stop()
            event.prevent_default()
        elif (event.key == "up" and self.cursor_location[0] == 0
              or event.key == "down" and self.cursor_location[0] == self.document.line_count - 1):
            self.recall(-1 if event.key == "up" else 1)
            event.stop()
            event.prevent_default()


class StatusBar(Static):
    """One line: session, mode, session state, Leader, live turn count."""

    def refresh_status(self, runtime, extra: str = "") -> None:
        store = runtime.store
        session = store.get_session(runtime.session_id)
        spec = store.load_team_spec(runtime.session_id)
        running = [r for r in store.runs_for_session(
            runtime.session_id, ["RUNNING", "QUEUED"])]
        tasks = store.tasks_for_session(
            runtime.session_id, [TaskStatus.PENDING, TaskStatus.RUNNING, TaskStatus.BLOCKED])
        approvals = store.pending_approvals(runtime.session_id)
        mode = session["permissions_mode"] if session else "?"
        state = session["status"] if session else "?"
        leader = spec.leader
        self.update(
            f" 会话 {runtime.session_id}  |  {state}  |  权限 {mode}  |  "
            f"Leader {leader.name}({leader.model_profile})  |  活动回合 {len(running)}  |  "
            f"未完成任务 {len(tasks)}  |  待批准 {len(approvals)}"
            + (f"  |  {extra}" if extra else ""))


class TeamPanel(Vertical):
    """Members, roles, models, status, workspace and reach (section 13)."""

    def compose(self):
        yield DataTable(id="team-table", zebra_stripes=True)

    def on_mount(self) -> None:
        table = self.query_one("#team-table", DataTable)
        table.add_columns("成员", "角色", "类型", "模型", "状态", "工作目录", "可见范围")
        self.post_message(PanelReady())

    def refresh_from(self, store: Store, session_id: str, runtime=None) -> None:
        table = self.query_one("#team-table", DataTable)
        table.clear()
        spec = store.load_team_spec(session_id)
        for agent in spec.agents:
            status = store.agent_status(session_id, agent.id)
            can_send = sorted({a.id for a in spec.agents if spec.can_send(agent.id, a.id)})
            can_delegate = sorted({a.id for a in spec.agents
                                   if spec.can_delegate(agent.id, a.id)})
            observers = [o.agent_id for o in spec.observers if agent.id in o.subjects]
            reach = []
            if can_send:
                reach.append("消息→" + ",".join(can_send))
            if can_delegate:
                reach.append("任务→" + ",".join(can_delegate))
            if observers:
                reach.append("被观察:" + ",".join(observers))
            table.add_row(agent.id, agent.role, agent.runtime_kind, agent.model_profile,
                          status.value, agent.workspace_policy.value,
                          " ".join(reach) or "-", key=agent.id)


class TasksPanel(Vertical):
    """Delegation tree with dependencies, status, results (section 13).

    Keys: c = cancel the selected task.  A BLOCKED task (interrupted member
    turn) has no other exit, and a RUNNING task with an active turn is only
    asked to cancel; there is no separate "pause task" state in the plan.
    """

    can_focus = True

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.runtime = None

    def compose(self):
        yield Static("c=取消选中任务（BLOCKED 直接取消；执行中的回合收到取消请求）",
                     id="tasks-hint")
        yield DataTable(id="tasks-table", zebra_stripes=True)

    def on_mount(self) -> None:
        table = self.query_one("#tasks-table", DataTable)
        table.add_columns("任务", "委派者", "承接者", "状态", "描述", "依赖", "结果")
        self.post_message(PanelReady())

    def refresh_from(self, store: Store, session_id: str, runtime=None) -> None:
        if runtime is not None:
            self.runtime = runtime
        table = self.query_one("#tasks-table", DataTable)
        table.clear()
        tasks = store.tasks_for_session(session_id)
        by_id = {t.task_id: t for t in tasks}
        depth = {}

        def level(task) -> int:
            seen = set()
            current = task
            depth_value = 0
            while current is not None and current.parent_task_id and \
                    current.parent_task_id not in seen:
                seen.add(current.task_id)
                current = by_id.get(current.parent_task_id)
                depth_value += 1
            return depth_value

        for task in tasks:
            indent = "  " * level(task)
            table.add_row(f"{indent}{task.task_id[-8:]}", task.requester, task.assignee,
                          task.status.value, task.description[:60],
                          ",".join(d[-8:] for d in task.dependencies) or "-",
                          ",".join(task.result_refs)[:60] or "-", key=task.task_id)

    def selected_task(self) -> str | None:
        table = self.query_one("#tasks-table", DataTable)
        if table.cursor_row is None or not table.row_count:
            return None
        try:
            return str(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value)
        except Exception:
            return None

    def on_key(self, event) -> None:
        if event.key == "c":
            self._cancel_selected(event)

    def _cancel_selected(self, event) -> None:
        task_id = self.selected_task()
        if task_id is None or self.runtime is None:
            return
        _ok, message = self.cancel(self.runtime, task_id)
        try:
            self.app.notify(message)
        except Exception:
            pass
        try:
            self.app._write_chat("system", message)
        except Exception:
            pass
        event.stop()

    @staticmethod
    def cancel(runtime, task_id: str) -> tuple[bool, str]:
        """Submit the user-side CANCEL_TASK path; control decides the outcome."""
        receipt = runtime.submit(TeamAction(
            action_id=f"ui-cancel-task-{task_id}",
            session_id=runtime.session_id, actor_id="user",
            kind=ActionKind.CANCEL_TASK, payload={"task_id": task_id}))
        short = task_id[-8:]
        if not receipt.ok:
            return False, f"取消任务失败：{receipt.error}"
        status = str((receipt.result or {}).get("status", ""))
        if status == TaskStatus.CANCELLED:
            return True, f"任务 {short} 已取消"
        if status == "CANCEL_REQUESTED":
            return True, f"任务 {short} 已请求取消（活动回合结束后生效）"
        return False, f"任务 {short} 已处于终态（{status}），无需取消"


class SharedPanel(Vertical):
    """Shared space entries with author and position (section 13)."""

    def compose(self):
        yield DataTable(id="shared-table", zebra_stripes=True)

    def on_mount(self) -> None:
        table = self.query_one("#shared-table", DataTable)
        table.add_columns("空间", "作者", "类型", "内容/引用", "序号")
        self.post_message(PanelReady())

    def refresh_from(self, store: Store, session_id: str, runtime=None) -> None:
        table = self.query_one("#shared-table", DataTable)
        table.clear()
        spec = store.load_team_spec(session_id)
        for space in spec.shared_spaces:
            for entry in store.shared_entries(session_id, [space.id], limit=200):
                table.add_row(entry.space_id, entry.author, entry.kind,
                              (entry.content[:80] or entry.ref or "-"),
                              str(entry.sequence),
                              key=f"{entry.space_id}:{entry.sequence}")


class LogPanel(Vertical):
    """Session log; filtered by member when a member is selected."""

    def compose(self):
        yield Static("全部事件", id="log-title")
        yield RichLog(id="log-stream", markup=False, wrap=True)

    def refresh_from(self, store: Store, session_id: str, member: str | None,
                     cursor: int) -> int:
        title = self.query_one("#log-title", Static)
        title.update(f"事件流{f'（成员 {member}）' if member else ''}")
        log = self.query_one("#log-stream", RichLog)
        high_water = cursor
        for event in store.events(session_id, after_sequence=cursor, limit=500):
            high_water = max(high_water, event["sequence"])
            if member and event["actor_id"] != member and member not in \
                    (event["payload_json"] or ""):
                continue
            payload = event["payload_json"]
            if len(payload) > 160:
                payload = payload[:160] + "…"
            log.write(f"{event['sequence']:>5} {event['kind']:<18} "
                      f"{event['actor_id']:<10} {payload}")
        return high_water

    def replay_from(self, store: Store, session_id: str, member: str | None,
                    cursor: int = 0) -> int:
        """Rebuild the stream in place (tab activation / member filter change)."""
        self.query_one("#log-stream", RichLog).clear()
        return self.refresh_from(store, session_id, member, cursor)


class InputRow(Vertical):
    """Prompt line plus a hint."""

    def compose(self):
        yield Static("就绪 · 向 Leader 输入任务或补充要求", id="composer-status", markup=False)
        with Horizontal(id="composer"):
            yield Static("›", id="prompt-prefix")
            yield PromptInput(id="prompt", language=None)
        yield Static("Enter 发送 · Shift+Enter / Ctrl+J 换行 · ↑↓ 历史 · Esc 停止 Leader",
                     id="composer-hint", markup=False)

    def focus_prompt(self) -> None:
        self.query_one("#prompt", PromptInput).focus()


class SettingsPanel(Vertical):
    """Session and settings view: state, permission mode, limits, catalog.

    Editing model profiles / tools / skills stays in the user config file
    (secrets and service definitions never belong in a UI session state);
    this panel shows them plus the exact config path, and toggles the two
    user-controlled session switches.
    """

    can_focus = True

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.runtime = None

    def compose(self):
        yield Static("", id="settings-body")

    def on_mount(self) -> None:
        self.post_message(PanelReady())

    def refresh_from(self, store: Store, session_id: str, runtime=None) -> None:
        if runtime is not None:
            self.runtime = runtime
        session = store.get_session(session_id)
        spec = store.load_team_spec(session_id)
        catalog = runtime.catalog if runtime is not None else None
        limits = spec.limits
        lines = [
            f"会话：{session_id}",
            f"状态：{session['status']}    权限模式：{session['permissions_mode']}"
            f"    （Ctrl+F 切换）",
            f"工作目录：{session['cwd']}",
            f"团队：{len(spec.agents)} 名成员，拓扑修订 {store.current_revision(session_id)}",
            f"上限：并发 {limits.max_parallel_workers}、"
            f"成员 {limits.max_members}、单目标回合 {limits.max_turns_per_goal}、"
            f"单回合步骤 {limits.max_model_steps_per_turn}、"
            f"回合超时 {limits.turn_active_timeout_s}s",
            f"用户配置：{_config_path()}",
        ]
        if catalog is not None:
            lines.append("模型 profiles：" + ("、".join(
                f"{name}({profile.provider}/{profile.model})"
                for name, profile in catalog.models.items()) or "无"))
            lines.append("工具绑定：" + ("、".join(
                f"{name}[{binding.kind}]" for name, binding in catalog.tools.items())
                or "无（files/shell/web 为内置）"))
            lines.append("Skills 目录：" + ("、".join(catalog.skills_paths) or "未配置"))
            lines.append("指令文件：" + ("、".join(catalog.instruction_files) or "未配置"))
        lines.append("")
        lines.append("恢复：teamagents --resume " + session_id +
                     "    新建：换 --cwd 或删掉会话目录")
        self.query_one("#settings-body", Static).update("\n".join(lines))


def _config_path() -> str:
    from ..config import user_config_path
    return str(user_config_path())


class SessionsPanel(Vertical):
    """Sessions for this working directory: switch / new / archive / delete.

    Keys: s or enter = switch, n = new session in this directory,
    a = archive, d = delete (press d twice to confirm).
    """

    can_focus = True

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.pending_delete: str | None = None
        # row key -> real session id; keys are decorated only on id collisions
        self._row_sessions: dict[str, str] = {}

    def compose(self):
        yield Static(
            "本目录会话：s=切换  n=新建  a=归档  d=删除（再按 d 确认，删当前会话后退出）",
            id="sessions-hint")
        yield DataTable(id="sessions-table", zebra_stripes=True)

    def on_mount(self) -> None:
        table = self.query_one("#sessions-table", DataTable)
        table.add_columns("会话", "状态", "目标", "事件", "大小", "更新", "标记")
        self.post_message(PanelReady())

    def refresh_from(self, store, session_id: str, runtime=None) -> None:
        from ..sessions import list_sessions

        table = self.query_one("#sessions-table", DataTable)
        table.clear()
        cwd = None
        if runtime is not None:
            session = runtime.store.get_session(runtime.session_id)
            cwd = session["cwd"] if session else None
        rows = list_sessions(cwd=cwd or None)
        self._row_sessions.clear()
        used: set[str] = set()
        for info in rows:
            marks = []
            if info.session_id == session_id:
                marks.append("当前")
            if info.running and info.session_id != session_id:
                marks.append("运行中")
            if info.archived:
                marks.append("已归档")
            if info.error:
                marks.append("读取异常")
            updated = time.strftime("%m-%d %H:%M", time.localtime(info.updated_at)) \
                if info.updated_at else "-"
            # the same id may exist as an active and an archived record (B-07);
            # decorate only the colliding row so unique ids keep their plain key
            key = info.session_id
            if key in used:
                key = f"{info.session_id}#{'archived' if info.archived else 'active'}"
            used.add(key)
            self._row_sessions[key] = info.session_id
            table.add_row(info.session_id, info.status, info.goal_state,
                          str(info.events), f"{info.size_mb:.1f}MB", updated,
                          " ".join(marks) or "-", key=key)

    def selected_session(self) -> str | None:
        table = self.query_one("#sessions-table", DataTable)
        if not table.row_count:
            return None
        try:
            key = str(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value)
        except Exception:
            return None
        return self._row_sessions.get(key, key)

    def on_key(self, event) -> None:
        app = self.app
        target = self.selected_session()
        if event.key in ("enter", "s"):
            if target:
                app.run_worker(app.switch_session(target), name="ta-switch")
            event.stop()
        elif event.key == "n":
            app.run_worker(app.new_session(), name="ta-new-session")
            event.stop()
        elif event.key == "a":
            if target:
                app.run_worker(app.archive_current_or(target), name="ta-archive")
            event.stop()
        elif event.key == "d":
            if not target:
                event.stop()
                return
            if self.pending_delete != target:
                self.pending_delete = target
                app._write_chat("system", f"再按一次 d 确认删除会话 {target}")
            else:
                self.pending_delete = None
                app.run_worker(app.delete_session_interactive(target), name="ta-delete")
            event.stop()
        elif event.key == "escape":
            self.pending_delete = None
            event.stop()
