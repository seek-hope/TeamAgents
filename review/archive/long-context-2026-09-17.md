# 长任务上下文与恢复审查（2026-09-17）

范围：`engine/src/chat.rs`、`engine/tests/chat_e2e.rs`。依据方案 §9–11、D-28、D-32；保留进入本轮时已有的协议适配改动。使用本地 Rust 假 HTTP 服务，不调用付费模型。

## 已复现

命令：`cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e long_context -- --test-threads=1`

首次运行 **0 通过 / 3 失败**：

1. `long_context_compaction_preserves_request_and_reads_large_output_after_restart`：工具输出超过压缩尾部阈值后，`keep_from` 被移到最新 assistant，最近用户要求随即被摘要覆盖。假摘要故意未复述中文约束，下一次真实工作请求丢失原文。
2. `long_context_summary_calls_consume_the_persisted_model_step_budget`：配置每回合 2 步，实际产生 3 次模型请求，因为摘要请求未计步。
3. `long_context_overflow_cannot_start_recovery_after_budget_exhaustion`：配置 1 步，第 1 次请求返回上下文溢出后，仍发起第 2 次摘要请求才报告耗尽。

另外，`cargo test --offline --manifest-path engine/Cargo.toml --lib repeated_compaction_indexes -- --exact chat::tests::repeated_compaction_indexes_each_retained_tool_call_once` 首次运行 **0 通过 / 1 失败**：同一已保留工具组连续压缩 10 次后，原本唯一的工具输出指针在索引中重复 11 次。

## 已修复

- 最近用户输入独立保留，不再因工具循环变长而被丢弃。近期调用组按新到旧的顺序选择，完整保留能放入尾部预算的组，再按原时序发送；超大组被摘要覆盖，其完整原文仍在私有树中，由 `read_history` 分页读回，避免刚压缩便把同一大输出重新发给服务。
- 当前分支工具索引按调用 ID 去重，连续压缩保留同一组时不再重复堆积索引行。
- 每次逻辑模型调用前统一检查并持久化步数，包括摘要、上下文溢出恢复与参数兼容回退。传输层既有 HTTP 重试策略不变。摘要失败仍消耗已经发出的调用额度，限制与检查点错误不会被摘要降级路径吞掉。

## 验证

新增 4 项假 HTTP 服务端到端回归、1 项树索引单测。前三项端到端与索引单测均先在修复前复现失败，修复后通过。额外正向恢复测试模拟供应商因 60,000 字符输出拒绝请求，验证「工具调用 → 溢出 → 摘要 → 成功续接」严格为 4 次模型调用。

`long_context_compaction_preserves_request_and_reads_large_output_after_restart` 覆盖两次连续压缩，故意让摘要不包含用户中文约束，仍检查后续工作请求保留原文；重建 runner 后分页读回 60,000 字符原始输出中间的标记，Shell 只执行 1 次。

- `cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e`：**35 通过 / 0 失败**（20.40 秒），包括既有批准、取消、流式、崩溃窗口与迟到摘要检查。
- `cargo test --offline --manifest-path engine/Cargo.toml --lib`：**96 通过 / 0 失败**（11.09 秒）。
- `git diff --check -- engine/src/chat.rs engine/tests/chat_e2e.rs review/long-context-2026-09-17.md`：通过。

边界：超大单条用户输入本身、工具索引本身超过模型上下文、摘要对较早语义约束的保真度，以及多语言 token 估计精度仍须独立评估。本轮没有改变摘要头尾截断策略、引入新检索接口或声称通过真实长任务模型对照评测。

## 大推理文本的补充回归

最近 assistant 的推理文本达到 20,000 字符、整个最近调用组超过尾部预算时，原实现只保留用户输入，连前面较小的源码读取结果也一并丢弃。该信息丢失由本地假服务独立复现，不作为真实模型能力测量。

新增 `long_context_keeps_small_recent_groups_when_the_latest_reasoning_is_oversized`：先读取源码标记，再生成携带 20,000 字符推理的另一调用；压缩摘要故意不含源码标记。修复前下一请求丢失源码，测试失败；修复后保留用户原文与前一完整读取组，跳过超大组，原始执行仍仅一次。`cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e long_context -- --nocapture` 五项通过。

总新增为 5 项端到端回归与 1 项索引单测。使用原生上下文长度的真实服务验收与任务完成口径见 [复杂编码记录](complex-coding-2026-09-17.md)。
