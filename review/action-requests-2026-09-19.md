# 动作请求校验与共享空间读取

本批继续方案 §5.2、§7、§9 和 D-32：补齐受支持团队动作的外层参数校验，
修复共享空间分页与统计，并验证生产 ChatRunner 和 `serve` 的拒绝、修正及重开路径。
使用本地模型协议夹具，不调用真实供应商。没有新增依赖、数据库迁移或架构偏离。

## 先复现的问题

初始八项 core 场景均先在修复前失败，日志：
`/tmp/teamagents-action-requests-before-20260919.log`。

- 消息正文、共享正文、求助内容和完成摘要会把布尔值、数字或对象转换成字符串；
  `signal_done` 的布尔摘要实际变成 `"false"`，完成申请仍获准。
- `user_message` 的布尔正文被接受，进入恢复暂停会话、清除原目标完成申请的路径。
  暂停请求的未知字段被忽略，`{"paused": false}` 仍执行暂停。
- `read_shared` 收到 `space_id: false` 后退回读取所有获准空间，并推进它们的游标。
  这里是错误请求范围扩大，不是越过共享空间 ACL。
- 未指定显式游标时，跨空间读取使用各空间游标的最小值。已单独读取过一个空间后，
  再读全部空间会重复返回该空间的旧条目，占用本页额度。
- `list_shared` 只取前 1000 条再计算数量与最后序列；1002 条实际返回 1000。
- 共享条目的 `supersedes` 未检查引用，缺失 ID 仍能发布；求助任务引用也缺少会话存在性校验。

真实 `serve` 子进程的输入检查也先复现失败：
`supplement: "false"` 被当作默认布尔值接受，错误参数返回成功。
日志：`/tmp/teamagents-worker-input-before-20260919.log`。

## 修复行为

1. 在既有任务/拓扑请求校验之外，为消息、共享发布/读取、求助、目标完成、用户输入、
   批准决策、权限模式和暂停增加严格请求类型，拒绝未知字段及错误类型。
   所有受支持 TeamAction 的 payload 必须是 JSON 对象；不能利用 serde 的结构体序列解码
   将 JSON 数组当作有名字段。权限、身份和终态检查继续保留。
2. 错误请求先形成持久失败回执，不发送消息、不追加共享条目、不改变批准、权限模式或会话状态。
   合法请求可用新调用 ID 修正；同 ID 重放保留原回执，改参数仍明确拒绝。
   `serve user_message` 在构造用户动作前检查文本与布尔 `supplement`，避免预处理丢弃错误值。
3. `read_shared` 的空间、游标与页大小均按声明类型检查。显式游标为非负整数，
   `limit` 为正整数，默认 50；分页错误保留字段名。
   默认读取先取得各获准空间的独立游标，再由一次 SQL 按全局序列排序、限制整页条数。
   显式 `after_sequence: 0` 仍可重读，不改变原有 ACL。
4. 读取和游标推进仍在原 SQLite 事务内。游标读取错误不退回零，
   第二个空间的推进失败会回滚第一个空间的推进；失败回执在修复存储后仍可重放，
   新请求才重新读取。
5. `list_shared` 使用限定会话/空间的 `COUNT(*)` 与 `MAX(sequence)`，返回实际数量和最新序列，
   不加载前 1000 条来代替统计，也不推进已读位置。
6. `supersedes` 必须指向当前会话中成员可访问的条目；仍追加新条目并保留原条目，
   不要求原作者相同，也没有引入同空间限制。求助的可选 `task_id` 必须存在于当前会话。
   省略任务 ID 的正常求助继续可用，不额外要求反向消息通道。
7. Chat 团队工具 schema 同步拒绝额外属性、声明分页范围，并补列已有的 `supersedes` 字段。
   工具说明压缩后为 11893 字符，系统提示 485 字符；原 12000 工具说明预算保持不变。

文档同步纠正了旧用户指南对 stdio MCP 的说明：已实现的默认模式是成员 workspace bubblewrap，
只有显式 host 模式才按宿主权限执行。这是对现有 `BoundTools::load_in` /
`McpClient::connect_stdio_in` 行为的说明修正，本批未改变 MCP 执行策略。

## 可复跑验证

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test action_requests
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e \
  communication_tools_correct_refusals_preserve_shared_pages_and_finish_after_reopen
cargo test --offline --locked --manifest-path engine/Cargo.toml --test worker_protocol \
  worker_user_input_rejects_malformed_flags_without_resuming_the_session
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib \
  prompt_overhead_stays_lean -- --nocapture
make check
make pty
```

新增十二项 core 检查在 `core/tests/action_requests.rs`：

| 范围 | 测试名 |
|---|---|
| 消息/共享/求助拒绝与修正 | `communication_requests_refuse_wrong_types_and_unknown_fields_without_delivery` |
| 错误用户输入与暂停保护 | `malformed_user_input_cannot_resume_a_paused_session_or_clear_completion`、`control_requests_refuse_ignored_options_before_pausing_or_changing_permissions` |
| 完成申请与数组载荷拒绝 | `completion_request_rejects_bad_summary_and_non_object_payloads`、`action_payloads_require_objects_even_when_serde_can_decode_sequences` |
| 读取范围与逐空间分页 | `shared_read_invalid_scope_or_cursor_cannot_consume_other_spaces`、`shared_read_without_explicit_cursor_respects_each_space_and_page_limit` |
| 数量、最新序列与引用 | `shared_listing_reports_exact_counts_and_latest_sequence_beyond_one_thousand`、`shared_replacement_and_help_references_are_checked_in_the_current_session` |
| 三类批准决定 | `approval_decisions_refuse_unknown_fields_before_granting_or_waking` |
| 故障回滚与数据库重开 | `shared_cursor_read_and_write_failures_preserve_progress`、`shared_reads_and_refusals_replay_after_database_reopen` |

批准检查覆盖 once/session/deny，确认未知字段被拒绝时既不写批准缓存，也不唤醒等待回合。
重开检查覆盖成功读取、条件已变化的原失败回执、改参数碰撞、后续新页面和修正请求。
该检查为条目设置可精确表示的固定时间，避免将浮点 JSON 编解码精度差异混入条目/游标断言。

新增两项 engine 检查：

- `chat_e2e::communication_tools_correct_refusals_preserve_shared_pages_and_finish_after_reopen`：
  普通/全自动两模式，经生产会话入口拒绝错误发布 → 正常发布两个空间 → 拒绝错误空间参数
  → 分页读取 → 拒绝错误完成摘要 → 实际写文件 → 正常完成目标。
  逐个核对后续模型请求收到的回执、正确页内容、文件精确内容、唯一目标完成事件与工具 schema；
  重开后已读位置保留、列表各一条、不重问模型。
- `worker_protocol::worker_user_input_rejects_malformed_flags_without_resuming_the_session`：
  真实 `teamagents serve` 子进程，错误标记/未知字段/数组参数不恢复暂停会话，也不记录用户消息；
  合法布尔补充输入仍原样记入事件。

定向和 core 全量通过；日志分别为：

- `/tmp/teamagents-action-requests-core-final-20260919.log`
- `/tmp/teamagents-action-requests-engine-targeted-final-20260919.log`

`make check` 通过格式、严格 Clippy、全部 crate 回归与仓库卫生检查：
**core 127 / engine 348 / TUI 104**，engine 另有三项显式 ignored。
日志：`/tmp/teamagents-action-requests-check-20260919.log`。

`make pty` 三项通过：输入/恢复冒烟、滚动后鼠标命中、工作区审查与恢复基线。
日志：`/tmp/teamagents-action-requests-pty-20260919.log`。

## 保留的边界

- 本批收口的是受支持团队动作的外层参数。提案具体操作仍在 Leader 应用时校验，
  其他管理接口、执行工具参数及历史数据库行没有因此全部严格化。
- 没有新增真实模型、真实远端 MCP 或真实模型 TUI 证据；数据库重开不作为新增 SIGKILL 证据。
- T1–T24 仍为十八项有路径证据、六项部分覆盖；D-30 跨存储边界、真实多供应商矩阵和成熟度对照继续保留。
