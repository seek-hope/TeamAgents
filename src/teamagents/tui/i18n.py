"""TUI-only text and local preferences; user/model content stays untouched."""
from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path

from ..config import state_dir

# Original Chinese UI text serves as the message id. English is the default.
ENGLISH = {
    "团队": "Team", "任务": "Tasks", "共享空间": "Shared", "批准": "Approvals",
    "会话": "Sessions", "日志": "Log", "设置": "Settings", "退出": "Quit",
    "暂停/继续": "Pause/resume", "刷新": "Refresh", "全自动": "Full auto",
    "切换面板": "Panels", "停止 Leader": "Stop Leader", "输入": "Compose",
    "换行": "Newline", "你": "You", "系统": "System", "状态栏": "Status",
    "成员": "Member", "角色": "Role", "类型": "Type", "模型": "Model",
    "状态": "Status", "工作目录": "Workspace", "可见范围": "Access",
    "委派者": "Requester", "承接者": "Assignee", "描述": "Description",
    "依赖": "Dependencies", "结果": "Result", "创建时间": "Created",
    "空间": "Space", "作者": "Author", "内容/引用": "Content / reference",
    "序号": "Sequence", "目标": "Goal", "事件": "Events", "大小": "Size",
    "更新": "Updated", "标记": "Flags", "操作": "Operation", "参数": "Arguments",
    "范围": "Scope", "消息→": "Messages → ", "任务→": "Tasks → ", "被观察:": "Observers: ",
    "当前": "Current", "运行中": "Running", "已归档": "Archived", "读取异常": "Read error",
    "准备中": "Queued", "正在处理": "Working", "等待成员结果": "Waiting for members",
    "等待批准": "Approval needed", "就绪": "Ready", "无": "None", "未配置": "Not configured",
    "已提交": "Submitted", "提交失败": "Failed", "（完成）": " (done)",
    "全部事件": "All events", "事件流{v0}": "Event stream{v0}",
    "（成员 {v0} · Enter 取消）": " (member {v0} · Enter to clear)",
    "高亮成员=筛选日志 · Enter 取消筛选": "Highlight a member to filter the log · Enter clears the filter",
    "[没有可用会话]": "[No session available]",
    "⚠ 成员 {v0} 的模型 profile {v1!r} 未配置：请创建 {v2}（可直接复制仓库里的 examples/config.toml），然后重开会话。在此之前发出的消息都会失败。":
        "⚠ Member {v0}: model profile {v1!r} is missing. Create {v2} (see examples/config.toml) and reopen the session. Requests will fail until configured.",
    "⚠ 模型 profile {v0!r} 需要环境变量 {v1}，当前未设置：请 export 后重开会话。":
        "⚠ Model profile {v0!r} needs environment variable {v1}. Export it and reopen the session.",
    "[界面刷新失败] ": "[UI refresh failed] ",
    "[界面读取事件失败] {v0}": "[Event read failed] {v0}",
    "{v0} · Leader / {v1} · 可继续输入补充要求": "{v0} · Leader / {v1} · You can send follow-up instructions",
    "任务 {v0} → {v1}：{v2}": "Task {v0} → {v1}: {v2}",
    "需要批准：{v0}（按 Ctrl+G 处理）": "Approval required: {v0} (Ctrl+G to review)",
    "批准 {v0} → {v1}": "Approval {v0} → {v1}",
    "✗ 成员 {v0} 的回合失败：{v1}": "✗ Member {v0}: turn failed: {v1}",
    "回合已停止：{v0} ({v1})": "Turn stopped: {v0} ({v1})",
    "{v0} 正在等待 {v1} 个任务完成": "{v0} is waiting for {v1} tasks",
    "成员状态：{v0} {v1} {v2}": "Member status: {v0} {v1} {v2}",
    "会话状态：{v0}": "Session status: {v0}", "{v0}（完成）": "{v0} (done)",
    "[达到上限] {v0}": "[Limit reached] {v0}", "目标完成：{v0}": "Goal complete: {v0}",
    "✗ 无法打开会话 {v0}：{v1}": "✗ Cannot open session {v0}: {v1}",
    "已切换到会话 {v0}": "Switched to session {v0}", "✗ 归档失败：{v0}": "✗ Archive failed: {v0}",
    "已归档会话 {v0} → {v1}": "Archived session {v0} → {v1}",
    "✗ 删除失败：{v0}": "✗ Delete failed: {v0}", "✗ 删除被阻止：{v0}": "✗ Delete blocked: {v0}",
    "已删除会话 {v0}": "Deleted session {v0}", "✗ 无法确定工作目录": "✗ Cannot determine the workspace",
    "[输入被拒绝] {v0}": "[Input rejected] {v0}",
    "任务 {v0} | {v1} | {v2} | 依赖 {v3} | 成果 {v4}":
        "Task {v0} | {v1} | {v2} | Dependencies {v3} | Results {v4}",
    "已请求停止 Leader，等待执行结束": "Leader stop requested; waiting for execution to finish",
    "停止失败：{v0}": "Stop failed: {v0}", "继续执行": "Continue execution",
    "会话已暂停（输入新消息即恢复）": "Session paused (send a message to resume)",
    "权限模式切换为 {v0}": "Permission mode changed to {v0}",
    "批准决定 {v0}：{v1}": "Approval decision {v0}: {v1}",
    "预授权": "Pre-authorized", "已关闭": "Closed",
    "未完成任务 {count}": "Tasks {count}", "待批准 {count}": "Approvals {count}",
    "c=取消选中任务（BLOCKED 直接取消；执行中的回合收到取消请求）":
        "Newest first · c=cancel selected task (running turns receive a stop request)",
    "取消任务失败：{v0}": "Cannot cancel task: {v0}", "任务 {v0} 已取消": "Task {v0} cancelled",
    "任务 {v0} 已请求取消（活动回合结束后生效）": "Task {v0}: cancellation requested (waiting for the active turn to stop)",
    "任务 {v0} 已处于终态（{v1}），无需取消": "Task {v0} is already terminal ({v1}); no cancellation needed",
    "就绪 · 向 Leader 输入任务或补充要求": "Ready · Send a task or follow-up to Leader",
    "Enter 发送 · Shift+Enter / Ctrl+J 换行 · ↑↓ 历史 · Esc 停止 Leader":
        "Enter send · Shift+Enter / Ctrl+J newline · ↑↓ history · Esc stop Leader",
    "会话：{v0}": "Session: {v0}",
    "状态：{v0}    权限模式：{v1}    （Ctrl+F 切换）": "Status: {v0}    Permissions: {v1}    (Ctrl+F to toggle)",
    "工作目录：{v0}": "Workspace: {v0}", "团队：{v0} 名成员，拓扑修订 {v1}": "Team: {v0} members, topology revision {v1}",
    "上限：并发 {v0}、成员 {v1}、单目标回合 {v2}、单回合步骤 {v3}、回合超时 {v4}s":
        "Limits: {v0} workers, {v1} members, {v2} turns/goal, {v3} steps/turn, {v4}s timeout",
    "用户配置：{v0}": "User config: {v0}", "模型 profiles：": "Model profiles: ", "工具绑定：": "Tool bindings: ",
    "无（files/shell/web 为内置）": "None (files/shell/web are built in)", "Skills 目录：": "Skills directories: ",
    "指令文件：": "Instruction files: ", "恢复：teamagents --resume ": "Resume: teamagents --resume ",
    "    新建：换 --cwd 或删掉会话目录": "    New session: use the Sessions panel",
    "本目录会话：s=切换  n=新建  a=归档  d=删除（再按 d 确认，删当前会话后退出）":
        "Sessions: s=switch  n=new  a=archive  d=delete (press d again to confirm; deleting the current session exits)",
    "再按一次 d 确认删除会话 {v0}": "Press d again to delete session {v0}",
    "待批准操作：a=本次批准  s=会话内批准  d=拒绝": "Pending approvals: a=allow once  s=allow for session  d=deny",
    "界面语言": "Interface language", "动效": "Animations", "偏好保存失败：{v0}": "Could not save preferences: {v0}",
    "空闲": "Idle", "等待任务": "Waiting for tasks", "正在停止": "Stopping", "已移除": "Removed",
    "已完成": "Completed", "失败": "Failed", "已取消": "Cancelled", "受阻": "Blocked",
    "已暂停": "Paused", "已排队": "Queued", "结果不明": "Outcome unknown",
    "{count} 个成员正在执行": "Active agents: {count}", "没有正在执行的回合": "No turns executing",
    "最近活动：{text}": "Latest: {text}", "等待输入": "Waiting for input",
    "{agent} 开始处理": "{agent} started working", "{agent} 正在回复": "{agent} is responding",
    "任务已完成": "Task completed", "需要用户批准": "User approval needed",
}

TABLE_HEADERS = {
    "team-table": ("成员", "角色", "类型", "模型", "状态", "工作目录", "可见范围"),
    "tasks-table": ("任务", "委派者", "承接者", "状态", "描述", "依赖", "结果", "创建时间"),
    "shared-table": ("空间", "作者", "类型", "内容/引用", "序号"),
    "approvals-table": ("成员", "操作", "参数", "范围"),
    "sessions-table": ("会话", "状态", "目标", "事件", "大小", "更新", "标记"),
}
STATIC_LABELS = {
    "team-hint": "高亮成员=筛选日志 · Enter 取消筛选",
    "tasks-hint": "c=取消选中任务（BLOCKED 直接取消；执行中的回合收到取消请求）",
    "approvals-hint": "待批准操作：a=本次批准  s=会话内批准  d=拒绝",
    "sessions-hint": "本目录会话：s=切换  n=新建  a=归档  d=删除（再按 d 确认，删当前会话后退出）",
    "composer-hint": "Enter 发送 · Shift+Enter / Ctrl+J 换行 · ↑↓ 历史 · Esc 停止 Leader",
    "language-label": "界面语言", "animations-label": "动效",
}


def tr(owner, message: str, **values) -> str:
    language = (owner if isinstance(owner, str) else getattr(owner, "ui_language", None))
    if language is None:
        language = getattr(owner.app, "ui_language", "en")
    return (message if language == "zh-CN" else ENGLISH.get(message, message)).format(**values)


def preferences_path() -> Path:
    return state_dir() / "ui.json"


def read_preferences() -> dict:
    try:
        data = json.loads(preferences_path().read_text())
    except (OSError, ValueError):
        data = {}
    if not isinstance(data, dict):
        data = {}
    return {"language": "zh-CN" if data.get("language") == "zh-CN" else "en",
            "animations": data.get("animations") is not False}


def _atomic_json(path: Path, payload) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent,
                                         prefix=f".{path.stem}-", delete=False) as f:
            temporary = Path(f.name)
            json.dump(payload, f, ensure_ascii=False)
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def write_preferences(language: str, animations: bool) -> None:
    _atomic_json(preferences_path(), {"language": language, "animations": animations})


#: composer history is a user-level convenience file, capped like Codex's
HISTORY_LIMIT = 500


def history_path() -> Path:
    return state_dir() / "composer-history.json"


def read_history() -> list[str]:
    try:
        data = json.loads(history_path().read_text())
    except (OSError, ValueError):
        return []
    if not isinstance(data, list):
        return []
    return [item for item in data if isinstance(item, str)][-HISTORY_LIMIT:]


def write_history(entries: list[str]) -> None:
    _atomic_json(history_path(), entries[-HISTORY_LIMIT:])
