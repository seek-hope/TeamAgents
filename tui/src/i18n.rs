//! TUI-only text and local preferences.
//! Chinese UI text is the message id; English is the default rendering.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

pub const HISTORY_LIMIT: usize = 500;

/// (message id, english) pairs.
static ENGLISH: &[(&str, &str)] = &[
    ("团队", "Team"),
    ("任务", "Tasks"),
    ("共享空间", "Shared"),
    ("批准", "Approvals"),
    ("会话", "Sessions"),
    ("日志", "Log"),
    ("设置", "Settings"),
    ("退出", "Quit"),
    ("暂停/继续", "Pause/resume"),
    ("刷新", "Refresh"),
    ("全自动", "Full auto"),
    ("切换面板", "Panels"),
    ("停止 Leader", "Stop Leader"),
    ("输入", "Compose"),
    ("换行", "Newline"),
    ("你", "You"),
    ("系统", "System"),
    ("状态栏", "Status"),
    ("成员", "Member"),
    ("角色", "Role"),
    ("类型", "Type"),
    ("模型", "Model"),
    ("状态", "Status"),
    ("工作目录", "Workspace"),
    ("可见范围", "Access"),
    ("委派者", "Requester"),
    ("承接者", "Assignee"),
    ("描述", "Description"),
    ("依赖", "Dependencies"),
    ("结果", "Result"),
    ("创建时间", "Created"),
    ("空间", "Space"),
    ("作者", "Author"),
    ("内容/引用", "Content / reference"),
    ("序号", "Sequence"),
    ("目标", "Goal"),
    ("事件", "Events"),
    ("大小", "Size"),
    ("更新", "Updated"),
    ("标记", "Flags"),
    ("操作", "Operation"),
    ("参数", "Arguments"),
    ("范围", "Scope"),
    ("消息→", "Messages → "),
    ("任务→", "Tasks → "),
    ("被观察:", "Observers: "),
    ("当前", "Current"),
    ("运行中", "Running"),
    ("已归档", "Archived"),
    ("读取异常", "Read error"),
    ("准备中", "Queued"),
    ("正在处理", "Working"),
    ("等待成员结果", "Waiting for members"),
    ("等待批准", "Approval needed"),
    ("就绪", "Ready"),
    ("无", "None"),
    ("查看 token 用量与上下文窗口", "Show token usage and context window"),
    ("查看或切换成员模型与推理档位", "Show or switch a member's model / reasoning effort"),
    ("获取模型信息失败：{v0}", "Failed to fetch model info: {v0}"),
    ("成员模型（* = 会话内覆盖，重开会话失效）：", "Member models (* = session override, lost when the session reopens):"),
    ("{v0} | 模型 {v1} | 档位 {v2}{v3}", "{v0} | model {v1} | effort {v2}{v3}"),
    ("模型切换失败：{v0}", "Model switch failed: {v0}"),
    ("已切换 {v0}：模型 {v1} · 档位 {v2}（下一回合生效）", "Switched {v0}: model {v1} · effort {v2} (applies from the next turn)"),
    ("已恢复 {v0} 的 profile 默认：模型 {v1} · 档位 {v2}", "Restored {v0} to the profile default: model {v1} · effort {v2}"),
    ("用法：/model <成员> <模型> [档位] · /model <成员> clear 恢复默认",
     "Usage: /model <member> <model> [effort] · /model <member> clear restores the profile"),
    ("Token 用量（本次会话累计，重启归零）：", "Token usage (this session, reset on restart):"),
    ("获取用量失败：{v0}", "Failed to fetch usage: {v0}"),
    ("获取回退点失败：{v0}", "Failed to fetch rewind points: {v0}"),
    ("回退对话到历史节点（/rewind 列出，/rewind <序号> 回退）", "Rewind the conversation to a history node (/rewind lists, /rewind <n> rewinds)"),
    ("从当前对话分叉新会话（团队事实不复制）", "Fork a new session from this conversation (team facts are not copied)"),
    ("没有这个序号（先用 /rewind 列出可回退点）", "No such index (list rewind points with /rewind first)"),
    ("暂无可回退的节点（leader 还没有对话历史）", "Nothing to rewind to yet (the leader has no history)"),
    ("可回退点（/rewind <序号> 保留该条输入，移开后续对话）：", "Rewind points (/rewind <n> keeps that input and branches off later messages):"),
    ("已回退对话（当前 {v0} 条消息；被放弃的分支仍保留，可再次 /rewind）", "Rewound ({v0} messages now; the abandoned branch is kept — /rewind again to go further)"),
    ("回退失败：{v0}", "Rewind failed: {v0}"),
    ("已从 {v0} 分叉到 {v1}（对话已带上，团队状态全新）", "Forked {v0} into {v1} (conversation carried over, team state fresh)"),
    ("分叉失败：{v0}", "Fork failed: {v0}"),
    ("斜杠命令：/help 本说明 · /settings 设置 · /status 用量 · /model 模型 · /rewind 回退 · /fork 分叉 · /quit 退出",
     "Slash commands: /help this text · /settings preferences · /status usage · /model models · /rewind rewind · /fork fork · /quit exit"),
    ("{v0} | {v1} | 窗口 {v2} | {v3} ({v4}/{v5}) | 剩余 {v6}", "{v0} | {v1} | window {v2} | {v3} ({v4}/{v5}) | remaining {v6}"),
    ("未配置", "Not configured"),
    ("已提交", "Submitted"),
    ("提交失败", "Failed"),
    ("（完成）", " (done)"),
    ("全部事件", "All events"),
    ("事件流{v0}", "Event stream{v0}"),
    ("（成员 {v0} · Enter 取消）", " (member {v0} · Enter to clear)"),
    ("高亮成员=筛选日志 · Enter 取消筛选", "Highlight a member to filter the log · Enter clears the filter"),
    ("[没有可用会话]", "[No session available]"),
    ("⚠ 成员 {v0} 的模型 profile {v1!r} 未配置：请创建 {v2}（可直接复制仓库里的 examples/config.toml），然后重开会话。在此之前发出的消息都会失败。", "⚠ Member {v0}: model profile {v1!r} is missing. Create {v2} (see examples/config.toml) and reopen the session. Requests will fail until configured."),
    ("⚠ 模型 profile {v0!r} 需要环境变量 {v1}，当前未设置：请 export 后重开会话。", "⚠ Model profile {v0!r} needs environment variable {v1}. Export it and reopen the session."),
    ("[界面刷新失败] ", "[UI refresh failed] "),
    ("[界面读取事件失败] {v0}", "[Event read failed] {v0}"),
    ("{v0} · Leader / {v1} · 可继续输入补充要求", "{v0} · Leader / {v1} · You can send follow-up instructions"),
    ("任务 {v0} → {v1}：{v2}", "Task {v0} → {v1}: {v2}"),
    ("需要批准：{v0}（按 Ctrl+G 处理）", "Approval required: {v0} (Ctrl+G to review)"),
    ("批准 {v0} → {v1}", "Approval {v0} → {v1}"),
    ("✗ 成员 {v0} 的回合失败：{v1}", "✗ Member {v0}: turn failed: {v1}"),
    ("回合已停止：{v0} ({v1})", "Turn stopped: {v0} ({v1})"),
    ("{v0} 正在等待 {v1} 个任务完成", "{v0} is waiting for {v1} tasks"),
    ("成员状态：{v0} {v1} {v2}", "Member status: {v0} {v1} {v2}"),
    ("会话状态：{v0}", "Session status: {v0}"),
    ("{v0}（完成）", "{v0} (done)"),
    ("[达到上限] {v0}", "[Limit reached] {v0}"),
    ("目标完成：{v0}", "Goal complete: {v0}"),
    ("✗ 无法打开会话 {v0}：{v1}", "✗ Cannot open session {v0}: {v1}"),
    ("已切换到会话 {v0}", "Switched to session {v0}"),
    ("✗ 归档失败：{v0}", "✗ Archive failed: {v0}"),
    ("已归档会话 {v0} → {v1}", "Archived session {v0} → {v1}"),
    ("会话已归档，不能切换", "Session is archived; cannot switch"),
    ("会话已归档，不能归档/删除", "Session is archived; cannot archive/delete"),
    ("✗ 删除失败：{v0}", "✗ Delete failed: {v0}"),
    ("✗ 删除被阻止：{v0}", "✗ Delete blocked: {v0}"),
    ("已删除会话 {v0}", "Deleted session {v0}"),
    ("✗ 无法确定工作目录", "✗ Cannot determine the workspace"),
    ("[输入被拒绝] {v0}", "[Input rejected] {v0}"),
    ("任务 {v0} | {v1} | {v2} | 依赖 {v3} | 成果 {v4}", "Task {v0} | {v1} | {v2} | Dependencies {v3} | Results {v4}"),
    ("已请求停止 Leader，等待执行结束", "Leader stop requested; waiting for execution to finish"),
    ("停止失败：{v0}", "Stop failed: {v0}"),
    ("继续执行", "Continue execution"),
    ("会话已暂停（输入新消息即恢复）", "Session paused (send a message to resume)"),
    ("权限模式切换为 {v0}", "Permission mode changed to {v0}"),
    ("批准决定 {v0}：{v1}", "Approval decision {v0}: {v1}"),
    ("预授权", "Pre-authorized"),
    ("已关闭", "Closed"),
    ("未完成任务 {count}", "Tasks {count}"),
    ("待批准 {count}", "Approvals {count}"),
    ("c=取消选中任务（BLOCKED 直接取消；执行中的回合收到取消请求）", "Newest first · c=cancel selected task (running turns receive a stop request)"),
    ("取消任务失败：{v0}", "Cannot cancel task: {v0}"),
    ("任务 {v0} 已取消", "Task {v0} cancelled"),
    ("任务 {v0} 已请求取消（活动回合结束后生效）", "Task {v0}: cancellation requested (waiting for the active turn to stop)"),
    ("任务 {v0} 已处于终态（{v1}），无需取消", "Task {v0} is already terminal ({v1}); no cancellation needed"),
    ("就绪 · 向 Leader 输入任务或补充要求", "Ready · Send a task or follow-up to Leader"),
    ("Enter 发送 · Shift+Enter / Ctrl+J 换行 · ↑↓ 历史 · Esc 停止 Leader", "Enter send · Shift+Enter / Ctrl+J newline · ↑↓ history · Esc stop Leader"),
    ("会话：{v0}", "Session: {v0}"),
    ("状态：{v0}    权限模式：{v1}", "Status: {v0}    Permissions: {v1}"),
    ("工作目录：{v0}", "Workspace: {v0}"),
    ("团队：{v0} 名成员，拓扑修订 {v1}", "Team: {v0} members, topology revision {v1}"),
    ("上限：并发 {v0}、成员 {v1}、单目标回合 {v2}、单回合步骤 {v3}、回合超时 {v4}s", "Limits: {v0} workers, {v1} members, {v2} turns/goal, {v3} steps/turn, {v4}s timeout"),
    ("用户配置：{v0}", "User config: {v0}"),
    ("模型 profiles：", "Model profiles: "),
    ("工具绑定：", "Tool bindings: "),
    ("无（files/shell/web 为内置）", "None (files/shell/web are built in)"),
    ("Skills 目录：", "Skills directories: "),
    ("指令文件：", "Instruction files: "),
    ("恢复：teamagents --resume ", "Resume: teamagents --resume "),
    ("    新建：换 --cwd 或删掉会话目录", "    New session: use the Sessions panel"),
    ("本目录会话：s=切换  n=新建  a=归档  d=删除（再按 d 确认，删当前会话后退出）", "Sessions: s=switch  n=new  a=archive  d=delete (press d again to confirm; deleting the current session exits)"),
    ("再按一次 d 确认删除会话 {v0}", "Press d again to delete session {v0}"),
    ("待批准操作：a=本次批准  s=会话内批准  d=拒绝", "Pending approvals: a=allow once  s=allow for session  d=deny"),
    ("界面语言", "Interface language"),
    ("偏好保存失败：{v0}", "Could not save preferences: {v0}"),
    ("空闲", "Idle"),
    ("等待任务", "Waiting for tasks"),
    ("正在停止", "Stopping"),
    ("已移除", "Removed"),
    ("已完成", "Completed"),
    ("失败", "Failed"),
    ("已取消", "Cancelled"),
    ("受阻", "Blocked"),
    ("已暂停", "Paused"),
    ("已排队", "Queued"),
    ("结果不明", "Outcome unknown"),
    ("{count} 个成员正在执行", "Active agents: {count}"),
    ("没有正在执行的回合", "No turns executing"),
    ("最近活动：{text}", "Latest: {text}"),
    ("等待输入", "Waiting for input"),
    ("{agent} 开始处理", "{agent} started working"),
    ("{agent} 正在回复", "{agent} is responding"),
    ("任务已完成", "Task completed"),
    // Rust-native shell extras (D-20): empty states, scroll markers, hints
    ("没有成员", "No members yet"),
    ("没有任务", "No tasks yet — ask the Leader to delegate"),
    ("没有待批准操作", "Nothing waiting for approval"),
    ("没有共享条目", "No shared entries yet"),
    ("没有会话记录", "No sessions in this directory"),
    ("输入你的目标，或向 Leader 补充要求（/settings 打开设置）",
     "Ask the Leader…  (/settings opens preferences)"),
    ("已上翻{count}行 · Ctrl+End 回到底部", "{count} lines up · Ctrl+End for newest"),
    ("面板：{v0}", "pane: {v0}"),
    ("滚动", "Scroll"),
    ("命令", "Commands"),
    ("/help", "/help"),
    ("/quit", "/quit"),
    ("/settings", "/settings"),
    ("/status", "/status"),
    ("/model", "/model"),
    ("选择成员", "Choose a member"),
    ("选择模型供应商", "Choose a provider"),
    (" · 供应商 {v0}", " · Provider {v0}"),
    ("选择模型", "Choose a model"),
    (" · 在线", " · online"),
    ("在线模型已更新", "Online models updated"),
    ("在线模型获取失败；已配置模型仍可用：", "Online discovery failed; configured models remain available:"),
    ("部分在线模型获取失败；已配置模型仍可用：", "Some catalogs failed; configured models remain available:"),
    ("正在获取在线模型；已配置模型可直接选择…", "Fetching online models; configured models are ready to select…"),
    ("选择思考强度", "Choose reasoning effort"),
    ("恢复此成员的默认模型", "Restore this member's default model"),
    ("使用默认档位", "Use default effort"),
    ("↑↓ 选择 · Enter 确认 · Esc 返回/关闭", "↑↓ select · Enter confirm · Esc back/close"),
    ("搜索：", "Search:"),
    ("没有匹配项；检查搜索词或 config.toml 模型配置", "No matches; check the search or models in config.toml"),
    ("显示快捷键与斜杠命令说明", "Show the key map and slash commands"),
    ("退出 TeamAgents", "Quit TeamAgents"),
    ("打开设置浮层（界面语言）", "Open the settings overlay (language)"),
    ("未知命令：{v0}（/help 查看可用命令）", "Unknown command: {v0} (/help lists the commands)"),
    ("输入：Enter 发送 · Shift+Enter 换行 · ↑↓ 历史 · PgUp/PgDn 滚动",
     "Composer: Enter send · Shift+Enter newline · ↑↓ history · PgUp/PgDn scroll"),
    ("界面：Ctrl+T 切面板 · Ctrl+G 批准 · Ctrl+F 全自动 · Ctrl+P 暂停 · Esc 停止 Leader · Ctrl+Q 退出",
     "Shell: Ctrl+T panes · Ctrl+G approvals · Ctrl+F full auto · Ctrl+P pause · Esc stops the Leader · Ctrl+Q quit"),
    ("Enter 发送", "Enter send"),
    ("Esc 停止 Leader", "Esc stops the Leader"),
    ("↑↓ 历史", "↑↓ history"),
    ("PgUp/PgDn 滚动", "PgUp/PgDn scroll"),
    ("Shift+Enter 换行", "Shift+Enter newline"),
    ("Enter 发送 · Shift+Enter 换行 · ↑↓ 历史 · PgUp/PgDn 滚动 · Esc 停止 Leader",
     "Enter send · Shift+Enter newline · ↑↓ history · PgUp/PgDn scroll · Esc stops the Leader"),
    ("共享空间条目：作者 / 类型 / 内容或引用", "Shared entries: author / kind / content or reference"),
    ("↑↓ 选择成员筛选 · Enter 取消 · PgUp/PgDn 滚动", "↑↓ filter by member · Enter clears · PgUp/PgDn scroll"),
    ("Enter 选择语言 · Esc 关闭", "Enter picks the language · Esc closes"),
    ("需要用户批准", "User approval needed"),
];

fn en_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| ENGLISH.iter().copied().collect())
}

/// tr(): zh-CN keeps the message id, anything else renders English.
/// Args fill {name} placeholders; {name!r} wraps the value in single quotes.
pub fn tr(lang: &str, msg: &str, args: &[(&str, &str)]) -> String {
    let template = if lang == "zh-CN" {
        msg.to_string()
    } else {
        en_map().get(msg).copied().unwrap_or(msg).to_string()
    };
    let mut out = template;
    for (k, v) in args {
        out = out.replace(&format!("{{{k}!r}}"), &format!("'{v}'"));
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// activity_status labels: turn/task status -> message id.
pub fn status_label_id(status: &str) -> &str {
    match status {
        "RUNNING" | "BUSY" => "正在处理",
        "IDLE" => "空闲",
        "QUEUED" | "PENDING" => "已排队",
        "WAITING" => "等待任务",
        "WAITING_TASK" => "等待成员结果",
        "WAITING_APPROVAL" => "等待批准",
        "DRAINING" => "正在停止",
        "REMOVED" => "已移除",
        "SUCCEEDED" | "COMPLETED" => "已完成",
        "FAILED" => "失败",
        "CANCELLED" => "已取消",
        "BLOCKED" => "受阻",
        "OUTCOME_UNKNOWN" => "结果不明",
        other => other,
    }
}

/// Table headers, keyed by panel id.
pub fn table_headers(table: &str) -> &'static [&'static str] {
    match table {
        "team" => &["成员", "角色", "类型", "模型", "状态", "工作目录", "可见范围"],
        "tasks" => &["任务", "委派者", "承接者", "状态", "描述", "依赖", "结果", "创建时间"],
        "shared" => &["空间", "作者", "类型", "内容/引用", "序号"],
        "approvals" => &["成员", "操作", "参数", "范围"],
        "sessions" => &["会话", "状态", "目标", "事件", "大小", "更新", "标记"],
        _ => &[],
    }
}

pub(crate) fn state_dir() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state"));
    base.join("teamagents")
}

#[derive(Clone, Copy, PartialEq)]
pub struct Prefs {
    pub language: &'static str, // "en" | "zh-CN"
    pub animations: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { language: "en", animations: true }
    }
}

pub fn read_preferences() -> Prefs {
    let data = std::fs::read_to_string(state_dir().join("ui.json")).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
    Prefs {
        language: if v.get("language").and_then(|x| x.as_str()) == Some("zh-CN") { "zh-CN" } else { "en" },
        animations: v.get("animations").and_then(|x| x.as_bool()) != Some(false),
    }
}

fn atomic_write_json(path: PathBuf, payload: &serde_json::Value) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string(payload)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn write_preferences(language: &str, animations: bool) -> std::io::Result<()> {
    atomic_write_json(state_dir().join("ui.json"),
        &serde_json::json!({"language": language, "animations": animations}))
}

pub fn read_history() -> Vec<String> {
    let data = std::fs::read_to_string(state_dir().join("composer-history.json")).unwrap_or_default();
    let v: serde_json::Value = serde_json::from_str(&data).unwrap_or(serde_json::Value::Null);
    let mut items: Vec<String> = v.as_array().map(|a| {
        a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
    }).unwrap_or_default();
    if items.len() > HISTORY_LIMIT {
        items.drain(..items.len() - HISTORY_LIMIT);
    }
    items
}

pub fn write_history(entries: &[String]) -> std::io::Result<()> {
    let start = entries.len().saturating_sub(HISTORY_LIMIT);
    atomic_write_json(state_dir().join("composer-history.json"),
        &serde_json::json!(entries[start..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tr_translates_and_formats() {
        assert_eq!(tr("en", "团队", &[]), "Team");
        assert_eq!(tr("zh-CN", "团队", &[]), "团队");
        assert_eq!(tr("en", "待批准 {count}", &[("count", "3")]), "Approvals 3");
        assert_eq!(tr("en", "{agent} 开始处理", &[("agent", "leader")]), "leader started working");
    }

    #[test]
    fn tr_repr_quotes() {
        let s = tr("en", "⚠ 模型 profile {v0!r} 需要环境变量 {v1}，当前未设置：请 export 后重开会话。",
                   &[("v0", "leader_main"), ("v1", "OPENAI_API_KEY")]);
        assert!(s.contains("'leader_main'"), "{s}");
        assert!(s.contains("OPENAI_API_KEY"), "{s}");
    }

    #[test]
    fn status_labels_cover_all() {
        for (s, en) in [("RUNNING", "Working"), ("IDLE", "Idle"), ("SUCCEEDED", "Completed"),
                        ("BLOCKED", "Blocked"), ("OUTCOME_UNKNOWN", "Outcome unknown")] {
            assert_eq!(tr("en", status_label_id(s), &[]), en);
        }
    }
}
