# plain 故障反馈与已返回终态的取消、恢复

本批延续 D-32/D-35 和方案 §9，处理行模式输入反馈、Chat 已返回结果在取消和冷恢复中的一致性。
生产改动位于 `engine/src/{cli,runtime,chat}.rs`，保持 core 单事务权威、数据库格式、权限边界和停止确认规则。
测试使用本地 HTTP 模型夹具、实际 CLI/serve 子进程与 SIGKILL；未调用真实供应商。

## 复现与修复

### 1. plain 将拒绝回执显示为已接收

SQLite trigger 拒绝插入 `user_message` 后，核心返回 `Receipt.ok=false`，旧 plain 仍显示
`[input received: ]`。新增子进程检查先失败，拒绝没有生成回合或受理事件。
现在只有成功回执显示“输入已接收”，拒绝带出原因并回到命令输入。
故障解除后在同一进程提交新消息，实际模型请求不包含先前被拒绝的输入。

### 2. plain 存储等待阻塞后续命令

成员视图准备失败时，旧 plain 调用 `settle(600)`，等待逻辑把读取错误继续当作忙碌，
没有向用户显示存储原因或及时处理下一条命令。新检查在五秒观察窗内超时，
输出只有会话行与输入已接收，日志 `/tmp/teamagents-repl-before-20260919.log`。

Runtime 提取单次 `is_settled` 观察，向交互调用者传出读取错误；原 `settle` 保持原来的限时等待语义。
plain 检查当前运行时诊断或状态读取错误后，提示原因并返回命令输入，后台继续重试。
`status` 除用量外显示新增事件与当前等待原因，故障解除后可看到原回复，无需重复发送工作。
普通等待超过十分钟会明确提示超时，不伪报已完成，也不取消后台工作。

### 3. Chat 已返回终态被迟到取消覆盖

模型先完成文件写入，再返回成功或明确 HTTP 失败；坏任务字段阻止运行时读取状态，
SQLite trigger 同时阻止结果归档。等待终态检查点落盘后提交取消，再解除故障。
修复前，成功结果变为 CANCELLED，最终回复没有归档；日志
`/tmp/teamagents-returned-cancel-before-20260919.log`。

`ChatRunner::request_interrupt` 现在先保留已有终态，并在等待工具退出后再次核对。
这与既有 Codex 中断接口保留自然终结状态的处理一致。运行时收到相同的确认状态时保留整个原结果，
不额外给完成结果加上“被取消”的说明。仍在执行的回合继续请求停止；未确认结果不改判为成功。

### 4. 冷恢复重新排队时丢弃已有结果

只修复进程内中断后，SIGKILL 重开组合继续失败：旧 Chat 恢复对所有可读检查点都返回 QUEUED，
核心随后看到旧取消标记，将已有终态的回合在排队阶段结清，丢掉待归档的完成结果。
日志 `/tmp/teamagents-returned-cancel-fix-20260919.log` 保留这一后续复现。

现在 Chat 恢复遇到可确认的终态检查点时，先按原规则核对历史 epoch、恢复未完成的历史提交日志，
再直接返回原终态与回复/错误供核心归档。执行恢复与终态核对共用同一个历史恢复方法，
不跳过日志修复，不请求模型或重放执行工具。显式 rewind 仍按原 epoch 规则使旧检查点失效；
损坏历史、未确认外部副作用与子代理未知结果继续停止核对，不伪造终态。

## 行为证据

新增四项 engine 回归：

| 检查 | 证明范围 |
|---|---|
| `recovery::plain_rejects_uncommitted_input_and_accepts_a_new_request_after_repair` | 实际 plain 进程显示拒绝原因，核心无受理记录、模型无请求；修复后同进程新输入正常完成，实际请求不混入拒绝消息 |
| `recovery::plain_storage_failure_returns_control_and_recovers_without_resubmitting_input` | 成员视图错误/整体状态读取错误两类；五秒内可继续执行 `status`，恢复后只处理一次原输入，`status` 能展示已归档原回复 |
| `recovery::returned_chat_outcome_survives_cancellation_during_read_failure_and_restart` | 成功/明确失败 × 进程内恢复/实际 SIGKILL；已受理取消不能覆盖原终态、回复或错误，无新模型请求、文件副作用不重做 |
| `recovery::returned_chat_outcome_survives_cancellation_while_waiting_to_finalize` | 成功/明确失败已进入归档重试队列后才取消，分别解除故障或 SIGKILL 重开，仍只归档同一个原结果 |

另扩展 `chat_e2e::review_tree_commit_recovers_on_both_sides_of_rename`：
执行恢复和直接 reconcile 两条路径均覆盖历史树替换前/后两个窗口，恢复后的树与原树逐值相等，
没有重复节点，模型调用数保持一次。`review_completed_checkpoint_restores_reply_without_another_model_call`
改为要求 reconcile 直接返回 COMPLETED 和原回复，保留后续执行恢复也不再次调用模型的断言；
原来要求先返回 QUEUED 的断言已随修复更新。

归档重试期间取消的组合是本批补充验证，不把原已正常保留结果的路径算作新修复。
已有运行中取消、批准、未知副作用、历史损坏、超时与 Codex 恢复用例随完整检查继续通过。

## 最终验证

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery --test chat_e2e
make fmt
make check
make pty
bash review/eval/check-runner.sh
```

最终完整检查中 `chat_e2e` **59** 项、`recovery` **22** 项通过。
`make check` 的格式、全目标严格 Clippy、回归与仓库卫生检查全部通过：
**core 145 / engine 362 / TUI 105，共 612 项**；engine 仍有三项显式 ignored。
三项 PTY 通过；评测入口 **30 项**无模型契约通过，没有新增真实模型成绩。

日志：

- `/tmp/teamagents-repl-before-20260919.log`
- `/tmp/teamagents-returned-cancel-before-20260919.log`
- `/tmp/teamagents-repl-fix-20260919.log`
- `/tmp/teamagents-returned-cancel-fix-20260919.log`
- `/tmp/teamagents-returned-cancel-fix2-20260919.log`
- `/tmp/teamagents-returned-cancel-finalize-20260919.log`
- `/tmp/teamagents-repl-finalization-focused-20260919.log`
- `/tmp/teamagents-repl-finalization-check-20260919.log`
- `/tmp/teamagents-repl-finalization-pty-20260919.log`
- `/tmp/teamagents-repl-finalization-runner-20260919.log`

## 保留边界

后续[通知与旧排队恢复批次](recovery-state-2026-09-19.md)补齐了以下挂起归档通知和旧 QUEUED
终态检查点的专属检查；本报告的原始范围与测试数量保持不变。

- 本批终态取消证据针对后端已有可确认结果、核心尚未归档的回合；尚未返回终态的取消、
  挂起结果归档与通知时序、取消确认超时仍需更完整的故障组合覆盖。
- 冷恢复的生产检查从仍为 RUNNING 的业务记录恢复。旧版若已将终态检查点对应回合写成 QUEUED，
  再留下取消标记，这一历史中间状态未在本批单独验证。
- plain 正常回合仍使用行模式等待；没有新增执行中的交互式批准/取消命令或后台守护进程。
  存储持续损坏时不能继续归档，错误提示和重试不等于自动修复数据库。
- 没有新增真实供应商、远端工具、真实模型 TUI、陌生仓库或发行制品验收。
  T1–T24 仍为十八项有路径证据、六项部分覆盖；总体成熟度目标保持进行中。
