# 模型协议审查与修复（2026-09-17）

范围：`engine/src/chat.rs`、`engine/src/stream.rs` 与真实 ChatRunner + 本地 HTTP 服务回归。依据方案 §11、T7、T22 与 D-32；不改变架构、权限或默认模型。

## P1：响应被截断后仍执行工具

修复前，Chat Completions 忽略 `finish_reason=length/content_filter`，Anthropic 忽略 `stop_reason=max_tokens`，Responses 把 `response.incomplete` 和 `response.completed` 归到同一个成功分支。JSON 回包也没有对应状态校验。即使工具参数恰好能解析成 JSON，也不能据此认定该次生成完整。

新增 `incomplete_model_responses_never_execute_tools`：用三种协议的 JSON/SSE 回包各生成一个语法完整的 Shell 调用，但终止原因为长度限制。修复前测试失败于 `openai streaming=false executed an incomplete response`；修复后六种组合均进入 FAILED，执行器零调用，HTTP 仅调用一次，不自动重放。

修复在协议归一化前拒绝明确的截断、未完成和错误状态。Responses/Anthropic 不接受 Chat Completions 的 `[DONE]` 代替自身终结事件。Responses 完成事件后立即返回，避免等待保持连接的服务再关闭传输。兼容未返回可选终止字段的旧端点，但不把显式失败改为成功。用户可以根据错误调整输出预算后继续；本次不新增自动续写策略。

## P1：Responses 工具续接丢失推理项

请求使用 `store:false`，但归一化只保留普通文本与函数名称/参数，丢弃 `reasoning.encrypted_content`、原始 item ID 与顺序。下一次请求无法完整续接前一次推理。

修复将 Responses 原始 output 保存在成员私有 assistant 历史中，续接时按原顺序回传；不把它渲染为用户回复。仍支持原有历史格式，切换到 Chat Completions 时移除该内部字段（及 Anthropic 专用字段），不向其它协议发送私有适配字段。

新增 `responses_reasoning_survives_tool_continuation`：修复前报 `the reasoning item must survive unchanged`；修复后验证不透明推理内容、函数 item ID、结果配对与顺序均保留，没有重复函数调用。

复核补充：该用例同时覆盖 JSON 和 SSE（含从 `output_item.done` 重建输出的路径）。新增 `responses_fallback_items_must_also_be_complete`，验证备用输出也不能包含未完成工具；`switching_protocols_does_not_send_internal_history_fields` 验证切换协议后不泄露内部适配字段。

官方接口依据（本轮直接抓取）：<https://developers.openai.com/api/docs/guides/reasoning>，其中说明无状态调用的加密推理回传、保留工具调用前后完整 output，以及 `incomplete/max_output_tokens` 的含义。此处依据协议要求，不代表真实 OpenAI 服务已经验收。

## 可复跑验证

```bash
cargo test --offline --manifest-path engine/Cargo.toml --lib stream::tests
cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e incomplete_model_responses_never_execute_tools
cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e responses_reasoning_survives_tool_continuation
```

修复前后日志在 `/tmp/teamagents-{incomplete,reasoning}-{before,after}.log`。长期证据为上述仓库回归测试；本地假服务不计入真实服务兼容性成绩。
