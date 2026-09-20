# 大窗口旧工具结果保留预算（2026-09-19）

按 D-28 的可调预算和 D-32/D-35 的长任务改进授权，调整 Chat 的 L1 发送视图。
生产改动仅在 `engine/src/chat.rs`，没有新增依赖或修改持久记录格式。

## 问题与实现

此前真实历史仓库评测发现重复读取同一历史页；保存的上下文表明重复前上一份页已被遮蔽。
这是相关性证据，不能据此认定遮蔽是所有失败的原因。此次先用实际 HTTP 请求回归确认：
配置为 1M 时，连续读取约 20KB 的两份源码和对应历史页，固定 16KB 预算会过早丢失先前结果。
主成员与私有子代理两项回归均先失败于源码不可见断言，再在修复后通过。

旧回执预算现在为 `clamp(context_window / 4, 16_000, 256_000)` 字节，未配置窗口使用 16,000。
原生 1M 配置对应 250,000 字节；这是保守且有上限的启发式，不声称字节与 token 精确等价。
由近到远保留、一份放不下便遮蔽更早回执的顺序沿用原逻辑。最后一个 assistant 之后的最新工具
回执不占该预算。L0 仍只限制发送副本，私有检查点与树保留原文；旧历史页仍指向原来源及页码。

L2 阈值估算、主循环请求、provider 溢出后的恢复请求、私有子代理请求均传入同一模型窗口。
L2 的摘要输入与尾部保留上限没有在本次调整。

## 确定性验证

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e large_window -- --nocapture
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib chat::tests
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e
make check
```

新增四项有行为断言的回归：

- `large_window_keeps_recent_source_and_history_pages_on_the_wire`：主成员的实际请求保留源码与页。
- `private_subagent_large_window_keeps_its_recent_source_and_history_pages`：私有辅助采用相同预算，
  原始工具输出仍不进入父成员上下文。
- `masking_large_windows_keeps_a_bounded_utf8_tail_without_changing_private_history`：UTF-8 按字节计，
  极大窗口仍有上限，最新结果继续受 L0 约束且有完整读回指针，私有记录不被修改。
- `compaction_threshold_includes_the_large_window_tool_budget`：保留的旧结果使下一请求超过阈值时
  触发摘要，不能继续用旧 16KB 视图低估请求。

两个实际请求测试均覆盖未配置、64K 和 1M；它们使用本地假服务，不代表真实模型能力。
完整 Chat 端到端 48 项、相关库单测 23 项通过，红绿日志保存在本批证据中。

## 真实对照设计

对照版先冻结本轮开始时的代码及二进制，已包含上一轮的 CLI 完成目标判定修复。
调整版仅改变上述 L1 策略及对应回归。两版各运行一个新的独立会话，使用原样冻结的
`repo-session-fork`，不复用原始三轮的失败会话，也不追改旧成绩。

两版均为 DeepSeek Flash / 原生 1,000,000 / high，请求超时 120 秒、重试 5 次，
任务时限 1200 秒，成员原有每回合 200 次步骤预算不变。原生窗口来源为用户确认的 D-36。
隐藏检查在模型退出后运行，源码构建和评分保持原有 bubblewrap。隐藏测试和参考答案不进入模型工作区。

本次是各一次的探索性对照，不是稳定完成率估计；两次运行可能重叠，并与本机离线检查共享资源，
耗时不能作为严格性能结论。原任务缺少旧 TUI 测试读取的 `review/tmp/parity_scenario.json`，
此限制保持原样，扩大测试范围时的相关失败不能全部归因于模型或上下文策略。

## 真实运行结果

两份新会话均已启动，但没有形成可评分的完成结果，不能作为完成率样本：

- 对照版 `proj_0b2a549bf184` 产生 401 条有效 JSONL 记录、391 次工具调用；
  在 verifier 反复修正公开检查时被本机终止，最后没有 `result` 记录。
- 调整版 `proj_f1723e4d00c0` 产生 78 条有效记录，另有末尾 1 行在终止时截断；
  在 `cargo test` 链接阶段收到 `Disk quota exceeded (os error 122)`，没有进入隐藏评分，
  也没有 `result` 记录。

直接配额错误证据仅在调整版日志中；对照版是在共享环境出现故障后被人工停止。
`/tmp` 总容量为 16GB，故障后的容量检查曾显示约 13GB 已用，并非故障瞬间的配额测量。
随后只删除了这两份工作目录下被评分器忽略的
临时 `target/` 构建目录，保留源码、日志、JSONL 和错误证据。该环境故障不归因于上下文预算
实现，也不证明调整版优于或劣于对照版。证据目录为
`review/eval/runs/2026-09-19-context-budget/`；本地确定性回归与 `make check` 已独立通过。
