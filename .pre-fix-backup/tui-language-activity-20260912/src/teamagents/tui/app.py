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
from textual.containers import Vertical, VerticalScroll
from textual.widgets import Footer, Header, Static, TabbedContent, TabPane

from ..models import EventKind, TaskStatus, TeamAction, ActionKind, TurnStatus, new_id
from ..runtime import SessionRuntime
from .approvals import ApprovalsPanel
from .panels import (ChatLog, InputRow, LogPanel, PromptInput, SettingsPanel,
                     SessionsPanel, SharedPanel, StatusBar, TasksPanel, TeamPanel,
                     PanelReady)
from .theme import CODEX_THEME

TOP_PANELS = [("team", "团队"), ("tasks", "任务"), ("shared", "共享空间"),
                ("approvals", "批准"), ("sessions", "会话"), ("log", "日志"),
                ("settings", "设置")]


class TeamAgentsApp(App):
    """Terminal UI over a SessionRuntime."""

    CSS = """
    /* Codex-like: flat, monochrome, one accent, thin borders */
    Screen { layout: vertical; background: $background; color: $foreground; }
    #body { height: 1fr; }
    #side { height: 2fr; min-height: 6; border-bottom: solid $secondary; padding: 0 1; }
    #chat { height: 3fr; min-height: 9; padding: 0 1; background: $background; }
    #chat-log { height: 1fr; min-height: 1; }
    #chat-live { height: auto; max-height: 8; padding: 0 1; color: $foreground; display: none; }
    #input-row { height: auto; }
    #composer { height: auto; }
    #prompt-prefix { width: 2; padding-top: 1; color: $primary; text-style: bold; }
    #composer-status, #composer-hint { height: 1; color: $text-muted; }
    #chat-stream { height: 1fr; }
    .panel { height: 1fr; }
    #prompt {
        height: 3;
        width: 1fr;
        border: none;
        padding: 1;
        background: $panel;
        color: $foreground;
    }
    #prompt:focus { background: $panel; }
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
    #settings-body, #approvals-hint, #tasks-hint, #log-title { color: $text-muted; padding: 0 0 1 0; }
    Header { background: $panel; color: $text-muted; }
    Footer { background: $panel; color: $text-muted; }
    Footer > .footer--key { background: $boost; color: $foreground; }
    """

    BINDINGS = [
        Binding("ctrl+q", "quit_app", "退出", priority=True),
        Binding("ctrl+p", "pause_session", "暂停/继续", priority=True),
        Binding("ctrl+r", "refresh_all", "刷新", priority=True),
        Binding("ctrl+f", "toggle_full_auto", "全自动", priority=True),
        Binding("ctrl+t", "cycle_panel", "切换面板", priority=True),
        Binding("ctrl+g", "focus_approvals", "批准", priority=True),
        Binding("escape", "interrupt_leader", "停止 Leader", priority=True),
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
        self._delta_buffer: dict[str, str] = {}
        self._stream_dirty = False
        self._log_cursor = 0
        self._log_member: str | None = None
        self._refresh_errors: tuple[str, ...] = ()
        self._last_event_seen = 0
        self._owns_runtime = runtime is None
        self._runtime_closed = False

    # ------------------------------------------------------------- layout

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield StatusBar("", id="status")
        with Vertical(id="body"):
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
            with Vertical(id="chat"):
                yield ChatLog(id="chat-log")
                yield InputRow(id="input-row")
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

    # ------------------------------------------------------------- rendering

    async def _refresh_widgets(self) -> None:
        """Periodic state render; one broken widget must not freeze the rest."""
        if self.rt is None:
            return
        store = self.rt.store
        session_id = self.rt.session_id
        errors: list[str] = []

        def refresh(name: str, update) -> None:
            try:
                update()
            except Exception as error:  # a UI read error must not kill the app
                errors.append(f"{name}: {error}")

        refresh("状态栏",
                lambda: self.query_one("#status", StatusBar).refresh_status(self.rt))
        for panel_type in (TeamPanel, TasksPanel, SharedPanel, ApprovalsPanel,
                           SettingsPanel):
            for panel in self.query(panel_type):
                refresh(panel_type.__name__,
                        lambda p=panel: p.refresh_from(store, session_id, self.rt))
        for panel in self.query(LogPanel):
            refresh("LogPanel", lambda p=panel: self._append_log(p))
        if errors and tuple(errors) != self._refresh_errors:
            # report a broken widget once, not once per tick
            self._write_chat("system", "[界面刷新失败] " + "；".join(errors))
        self._refresh_errors = tuple(errors)
        spec = store.load_team_spec(session_id)
        runs = [r for r in store.runs_for_session(session_id,
                ["QUEUED", "RUNNING", "WAITING_TASK", "WAITING_APPROVAL"])
                if r.agent_id == spec.leader_id]
        state = {"QUEUED": "准备中", "RUNNING": "正在处理", "WAITING_TASK": "等待成员结果",
                 "WAITING_APPROVAL": "等待批准"}.get(runs[0].status if runs else "", "就绪")
        self.query_one("#composer-status", Static).update(
            f"{state} · Leader / {spec.leader.model_profile} · 可继续输入补充要求")

    def _append_log(self, panel: LogPanel) -> None:
        """Append new events; the log cursor is independent of the chat cursor."""
        if self._selected_member != self._log_member:
            self._log_member = self._selected_member
            self._log_cursor = panel.replay_from(
                self.rt.store, self.rt.session_id, self._selected_member)
        else:
            self._log_cursor = panel.refresh_from(
                self.rt.store, self.rt.session_id, self._selected_member,
                self._log_cursor)

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
                self.query_one(PromptInput).record_submission(payload.get("text", ""))
            elif kind == EventKind.LEADER_REPLY:
                self._delta_buffer.pop(payload.get("run_id"), None)
                self.query_one(ChatLog).show_stream("")
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
                                 f"需要批准：{_approval_line(payload)}（按 Ctrl+G 处理）")
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
        if self.rt is None:
            return
        if agent_id != self.rt.store.load_team_spec(self.rt.session_id).leader_id:
            return
        # Transient preview is bounded; the full final reply is persisted.
        self._delta_buffer[run_id] = (self._delta_buffer.get(run_id, "") + text)[-32000:]
        self._stream_dirty = True

    def _flush_deltas(self) -> None:
        if self.rt is None:
            return
        for run_id in list(self._delta_buffer):
            run = self.rt.store.get_run(run_id)
            if run is not None and run.status in (TurnStatus.COMPLETED, TurnStatus.FAILED,
                                                 TurnStatus.CANCELLED, TurnStatus.OUTCOME_UNKNOWN):
                del self._delta_buffer[run_id]
                self._stream_dirty = True
        if self._stream_dirty:
            self.query_one(ChatLog).show_stream("\n\n".join(self._delta_buffer.values()))
            self._stream_dirty = False

    def on_tabbed_content_tab_activated(self, event) -> None:
        """A pane mounts lazily: refresh it as soon as it becomes visible."""
        self._refresh_panel_for(getattr(event.pane, "id", None))

    def on_panel_ready(self, event: PanelReady) -> None:
        """A panel finished mounting: give it the current state."""
        panel = getattr(event, "control", None)
        if panel is None or self.rt is None:
            return
        if isinstance(panel, LogPanel):
            # its refresh takes a member filter + cursor, and a freshly
            # mounted pane must not start empty (B-06)
            with contextlib.suppress(Exception):
                self._log_cursor = panel.replay_from(
                    self.rt.store, self.rt.session_id, self._selected_member)
                self._log_member = self._selected_member
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
            "tab-sessions": SessionsPanel, "tab-log": LogPanel,
            "tab-settings": SettingsPanel,
        }.get(tab_id or "")
        if widget is None:
            return
        with contextlib.suppress(Exception):
            panel = self.query_one(widget)
            if widget is LogPanel:
                self._log_cursor = panel.replay_from(store, session_id,
                                                     self._selected_member)
                self._log_member = self._selected_member
            else:
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
        self.query_one(ChatLog).show_stream("")
        self.query_one(PromptInput).reset_history()
        self._log_cursor = 0
        self._log_member = None
        self._refresh_errors = ()
        self.sub_title = self.rt.session_id
        with contextlib.suppress(Exception):
            self.query_one(ChatLog).clear()
        with contextlib.suppress(Exception):
            self.query_one("#log-stream").clear()
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
            self.query_one(PromptInput).text = message.text
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
        ids = [f"tab-{name}" for name, _ in TOP_PANELS]
        try:
            index = (ids.index(tabs.active) + 1) % len(ids)
        except ValueError:
            index = 0
        tabs.active = ids[index]

    def action_refresh_all(self) -> None:
        self._refresh_now()

    def action_interrupt_leader(self) -> None:
        if self.rt is None:
            return
        leader_id = self.rt.store.load_team_spec(self.rt.session_id).leader_id
        for run in self.rt.store.runs_for_session(self.rt.session_id,
                ["QUEUED", "RUNNING", "WAITING_TASK", "WAITING_APPROVAL"]):
            if run.agent_id == leader_id:
                receipt = self.rt.submit(TeamAction(
                    action_id=new_id("ui-stop"), session_id=self.rt.session_id,
                    actor_id="user", kind=ActionKind.CANCEL_RUN,
                    payload={"run_id": run.run_id}))
                self._write_chat("system", "已请求停止 Leader，等待执行结束"
                                 if receipt.ok else f"停止失败：{receipt.error}")
                break

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
