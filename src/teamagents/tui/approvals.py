"""Approval queue panel: decide once / for the session / deny (section 13)."""

from __future__ import annotations

from textual.containers import Vertical
from textual.widgets import DataTable, Static

from ..models import ActionKind, TeamAction
from .panels import PanelReady, save_table_cursor, restore_table_cursor
from .i18n import tr, TABLE_HEADERS


class ApprovalsPanel(Vertical):
    """Pending approvals with their exact operation and parameter scope.

    Keys: a = approve once, s = approve for the session, d = deny.
    """

    can_focus = True

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.runtime = None

    def compose(self):
        yield DataTable(id="approvals-table", zebra_stripes=True, cursor_type="row")
        yield Static(tr(self, '待批准操作：a=本次批准  s=会话内批准  d=拒绝'), id="approvals-hint")

    def on_mount(self) -> None:
        table = self.query_one("#approvals-table", DataTable)
        table.add_columns(*(tr(self, label) for label in TABLE_HEADERS["approvals-table"]))
        self.post_message(PanelReady())

    def refresh_from(self, store, session_id: str, runtime=None) -> None:
        if runtime is not None:
            self.runtime = runtime
        table = self.query_one("#approvals-table", DataTable)
        saved = save_table_cursor(table)
        table.clear()
        for approval in store.pending_approvals(session_id):
            scope = approval.requested_scope or {}
            tool = scope.get("tool") or scope.get("kind") or "?"
            args = scope.get("args") or scope.get("request") or {}
            table.add_row(approval.agent_id, str(tool),
                          str(args)[:60], str(scope.get("reason", ""))[:40],
                          key=approval.approval_id)
        restore_table_cursor(table, saved)

    def selected_approval(self) -> str | None:
        table = self.query_one("#approvals-table", DataTable)
        if not table.row_count:
            return None
        try:
            return str(table.coordinate_to_cell_key(table.cursor_coordinate).row_key.value)
        except Exception:
            return None

    def on_key(self, event) -> None:
        if event.key == "a":
            self._decide(event, "once")
        elif event.key == "s":
            self._decide(event, "session")
        elif event.key == "d":
            self._decide(event, "deny")

    def _decide(self, event, decision: str) -> None:
        approval_id = self.selected_approval()
        if approval_id is None or self.runtime is None:
            return
        ok = self.decide(self.runtime, approval_id, decision)
        try:
            self.app.notify(tr(self, '批准决定 {v0}：{v1}', v0=decision, v1=tr(self, '已提交') if ok else tr(self, '提交失败')))
        except Exception:
            pass
        event.stop()

    @staticmethod
    def decide(runtime, approval_id: str, decision: str) -> bool:
        receipt = runtime.submit(TeamAction(
            action_id=f"ui-approval-{approval_id}-{decision}",
            session_id=runtime.session_id, actor_id="user",
            kind=ActionKind.APPROVAL_DECISION,
            payload={"approval_id": approval_id, "decision": decision}))
        return receipt.ok
