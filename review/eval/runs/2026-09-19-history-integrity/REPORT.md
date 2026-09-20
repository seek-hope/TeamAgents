# 持久对话完整性与长历史遍历验收（2026-09-19）

本归档记录 [历史完整性修复](../../../history-integrity-2026-09-19.md) 的红绿回归、
完整离线检查、长链观测和一次真实 DeepSeek 续接/重建检查。修复范围属于既有
D-26/D-28、D-32/D-35 恢复与长任务范围；没有扩展产品范围。

## 结果

- 历史读取、树结构验证、线性祖先遍历、分支压缩根和回退错误传播已修复。
- 新增 8 项回归全部通过；`make check` 通过：
  core **84**、engine **320**（另 **3 ignored**）、TUI **104**。
- 长链探针在同一 debug 构建中观察到展开与回退列表合计耗时：
  4,000 节点 **166.335 ms → 34.662 ms**，8,000 节点
  **672.021 ms → 47.944 ms**，16,000 节点 **2,467.443 ms → 100.671 ms**。
  这些是单次本机操作观测，没有作为性能门槛或模型任务 SLA。
- 真实 DeepSeek Flash 检查三阶段均通过：文件读写、同会话历史恢复、关闭重建后的
  历史与用量恢复。使用 high、原生 **1,000,000** 上下文、120 秒请求超时和 5 次重试；
  共 17 次请求，输入 92,589、输出 2,088、总计 94,677 tokens，缓存输入 88,704。
  工具成功 13 次、失败 1 次；首次阶段有一次无效 `complete_task`，之后正常完成，
  因此保留为非零工具计数。

## 红绿证据

红回归：

- `legacy-red.log`：旧线性历史损坏会被当成空对话。
- `tree-red.log`：旧树实现未拒绝结构损坏。
- `resume-red-corrected.log`：生产恢复在修复前错误地进入模型请求。
- `compaction-red.log`：旧实现的 `/rewind 0` 后压缩摘要跳到旧分支根
  （`n1`，应为当前分支根 `n4`）。

绿回归：

- `legacy-green.log`、`tree-green.log`、`resume-green.log`、
  `identity-green.log`、`compaction-green.log`、`chat-unit-green.log`
- `traversal-before.log`、`traversal-after.log`
- `make-check.log`、`test-counts.json`
- `live-deepseek.log`、`live-deepseek/report.json`

`product-and-tests.patch` 是以本轮开始时的 `before/` 快照为基准生成的生产代码与测试
增量，不代表整个工作树相对 Git HEAD 的差异。四份同步更新的文档没有重复塞入该补丁，
其当前/基线哈希在 `manifest.json` 中列出。`before.json` 保留原始快照清单；复制
`engine/tests/chat_e2e.rs` 到快照目录晚于该清单生成，因此该文件的基准哈希也在
`manifest.json` 中单独列出。

## 不计入产品红证据的探针

`invalid-probes/` 单独保存两份早期探针：

- `resume-red-initial.log` 误读了 `TurnRun.error`，没有读事件载荷中的实际错误；
  校正后才使用 `resume-red-corrected.log`。
- `compaction-baseline-incomplete.log` 的旧代码副本缺少编译期配置样例；
  补齐样例后才使用 `compaction-red.log`。

它们用于保留审查过程，不能单独证明产品缺陷。

## 可复跑入口

确定性检查：

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib chat::tests
cargo test --offline --locked --manifest-path engine/Cargo.toml --test session_identity
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e \
  compaction_after_empty_rewind_keeps_the_new_branch_root_across_restart -- --exact --nocapture
make check
```

真实模型检查需要本机凭据，且必须使用本归档中的非敏感
`selected.config.toml` 与仓库的 `review/eval/live-models.example.toml`：

```bash
TEAMAGENTS_LIVE_MODELS_CONFIG="$PWD/review/eval/runs/2026-09-19-history-integrity/selected.config.toml" \
TEAMAGENTS_LIVE_MODELS_MANIFEST="$PWD/review/eval/live-models.example.toml" \
TEAMAGENTS_LIVE_MODELS_EVIDENCE="$PWD/review/tmp/history-integrity-20260919/live-deepseek-rerun" \
TMPDIR="$PWD/review/tmp/history-integrity-20260919/scratch-rerun" \
cargo test --offline --locked --manifest-path engine/Cargo.toml --test live_models \
  live_chat_model_matrix -- --ignored --exact --nocapture
```

该命令会调用真实供应商；归档只保存模型名、参数、用量和断言结果，不保存认证值。
模型检查不能替代损坏注入、长链和压缩回归，也不能证明其他供应商、SIGKILL 时序
或竞品整体能力。

## 完整性校验

`manifest.json` 固定了本轮目标源码、测试、锁文件、工具链、模型配置与增量补丁的
SHA-256。`SHA256SUMS` 覆盖本目录除自身外的全部归档文件，并在归档完成后执行：

```bash
cd review/eval/runs/2026-09-19-history-integrity
sha256sum -c SHA256SUMS --quiet
```

归档前扫描了当前环境中名称含 key/token/secret/password/credential/auth 的非空值；
没有任何凭据值出现在待归档文件中。

本批完成的是一次有界修复与验收，长期对齐 Codex CLI、Claude Code、pi、Hermes 的目标
仍在进行中。
