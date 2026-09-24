# 模型响应传输恢复（2026-09-17）

## 范围与发现

本轮在 D-32 / D-35 授权内修复模型响应读取阶段的瞬时故障恢复，遵守 D-36：不调用真实模型，
本报告仅记录本地假服务的确定性检查，不替代原生上下文真实评测。

`ChatRunner::chat`、`chat_responses`、`chat_anthropic` 对 HTTP 请求建立阶段执行 `max_retries`，
但收到响应头后的 `stream::response(...)?` 直接向外返回错误。读超时或连接中断因此绕过
已配置的重试，即使本次响应中的任何工具都尚未执行。重试退避使用不可中断的 `sleep`，
收到最长 30 秒的 `Retry-After` 后，取消仍需要等待整个退避结束。

真实评测运行 2（`eval/runs/2026-09-17-rust-ledger/r2.jsonl`）即因此损失约 124 秒：
成员回合在 226.5 秒时以 `ChatError: model stream: timed out reading response` 失败，
此前该响应未执行任何工具。

## 实施

- `stream.rs` 新增 `StreamError` 分类：`Interrupted`（取消，原样传播）、
  `Transport`（I/O 失败或流在终结事件前结束，附带是否已有文本送出）、`Protocol`
  （响应违反协议或端点报错）。`decode` 用 `Cell` 跟踪文本送出，借出标记兼作重试安全判据。
- 三个协议的重试循环对 `Transport` 且未送出任何文本的失败按 `max_retries` 重试：
  未送出文本时流里至多只有部分工具参数，而部分参数从不进入执行器，重试无副作用；
  已送出文本或协议错误保持立即失败，不重复展示输出、不重放供应商错误。
- 退避改为 `interruptible_backoff`（50 秒粒度轮询取消），取消在最长 30 秒的
  `Retry-After` 等待中立即生效。

## 验证

- `stream.rs` 单测：仅含工具的截断流判为 `Transport(_, false)`；送出文本后的截断判为
  `Transport(_, true)`；取消判为 `Interrupted`。
- `chat.rs` 单测 `backoff_waits_in_full_but_stops_on_cancel`：退避走满 120ms；
  已取消的控制在 30 秒退避下立即返回。
- 端到端 `chat_e2e.rs`：
  - `truncated_tool_stream_retries_and_executes_once`：三种协议各自首响应截断、
    重试取得完整工具调用、工具恰好执行一次、回合完成，请求数 = 截断 1 + 重试 1 + 收尾 1。
  - `stream_retry_respects_budget_and_visible_output`：`max_retries=0` 不重试；
    `max_retries=1` 恰好两次尝试后失败；已送出文本的截断不重试。三种情形部分
    工具参数均未执行。
- 既有 `retry_policy_only_retries_transient_errors` 与 `incomplete_model_responses_never_execute_tools`
  继续通过，状态码语义与"不完整响应不执行"未改变。
- `cargo test --offline`：engine **212 通过 / 0 失败 / 1 ignored**（ignored 为隐藏评分入口），
  core 63 / tui 91 不变。

## 边界

模型请求重试可能产生额外服务用量；中断响应没有完整用量信息时不能把缺失用量当作零消耗证明。
本轮不改变模型上下文长度、真实评测任务、原生工具操作去重或后台终端协议。
读超时已送出文本的响应仍判失败：重复展示文本不可取，且此时无法区分端点是否已计费完整响应。
