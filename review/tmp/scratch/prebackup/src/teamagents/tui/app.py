"""Main TUI: Leader chat, side panels, approvals, status (plan section 13).

The UI reads committed state and renders it; it never becomes the source of
truth. Model deltas arrive through a bounded, coalesced refresh.
"""

from __future__ import annotations

import asyncio
import contextlib
import os
from pathlib import Path
from typing import Any

from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.widgets import Footer, Header, Static, TabbedContent, TabPane

from ..models import EventKind, TaskStatus, TeamAction, ActionKind
from ..runtime import SessionRuntime
from .approvals import ApprovalsPanel
from .panels import (ChatLog, InputRow, LogPanel, PromptInput, SettingsPanel,
                     SessionsPanel, SharedPanel, StatusBar, TasksPanel, TeamPanel,
                     PanelReady)
from .theme import CODEX_THEME

RIGHT_PANELS = [("team", "团队"), ("tasks", "任务"), ("shared", "共享空间"),
                ("approvals", "批准"), ("sessions", "会话"), ("log", "日志"),
                ("settings", "设置")]


class TeamAgentsApp(App):
    """Terminal UI over a SessionRuntime."""

    CSS = """
    /* Codex-like: flat, monochrome, one accent, thin borders */
    Screen { layout: vertical; background: $background; color: $foreground; }
    #body { height: 1fr; }
    #chat { width: 2fr; padding: 0 1; background: $background; }
    #side { width: 3fr; border-left: solid $secondary; padding: 0 1; }
    #side.narrow { display: none; }
    #chat.narrow { width: 1fr; }
    .panel { height: 1fr; }
    #prompt {
        height: 4;
        border: round $secondary;
        background: $background;
        color: $foreground;
    }
    #prompt:focus { border: round $primary; }
    #status {
        height: 1;
        background: $panel;
        color: $text-muted;
        padding: 0 1;
    }
    #chat-stream, #log-stream { background: $background; }
    DataTable { height: 1fr; background: $background; color: $foreground; }
    DataTable > .datatable--header {
        background: $panel;
        color: $text-muted;
        text-style: bold;
    }
    DataTable > .datatable--cursor { background: $primary 40%; color: $foreground; }
    DataTable > .datatable--hover { background: $boost; }
    DataTable > .datatable--odd-row { background: $background; }
    DataTable > .datatable--even-row { background: $background; }
    Tabs { background: $background; }
    Tab { color: $text-muted; background: $background; }
    Tab.-active { color: $foreground; text-style: bold; }
    TabPane { padding: 0; }
    #settings-body, #approvals-hint, #log-title { color: $text-muted; padding: 0 0 1 0; }
    Header { background: $panel; color: $text-muted; }
    Footer { background: $panel; color: $text-muted; }
    Footer > .footer--key { background: $boost; color: $foreground; }
    """

    BINDINGS = [
        # priority: the prompt area would otherwise swallow Ctrl+A (select all)
        Binding("ctrl+q", "quit_app", "退出", priority=True),
        Binding("ctrl+p", "pause_session", "暂停/继续", priority=True),
        Binding("ctrl+r", "refresh_all", "刷新", priority=True),
        Binding("ctrl+f", "toggle_full_auto", "全自动", priority=True),
        Binding("ctrl+t", "cycle_panel", "切换面板", priority=True),
        Binding("ctrl+a", "focus_approvals", "批准", priority=True),
        Binding("ctrl+n", "focus_prompt", "输入", priority=True),
    ]

    def __init__(self, runtime: SessionRuntime | None = None,
                 runtime_factory=None, cwd: Any = None,
                 open_kwargs: dict | None = None, **kwargs: Any) -> None:
        super().__init__(**kwargs)
        self.rt = runtime
        self._runtime_factory = runtime_factory
        self._cwd = str(cwd) if cwd else None
        self._open_kwargs = dict(open_kwargs or {})
        self._cursor = 0
        self._selected_member: str | None = None
        self._delta_buffer: dict[str, list[str]] = {}
        self._last_event_seen = 0
        self._owns_runtime = runtime is None
        self._runtime_closed = False

    # ------------------------------------------------------------- layout

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Static("", id="status")
        with Horizontal(id="body"):
            with Vertical(id="chat"):
                yield ChatLog(id="chat-log")
                yield InputRow(id="input-row")
            with Vertical(id="side"):
                with TabbedContent(initial="tab-team"):
                    with TabPane("团队", id="tab-team"):
                        yield TeamPanel(classes="panel")
                    with TabPane("任务", id="tab-tasks"):
                        yield TasksPanel(classes="panel")
                    with TabPane("共享空间", id="tab-shared"):
                        yield SharedPanel(classes="panel")
                    with TabPane("批准", id="tab-approvals"):
                        yield ApprovalsPanel(classes="panel")
                    with TabPane("会话", id="tab-sessions"):
                        yield SessionsPanel(classes="panel")
                    with TabPane("日志", id="tab-log"):
                        yield LogPanel(classes="panel")
                    with TabPane("设置", id="tab-settings"):
                        yield SettingsPanel(classes="panel")
        yield Footer()

    async def on_mount(self) -> None:
        self.register_theme(CODEX_THEME)
        self.theme = CODEX_THEME.name
        if self.rt is None and self._runtime_factory is not None:
            self.rt = await self._runtime_factory()
        self.title = "TeamAgents"
        self.sub_title = self.rt.session_id if self.rt else ""
        if self.rt is None:
            self._write_chat("system", "[没有可用会话]")
            return
        await self.rt.start()
        self.rt.set_stream_sink(self._on_model_delta)
        self._startup_checks()
        self.run_worker(self._event_loop(), name="ta-events")
        self.set_interval(0.08, self._flush_deltas)
        self.set_interval(1.0, self._refresh_widgets)
        self.call_after_refresh(self._refresh_now)
        await self._refresh_widgets()
        self.query_one("#prompt", PromptInput).focus()

    def _startup_checks(self) -> None:
        """Tell the user up front what will make their first turn fail."""
        from ..config import user_config_path

        spec = self.rt.store.load_team_spec(self.rt.session_id)
        catalog = self.rt.catalog
        for agent in spec.agents:
            profile = catalog.models.get(agent.model_profile)
            if profile is None:
                self._write_chat(
                    "system",
                    f"⚠ 成员 {agent.id} 的模型 profile {agent.model_profile!r} 未配置："
                    f"请创建 {user_config_path()}（可直接复制仓库里的 "
                    f"examples/config.toml），然后重开会话。在此之前发出的消息都会失败。")
            elif profile.api_key_env and not os.environ.get(profile.api_key_env):
                self._write_chat(
                    "system",
                    f"⚠ 模型 profile {agent.model_profile!r} 需要环境变量 "
                    f"{profile.api_key_env}，当前未设置：请 export 后重开会话。")

    async def on_unmount(self) -> None:
        """Closing the UI must also close the session runtime.

        Otherwise background tasks and the checkpointer's aiosqlite thread keep
        the process alive (the `uv run` parent never returns).
        """
        if self._owns_runtime:
            await self._close_runtime()

    async def _close_runtime(self) -> None:
        """Stop the executor and release the session lock / file handles."""
        if self.rt is None or self._runtime_closed:
            return
        runtime, self.rt = self.rt, None
        self._runtime_closed = True
        with contextlib.suppress(Exception):
            await runtime.close()
        with contextlib.suppress(Exception):
            runtime.store.close()

    def _refresh_now(self) -> None:
        self.run_worker(self._refresh_widgets(), name="ta-refresh")

    def on_resize(self, event) -> None:
        """Narrow terminals switch to a single column (plan section 13)."""
        narrow = event.size.width < 100
        with contextlib.suppress(Exception):
            self.query_one("#side").set_class(narrow, "narrow")
            self.query_one("#chat").set_class(narrow, "narrow")

    # ------------------------------------------------------------- rendering

    async def _refresh_widgets(self) -> None:
        store = self.rt.store
        session_id = self.rt.session_id

        def render() -> None:
            self.query_one("#status", StatusBar).refresh_status(self.rt)
            self.query_one(TeamPanel).refresh_from(store, session_id, self.rt)
            self.query_one(TasksPanel).refresh_from(store, session_id, self.rt)
            self.query_one(SharedPanel).refresh_from(store, session_id, self.rt)
            self.query_one(ApprovalsPanel).refresh_from(store, session_id, self.rt)
            self.query_one(SettingsPanel).refresh_from(store, session_id, self.rt)
            self._cursor = self.query_one(LogPanel).refresh_from(
                store, session_id, self._selected_member, 0)
        with contextlib.suppress(Exception):
            render()

    async def _event_loop(self) -> None:
        """Render committed events incrementally; never blocks execution."""
        while True:
            try:
                if self.rt is not None:
                    await self._drain_events()
            except Exception as error:  # a UI read error must not kill the app
                self._write_chat("system", f"[界面读取事件失败] {error}")
            await asyncio.sleep(0.1)

    async def _drain_events(self) -> None:
        events = self.rt.store.events(self.rt.session_id, after_sequence=self._cursor)
        for event in events:
            self._cursor = max(self._cursor, event["sequence"])
            kind = event["kind"]
            payload = _payload(event)
            if kind == EventKind.USER_MESSAGE:
                self._write_chat("你", payload.get("text", ""))
            elif kind == EventKind.LEADER_REPLY:
                self._write_chat("Leader", payload.get("text", ""))
            elif kind == EventKind.MESSAGE:
                self._write_chat(f"{event['actor_id']}→{payload.get('target')}",
                                 payload.get("text", ""))
            elif kind == EventKind.TASK_CREATED:
                self._write_chat("system",
                                 f"任务 {payload.get('task_id', '')[-8:]} → "
                                 f"{payload.get('assignee')}：{payload.get('description', '')}")
            elif kind in (EventKind.TASK_COMPLETED, EventKind.TASK_FAILED,
                          EventKind.TASK_BLOCKED, EventKind.TASK_CANCELLED):
                self._write_chat("system",
                                 f"[{kind}] {payload.get('task_id', '')[-8:]} "
                                 f"{payload.get('summary') or payload.get('reason') or payload.get('error') or ''}")
            elif kind == EventKind.APPROVAL_REQUESTED:
                self._write_chat("system",
                                 f"需要批准：{_approval_line(payload)}（按 Ctrl+A 处理）")
            elif kind == EventKind.APPROVAL_DECIDED:
                self._write_chat("system",
                                 f"批准 {payload.get('approval_id')} → {payload.get('status')}")
            elif kind == EventKind.RUN_FAILED:
                self._write_chat(
                    "system",
                    f"✗ 成员 {payload.get('agent_id')} 的回合失败：{payload.get('error')}")
            elif kind == EventKind.RUN_CANCELLED:
                self._write_chat("system",
                                 f"回合已停止：{payload.get('agent_id')} "
                                 f"({payload.get('status')})")
            elif kind == EventKind.RUN_WAITING:
                waiting = payload.get("waiting_on") or []
                if waiting:
                    self._write_chat("system",
                                     f"{payload.get('agent_id')} 正在等待 "
                                     f"{len(waiting)} 个任务完成")
            elif kind == EventKind.MEMBER_STATUS:
                self._write_chat("system",
                                 f"成员状态：{payload.get('agent_id')} "
                                 f"{payload.get('status')} {payload.get('error', '')}")
            elif kind == EventKind.SESSION_STATUS:
                self._write_chat("system", f"会话状态：{payload}")
            elif kind == EventKind.RUN_PROGRESS and payload.get("final"):
                self._write_chat(f"{payload.get('agent_id')}（完成）",
                                 payload.get("text", ""))
            elif kind == EventKind.LIMIT_REACHED:
                self._write_chat("system", f"[达到上限] {payload}")
            elif kind == EventKind.GOAL_DONE:
                self._write_chat("system", f"目标完成：{payload.get('summary', '')}")

    def _on_model_delta(self, run_id: str, agent_id: str, text: str) -> None:
        self._delta_buffer.setdefault(agent_id, []).append(text)

    def _flush_deltas(self) -> None:
        """Coalesced refresh: bounded queue, no per-character repaint."""
        if not self._delta_buffer or self.rt is None:
            return
        try:
            leader_id = self.rt.store.load_team_spec(self.rt.session_id).leader_id
        except Exception:
            self._delta_buffer.clear()
            return
        for agent_id, chunks in list(self._delta_buffer.items()):
            text = "".join(chunks)[-4000:]
            del self._delta_buffer[agent_id]
            if agent_id == leader_id:
                self._write_chat("Leader（生成中）", text[-1500:])

    def on_tabbed_content_tab_activated(self, event) -> None:
        """A pane mounts lazily: refresh it as soon as it becomes visible."""
        self._refresh_panel_for(getattr(event.pane, "id", None))

    def on_panel_ready(self, event: PanelReady) -> None:
        """A panel finished mounting: give it the current state."""
        panel = getattr(event, "control", None)
        if panel is None:
            return
        with contextlib.suppress(Exception):
            panel.refresh_from(self.rt.store, self.rt.session_id, self.rt)
        if isinstance(panel, ApprovalsPanel):
            with contextlib.suppress(Exception):
                panel.focus()

    def _refresh_panel_for(self, tab_id: str | None) -> None:
        store, session_id = self.rt.store, self.rt.session_id
        widget = {
            "tab-team": TeamPanel, "tab-tasks": TasksPanel,
            "tab-shared": SharedPanel, "tab-approvals": ApprovalsPanel,
            "tab-sessions": SessionsPanel,
            "tab-settings": SettingsPanel,
        }.get(tab_id or "")
        if widget is None:
            return
        with contextlib.suppress(Exception):
            panel = self.query_one(widget)
            panel.refresh_from(store, session_id, self.rt)
            if widget is ApprovalsPanel:
                panel.focus()

    # ------------------------------------------------------- session records

    async def _open_session(self, session_id: str | None):
        """Open a session through whatever route created this app."""
        if self._runtime_factory is not None:
            try:
                return await self._runtime_factory(session_id)
            except TypeError:                    # older factories take no argument
                return await self._runtime_factory()
        from ..session import open_session
        kwargs = dict(self._open_kwargs)
        cwd = self._cwd or (self.rt.store.get_session(self.rt.session_id)["cwd"]
                            if self.rt else None)
        return await open_session(cwd=Path(cwd) if cwd else None,
                                  session_id=session_id, **kwargs)

    async def switch_session(self, session_id: str) -> bool:
        """Close the current session and open another one (records preserved)."""
        if self.rt is not None and session_id == self.rt.session_id:
            return True
        try:
            new_rt = await self._open_session(session_id)
        except Exception as e:
            self._write_chat("system", f"✗ 无法打开会话 {session_id}：{e}")
            return False
        await self._close_runtime()
        self.rt = new_rt
        self._runtime_closed = False
        self._owns_runtime = True
        await self.rt.start()
        self.rt.set_stream_sink(self._on_model_delta)
        self._cursor = 0
        self._selected_member = None
        self._delta_buffer.clear()
        self.sub_title = self.rt.session_id
        with contextlib.suppress(Exception):
            self.query_one(ChatLog).query_one("#chat-stream").clear()
        self._write_chat("system", f"已切换到会话 {self.rt.session_id}")
        self._startup_checks()
        self._refresh_now()
        return True

    async def archive_current_or(self, session_id: str) -> bool:
        """Archive a session; the current one must be closed first."""
        from ..sessions import SessionInUse, archive_session

        was_current = self.rt is not None and session_id == self.rt.session_id
        if was_current:
            await self._close_runtime()
        try:
            target = archive_session(session_id)
        except SessionInUse as e:
            self._write_chat("system", f"✗ 归档失败：{e}")
            return False
        except Exception as e:
            self._write_chat("system", f"✗ 归档失败：{e}")
            return False
        self._write_chat("system", f"已归档会话 {session_id} → {target}")
        if was_current:
            self.exit()
            return True
        self._refresh_now()
        return True

    async def delete_session_interactive(self, session_id: str) -> bool:
        """Delete a session; a member worktree with unmerged work blocks it."""
        from ..sessions import (SessionDeleteBlocked, SessionInUse, delete_session)

        was_current = self.rt is not None and session_id == self.rt.session_id
        if was_current:
            # release our own lock before removing the directory
            await self._close_runtime()
        try:
            delete_session(session_id)
        except SessionInUse as e:
            self._write_chat("system", f"✗ 删除失败：{e}")
            return False
        except SessionDeleteBlocked as e:
            self._write_chat("system", f"✗ 删除被阻止：{e}")
            return False
        except Exception as e:
            self._write_chat("system", f"✗ 删除失败：{e}")
            return False
        self._write_chat("system", f"已删除会话 {session_id}")
        if was_current:
            self.exit()
            return True
        self._refresh_now()
        return True

    async def new_session(self) -> bool:
        from ..sessions import new_session_id

        cwd = self._cwd or (self.rt.store.get_session(self.rt.session_id)["cwd"]
                            if self.rt else None)
        if not cwd:
            self._write_chat("system", "✗ 无法确定工作目录")
            return False
        return await self.switch_session(new_session_id(Path(cwd)))

    def _write_chat(self, who: str, text: str) -> None:
        with contextlib.suppress(Exception):
            self.query_one(ChatLog).write_line(who, text)

    async def on_prompt_input_submitted(self, message: PromptInput.Submitted) -> None:
        receipt = self.rt.user_message(message.text)
        if not receipt.ok:
            self._write_chat("system", f"[输入被拒绝] {receipt.error}")

    def on_data_table_row_highlighted(self, event) -> None:
        if event.data_table.id == "team-table":
            self._selected_member = str(event.row_key.value)

    def on_data_table_row_selected(self, event) -> None:
        if event.data_table.id == "tasks-table":
            task_id = str(event.row_key.value)
            task = self.rt.store.get_task(task_id)
            if task is not None:
                self._write_chat("system",
                                 f"任务 {task.task_id} | {task.status} | "
                                 f"{task.description} | 依赖 {task.dependencies} | "
                                 f"成果 {task.result_refs}")

    def action_focus_prompt(self) -> None:
        self.query_one("#prompt", PromptInput).focus()

    def action_focus_approvals(self) -> None:
        with contextlib.suppress(Exception):
            tabs = self.query_one(TabbedContent)
            tabs.active = "tab-approvals"
        # the pane mounts lazily; TabActivated refreshes and focuses it
        self.set_timer(0.2, lambda: self._refresh_panel_for("tab-approvals"))

    def action_cycle_panel(self) -> None:
        tabs = self.query_one(TabbedContent)
        ids = [f"tab-{name}" for name, _ in RIGHT_PANELS]
        try:
            index = (ids.index(tabs.active) + 1) % len(ids)
        except ValueError:
            index = 0
        tabs.active = ids[index]

    def action_refresh_all(self) -> None:
        self._cursor = 0
        self._write_chat("system", "[已刷新视图]")

    def action_pause_session(self) -> None:
        session = self.rt.store.get_session(self.rt.session_id)
        if session["status"] == "PAUSED":
            self.rt.user_message("继续执行")
            return
        receipt = self.rt.submit(TeamAction(
            action_id=f"ui-pause-{self._cursor}", session_id=self.rt.session_id,
            actor_id="user", kind=ActionKind.PAUSE_SESSION, payload={}))
        if receipt.ok:
            self._write_chat("system", "会话已暂停（输入新消息即恢复）")

    def action_toggle_full_auto(self) -> None:
        session = self.rt.store.get_session(self.rt.session_id)
        mode = "full_auto" if session["permissions_mode"] != "full_auto" else "approved_scope"
        receipt = self.rt.submit(TeamAction(
            action_id=f"ui-mode-{mode}-{self._cursor}", session_id=self.rt.session_id,
            actor_id="user", kind=ActionKind.SET_PERMISSION_MODE,
            payload={"mode": mode}))
        if receipt.ok:
            self._write_chat("system", f"权限模式切换为 {mode}")

    def action_quit_app(self) -> None:
        self.exit()

    # -- approval decisions from the approvals table ------------------------

    def _decide_selected(self, decision: str) -> None:
        panel = self.query_one(ApprovalsPanel)
        approval_id = panel.selected_approval()
        if approval_id is None:
            return
        ok = panel.decide(self.rt, approval_id, decision)
        self._write_chat("system",
                         f"批准决定 {decision}：{'已提交' if ok else '提交失败'}")

    _ = (VerticalScroll, TaskStatus, Static)


def _payload(event) -> dict:
    import json
    try:
        return json.loads(event["payload_json"])
    except Exception:
        return {}


def _approval_line(payload: dict) -> str:
    scope = payload.get("scope") or {}
    tool = scope.get("tool") or scope.get("kind") or "?"
    args = scope.get("args") or scope.get("request") or {}
    return f"{payload.get('agent_id')} {tool} {str(args)[:60]}"
