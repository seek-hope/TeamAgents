# TUI 修复笔记（impl_tui）

负责人：impl_tui。任务：task_fcf54d8eb724 / task_17a1dd8d204c（A-03 + B-02）、
task_83eabc6071f2（B-07 显示侧：会话面板重复行 key）。
文件所有权：仅 `src/teamagents/tui/*` + 新增 `tests/test_*.py`。

## 基线复现（改动前）

```bash
cd /home/rimuru/Projects/Code/for_fun/TeamAgents
rm -rf /tmp/ta-repro
PYTHONPATH=tests XDG_STATE_HOME=/tmp/ta-repro/state XDG_CONFIG_HOME=/tmp/ta-repro/config \
  .venv/bin/python review/tmp/repro_surface.py
```

关键输出（2026-09-12，textual 8.2.8，Python 3.13.15）：

```
[B-02] query_one('#status', StatusBar) -> WrongType ... found Static(id='sta
[B-02] status text: ''
[B-02] LogPanel.refresh_from called by periodic refresh: 0
[B-06] log-stream lines after opening log tab: 0
[B-07] duplicate row key -> DuplicateKey
```

另用临时探针确认：textual 8.2.8 下 7 个面板启动即全部挂载（`app.query(Panel).nodes` 均为 1），
因此周期刷新可逐个安全刷新；旧 `on_panel_ready` 对 LogPanel 用错签名（3 参数 vs 实际 4 参数），
异常被 suppress 吞掉。

## A-03 + B-02 修复（task_fcf54d8eb724 / task_17a1dd8d204c）

### 改动

`src/teamagents/tui/app.py`
- **B-02 根因**：`compose` 产出 `StatusBar("", id="status")`（app.py:112，原为 `Static`）。
- `_refresh_widgets`（app.py:207-233）由「整段一个 suppress」改为**逐 widget try/except**：
  一个 widget 读错误不再冻结其余刷新；错误聚合成一条 `[界面刷新失败] ...` 聊天提示，
  且仅在错误集合变化时提示一次（`_refresh_errors` 去重，app.py:103/230-233），
  保留「UI 读错误不杀 app」的弹性（两处均注释说明）。
- `_append_log`（app.py:235-246）：新增独立 `_log_cursor`（app.py:101），
  修掉旧代码把 LogPanel 高水位写回聊天游标的问题（B-05 第一条）；
  成员过滤变化时 `replay_from` 重建。
- `on_panel_ready`（app.py:334-351）与 `_refresh_panel_for`（app.py:353-375）：
  日志面板用正确的 `replay_from` 签名；`tab-log` 纳入 tab→面板映射（B-06）。
- `switch_session` 重置 `_log_cursor/_log_member/_refresh_errors`（app.py:408-410）。
- CSS 增加 `#tasks-hint`（app.py:73）。

`src/teamagents/tui/panels.py`
- **A-03**：`TasksPanel`（panels.py:142-231）
  - 提示行 `c=取消选中任务（BLOCKED 直接取消；执行中的回合收到取消请求）`（compose）；
  - `on_key` 处理 `c` → `_cancel_selected`（panels.py:206）→ `TasksPanel.cancel`
    （panels.py:222）以 `runtime.submit(TeamAction(kind=CANCEL_TASK, actor_id="user"))`
    走唯一控制面（模式参考 `ApprovalsPanel.decide`）；
  - 反馈：`app.notify(...)` + `app._write_chat("system", ...)`；
    结果文案按回执：CANCELLED→"已取消"、CANCEL_REQUESTED→"已请求取消（活动回合结束后生效）"、
    终态→"已处于终态（X），无需取消"、失败→"取消任务失败：<error>"。
- **B-06 支撑**：`LogPanel.replay_from`（panels.py:287-291）清空后重放。

### 验收结果

新增 `tests/test_p6_tui_cancel_refresh.py`（5 用例，全过）：

```bash
.venv/bin/python -m pytest tests/test_p6_tui_cancel_refresh.py -q
# 5 passed
```

覆盖：BLOCKED 任务选中行按 `c` → store 变 CANCELLED + 界面反馈；
RUNNING+活动回合 → CANCEL_REQUESTED；终态 → 提示无操作；
周期刷新后状态栏非空且随状态变化、面板行内容随 store 更新。

复跑 `review/tmp/repro_surface.py`（修复后）：

```
[B-02] query_one('#status', StatusBar) -> OK
[B-02] status text: ' 会话 s1  |  ACTIVE  |  权限 approved_scope  |  Leader Leader(test)  |  活动回合 0  |  未完成任务 0  |  待批准 0'
[B-02] LogPanel.refresh_from called by periodic refresh: 4
[B-06] log-stream lines after opening log tab: 1200（600 事件，RichLog 折行后 visual strips）
```

（1200 行不是重复渲染：RichLog.lines 是折行后的 Strip，探针显示每个事件首行+续行成对出现。）

## B-07 显示侧修复（task_83eabc6071f2）

问题：`SessionsPanel` 以 `key=info.session_id` 加行；当同一 id 同时存在于活动与归档
（历史遗留，根因由 impl_sessions 在 sessions.py 修）时 DataTable 抛 `DuplicateKey`。

改动 `src/teamagents/tui/panels.py`：
- `_row_sessions: dict[str, str]`（panels.py:377）保存 row key → 真实 session_id；
- `refresh_from`（panels.py:400-420）：**仅在 id 冲突时为后来行加 `#archived`/`#active` 后缀**，
  唯一 id 仍保持原来的裸 id 作为行 key（向后兼容既有测试与调用方）；
- `selected_session()`（panels.py:425-433）经映射返回真实 id（无映射时回退裸 key），
  `s/n/a/d` 动作仍作用于正确会话。

说明：Leader 建议的统一后缀 `f"{id}:{'active'|'archived'}"` 改为「必要时才装饰」，
以避免破坏 `tests/test_p6_sessions_ui.py` 里把行 key 当 session id 用的既有断言
（`rows.index("two")` 等）——语义等价、改动面更小；如需统一格式请示下。

新增 `tests/test_p6_sessions_ui_dupkey.py`（1 用例）：

```bash
.venv/bin/python -m pytest tests/test_p6_sessions_ui_dupkey.py tests/test_p6_sessions_ui.py \
  tests/test_p6_tui.py tests/test_p6_tui_cancel_refresh.py -q
# 20 passed
```

覆盖：真实 `open_session` 打开同 id → 归档 → 再次以同 id 打开 → 面板列出 2 行、
行 key 唯一、两行 `selected_session()` 都解析回 "dup"、周期刷新无错误。

## 全套件回归

```bash
.venv/bin/python -m pytest tests/ -q
# 110 passed, 2 failed, 12 deselected in 43.03s
# failed 均为基线环境失败：
#   test_config_cli.py::test_xhigh_maps_to_max_for_models_without_xhigh（缺 DEEPSEEK_API_KEY）
#   test_p3_web_tools.py::test_ssrf_guard_blocks_internal_targets（无 DNS）
```

无新失败（基线 109 passed → 110 passed，+1 为本任务新增用例）。

## 遗留

1. **任务「暂停」语义未定义**：计划 §13 提到任务暂停/取消，但产品模型（TaskStatus）没有
   PAUSED 任务态，`ActionKind` 也无暂停任务动作；未发明该功能。用户侧现状：取消任务 =
   BLOCKED→CANCELLED 或 RUNNING→CANCEL_REQUESTED（后者真正中断依赖 RT-03 批次 2）。
2. **B-05 剩余两条**：LogPanel 单次读取上限 500 条（现在周期刷新会自动续读，激活时先
   重放前 500、后续 tick 追加剩余）；`Ctrl+R` 仍重置聊天游标并重放全部历史（未改，避免
   扩大范围）。日志高水位污染聊天游标的根因已修。
3. **命令 `c` 与 DataTable 默认键无冲突**；SessionsPanel 的 `#` 后缀格式仅在 id 冲突时出现。
4. 归档行与活动行同 id 时，「当前」标记会同时出现在两行（显示层小瑕疵，根因修复后进入
   该状态的唯一途径是历史遗留数据）；未扩大范围处理。
