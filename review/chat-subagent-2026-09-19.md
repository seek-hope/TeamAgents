# Chat 成员私有子代理（2026-09-19）

## 范围

本批实现方案 §5.1、§6.4、§10.1 所定义的 Chat 成员内部私有辅助过程。它不是 TeamSpec 成员、不会创建独立任务或线程，也不会把私有 transcript 暴露给 Leader 以外的团队状态。没有调用真实模型服务，未扩大供应商验收范围。

## 实现

- `engine/src/chat.rs` 在 Chat 成员工具集中增加 `run_subagent(task, context)`；输入分别限制为 12,000 和 16,000 个 Unicode 字符。
- 辅助只看到显式任务上下文和自己的嵌套 transcript；没有父成员历史、TeamSpec 身份、团队工具、`update_plan` 或递归 `run_subagent`。
- 辅助继承父成员的工作目录、已绑定执行工具、权限/批准闸门和 `TurnControl`。辅助模型请求调用 `reserve_model_step`，计入父回合 `max_model_steps_per_turn`。
- 辅助最终文本或模型失败均作为父工具结果返回；父模型继续处理结果并负责团队动作。
- 辅助工具批准可以暂停父回合。恢复时不插入会破坏 provider assistant-tool-result 顺序的 user 消息；批准后的子调用只执行一次。
- 外部工具回合使用两阶段检查点：执行前写 `pending_tool`，执行完成后先写 `tool_receipt`；只有嵌套 `tool` 消息和 marker 清理一起落盘后才继续。无 receipt 的 in-flight 状态进入 `OUTCOME_UNKNOWN`，有 receipt 则恢复时补写结果而不重执行外部副作用。

## 可复跑证据

```bash
cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e private_subagent
cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e private_subagent_receipt_recovers_without_replaying_the_child_tool
cargo test --offline --manifest-path engine/Cargo.toml
```

当前本地结果：`chat_e2e` 的 5 项私有子代理回归通过；完整 engine 测试通过（100 库单测、1 CLI 单测、177 集成测试，另有 1 项显式 ignored）。

覆盖用例：

| 测试 | 断言 |
|---|---|
| `private_subagent_uses_parent_bindings_without_team_identity_or_history` | 只继承父执行工具；父历史不泄露；无团队工具、计划工具和递归能力 |
| `private_subagent_approval_resumes_nested_tool_once_and_keeps_protocol_order` | 子工具批准挂起后恢复，外部工具恰好一次，assistant/tool 顺序合法 |
| `private_subagent_model_failure_is_returned_to_parent_and_parent_can_continue` | 辅助模型失败变成父模型可见的失败结果，父回合仍可完成 |
| `private_subagent_model_steps_share_the_parent_turn_budget` | 辅助模型请求消耗父回合预算，不产生额外额度 |
| `private_subagent_receipt_recovers_without_replaying_the_child_tool` | 模拟执行后、嵌套结果写入前崩溃；恢复只补写 receipt，不重执行子工具 |

此外，core 与 TUI 离线测试也通过。此记录不代表 OpenAI、Anthropic、Kimi、DeepSeek、GLM 等真实服务验收。

## 边界与后续

- 这不是 Codex 外部成员的子代理适配；Codex 仍按自身 app-server 协议运行。
- 无 receipt 的外部调用不能证明副作用是否已发生，系统保持 `OUTCOME_UNKNOWN`，需要用户/Leader 按既有恢复流程处理。
- 当前 checkpoint 与成员历史仍沿用仓库既有 JSON 持久化和配额边界；未在本批新增磁盘配额或真实模型评测。
