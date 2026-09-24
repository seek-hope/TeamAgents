# 共享附件与任务成果引用校验（2026-09-19）

基准：实施方案 §5.2「成员私有上下文引用不能作为普通共享附件发布」、
§6.2 的完成申请/结果合约，以及 §7 的 `complete_task` 输出引用检查。
这是既定合约的缺陷修复，保留普通工作文件、共享制品与外部证据引用，无新增依赖或数据库结构变更。

## 已复现的问题

1. `publish_shared` 只检查 content/ref 是否为真值，私有 `ctx:`、已登记的外部线程 ID、
   历史文件路径均可发布；对象/数值也会被受理，然后在读取字符串时丢失。
2. `complete_task` 的数组读取静默丢弃非字符串元素，私有引用能进入完成申请并传播为成功结果。
3. 回合结算不重新检查旧申请；升级前的私有引用，或提交后被换为私有历史符号链接的成果，仍能成功。
4. `completion_request` 将损坏的 JSON 当作空数组，掩盖持久数据错误并继续结算。

首轮四项回归在生产修复前均失败，日志 `/tmp/teamagents-output-refs-red-verified-20260919.log`。
最初缺少测试 import 的编译失败不计为缺陷证据。
扩展路径检查还复现了两类校验错误：

- 先消除 `..` 会抹去制品目录中符号链接的真实目标：
  `/artifacts/alias/../chat_tree.json` 被错误受理。
- 只检查去掉空格/片段/行号的字符串，会漏掉原始文件名对应的私有符号链接。
  检查现在保留原始拼写，同时验证支持的片段与行号引用。

对应失败日志是 `/tmp/teamagents-output-refs-expanded-20260919.log`、
`/tmp/teamagents-output-refs-spelling-red-20260919.log`。

## 修复后的合约

`core/src/references.rs` 由 `Control::submit` 的事务路径调用；仅在原有身份、任务状态和共享空间权限检查通过后验证引用。

- `ref` 为非空字符串，`result_refs` 为完整字符串数组；混合类型整项拒绝，不截短数组。
  省略/null 保留无引用语义，结果空数组仍合法。被拒绝的更新不覆盖之前有效的完成申请。
- 拒绝 `ctx:`、`codex:`，以及数据库登记的 context、外部 thread/turn 标识。
  本地引用检查成员历史、自动输出、数据库及旁路文件、配置和 Codex 私有根，也覆盖同一状态根中的其他/归档会话。
- 相对路径按成员工作目录解析；`/artifacts/` 与 `artifacts/` 指向会话共享制品。
  工作文件与 `members/<成员>/work/` 成果保留可引用性。旧 `artifacts/exec-*.log` 无可靠归属，仍拒绝作为成果。
- 同时检查路径拼写与符号链接的真实目标；保留 `alias/..` 的实际解析顺序，
  不存在的成果通过现有祖先检查。支持本机 `file:` URI、百分号编码、片段与 `:line[:column]`；
  非本机 file authority、坏编码和控制字符返回错误。
- 外部 URL/制品 URI 保留，不在 SQLite 事务中发网络请求或读取文件内容。

任务正常结束时重新验证完成申请，包含旧版本持久申请和符号链接被更换的情况。
非法申请保留在数据库供审计，不形成 `task_completed` 或任务成功结果；
产生不含私有引用原文的进度原因，当前附属任务走既有 `BLOCKED` 路径。
回合仍为 `COMPLETED`，表示执行结束，不代表任务验收成功。
损坏 JSON 或基础数据读取失败使结算返回错误，事件写入失败也整体回滚，输入不被提前确认。
重复结算不会重复产生事件。工具说明同步提示模型先生成可共享成果。

## 可复跑证据

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test output_references
cargo test --offline --locked --manifest-path engine/Cargo.toml --test private_context -- --nocapture
make check
```

核心五项回归：

| 测试 | 证据 |
|---|---|
| `shared_references_reject_private_handles_paths_and_invalid_types_without_publishing` | 私有标识/路径/别名及无效类型拒绝，无共享条目、事件或投递；失败回执幂等；九种正常成果引用通过 |
| `task_result_references_validate_the_whole_array_and_keep_the_last_valid_request` | 私有 context/turn 引用、混合数组及错误类型拒绝，保留上次合法申请，正常结束得到原成果 |
| `pending_private_or_retargeted_references_block_completion_without_publishing` | 旧申请和提交后替换的符号链接均阻止任务成功，保留申请与错误原因，重复结算仅一次 |
| `reference_validation_storage_failure_does_not_settle_or_acknowledge_the_run` | 损坏持久 JSON 导致错误，run/task 保持 RUNNING，输入未确认 |
| `rejected_reference_settlement_rolls_back_its_status_events_and_deliveries_together` | SQLite 触发器注入 `task_blocked` 写入失败，状态、事件、输入确认一起回滚；恢复存储后正常 BLOCKED |

Engine 新增 `private_output_references_are_refused_and_corrected_through_model_tools_in_both_modes`：
生产 `open_session`、本地 HTTP 模型夹具和真实团队工具调用；普通/全自动两种模式均拒绝私有引用。
第二轮模型响应读取错误后写入共享文件、发布并完成任务，B/C/Leader 能读到成果；
它们的实际请求中没有被拒引用、私有文件内容或被拒条目的正文/摘要。
此项不用真实供应商，不以模型质量或真实服务兼容性验收计数。

`make check` 全部通过：格式、全部目标 Clippy（`-D warnings`）、三个 crate 测试及仓库卫生检查。

| crate | Cargo 报告通过 |
|---|---:|
| core | 84 |
| engine | 296 |
| tui | 104 |

Engine 另有 1 项显式 ignored；未开启 `live_codex` 的提前返回不算真实服务验收。
本轮未修改 TUI 交互，也未重跑 PTY；同日此前的三项终端证据见私有上下文隔离记录。

通过日志：

```text
/tmp/teamagents-output-refs-focused-20260919.log
/tmp/teamagents-output-refs-wire-20260919.log
/tmp/teamagents-output-refs-check-20260919.log
```

## 范围

这是发布/结算时的引用合约，不授予接收者文件权限，也不证明文件内容满足验收条件。
不会扫描任意自然语言或外部 URL 内容，不会重写既有共享条目。
完成发布后的宿主并发替换、硬链接、用户主动复制私有内容及恶意同用户进程不在本项保证范围。
五家真实模型矩阵、复杂仓库任务与同条件竞品比较仍待完成。
