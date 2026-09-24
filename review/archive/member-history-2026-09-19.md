# 成员持久记录浏览（2026-09-19）

本文保留 2026-09-19 的首批范围与验证。2026-09-20 已增加按需读取 Codex 原生对话与工具记录，
覆盖原生分页及受限 JSONL 回退；当前行为、测试计数和边界见[后续记录](native-history-2026-09-20.md)。

## 范围与产品边界

本批落实用户只读查看成员持久记录的入口，覆盖当前成员和已经移除的成员。目标是让用户能够在不
启动后端、不改变团队状态的前提下，核对本地已经落盘的成员对话树、线性快照、回合检查点和
与成员明确相关的团队事件；不把成员私有内容注入 Leader，也不把日志摘要冒充完整对话。

这不是把 TeamAgents 变成 Codex 历史存储。Codex 的完整外部对话、工具历史和内部推理仍由 Codex
自身保存；本地没有完整副本时，界面显示 warning，不伪造正文。方案 §13 原本将完整成员对话/工具
浏览列为目标能力，本批完成的是有明确来源和上限的本地持久记录浏览，保留上述外部历史边界。

## 实现

- `engine/src/history.rs` 增加只读 `history` worker 协议：使用独立的 SQLite `query_only` 连接和
  受限文件打开，不调用 runner、模型工具或核心写入入口。
- 来源分为 `chat_tree.json`、`chat_history.json`、`turn_runs`/检查点和 `events`。旧会话没有树文件时
  只在内存中生成线性树形视图，不触发惰性迁移。
- 对话/检查点按文件 SHA-256 做版本校验；数据库分页固定新增记录上界。事件关联只检查 actor、受众
  和明确的成员字段，不因普通文本恰好出现成员 ID 而误收。
- 文件读取拒绝符号链接、非普通文件、损坏 JSON 和超过 32 MiB 的文件/单条事件；读取有取消信号和
  5 秒查询上限，避免历史浏览长期占用 worker。
- `tui/src/history.rs` 提供成员→来源→详情导航、分页、刷新、说明、返回、窄屏换行和 generation
  防迟到响应。入口为 `/history`，团队/日志页签选中成员后按 `h` 可直接进入。
- 已移除成员仍保留在 `agent_runtime` 的记录列表中；浏览只读已保存成果，不恢复成员运行器、不重启
  Codex/MCP，也不确认投递或修改任务/批准状态。

## 可复跑证据

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test history_protocol
cargo test --offline --locked --manifest-path tui/Cargo.toml --test history_tests
make check
make pty
```

### Engine 协议回归（3 项）

| 测试 | 断言 |
|---|---|
| `history_is_user_only_paged_and_keeps_removed_member_records` | 浏览对话树、检查点和已移除成员；记录中的工具结果可读；读取前后运行/事件状态不变；文件变化拒绝旧分页版本 |
| `history_rejects_symlinked_records_and_never_reads_outside_session` | 符号链接记录被拒绝，外部文件内容不泄露 |
| `history_event_pages_stop_at_related_rows_and_use_schema_fields` | 相关事件分页恰好收满 40 条后以实际 sequence 续页；普通文本命中不构成成员关联；详情按 sequence 读取 |

### TUI 回归（5 项）

| 测试 | 断言 |
|---|---|
| `slash_team_and_log_shortcuts_open_the_expected_member_history` | `/history`、团队/日志 `h` 生成正确请求 |
| `member_source_and_detail_navigation_preserves_parent_pages` | 成员→来源→详情导航可返回且保留父页 |
| `stale_generation_and_closed_views_ignore_late_history_replies` | 关闭、切换或刷新后的迟到响应不会污染新视图 |
| `missing_codex_history_is_shown_as_a_warning_not_fabricated_content` | Codex 缺少完整本地记录时显示明确 warning，不生成假对话 |
| `pagination_refresh_info_and_narrow_frames_are_safe` | 分页、刷新、说明、窄屏和长正文换行不崩溃 |

`make check` 本轮通过：core **63**、engine **271**（其中 `eval_grader` 1 项显式 ignored）、TUI
**104**。`make pty` 通过输入/恢复、鼠标命中和工作区审查三项无模型真终端检查；这些 PTY 检查验证
整体 TUI 入口和响应，不替代成员记录协议的专项测试。

## 失败到通过与安全检查

- 初始事件扫描按固定 offset 续页会在无关事件交错时跳过相关事件；改为返回扫描到的实际 sequence，
  并在收满页面时停止，专项分页测试固定了该行为。
- 初始关联若递归搜索所有字符串，会把 payload 普通文本中的成员 ID 当成关联；现改为显式 schema
  字段匹配，专项测试保留反例。
- 历史读取不复用写连接、不进入模型上下文、不走成员工具层；因此不会触发工具、副作用、投递确认或
  Leader 可见性变化。
- 文件和数据库页面分别校验版本/上界；跨文件并非原子快照，读取期间外部修改可能要求用户刷新。

## 已知边界

- 只显示已经落盘的本地记录；流式但尚未写入检查点/对话树的文本不可用。
- Codex 的外部历史和内部推理不由 TeamAgents 接管；TeamAgents 只显示本地保存的团队事件、回合元数据
  和可用检查点。
- 单文件或单条事件上限为 32 MiB；历史页不是归档会话浏览入口，当前仅能从已打开会话查看其记录。
- 数据库、对话树、线性快照和检查点之间没有跨文件原子快照保证；刷新是用户确认当前内容的方式。
- 该功能是用户界面能力，不授予 Leader、成员或观察者新的读取权限；模型仍不能通过它读取其他成员私有历史。
