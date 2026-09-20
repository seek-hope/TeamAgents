# 多文件 Rust 任务：TeamAgents / Codex CLI 首次对照（2026-09-19）

本轮为单项、单次的起步对照，随后针对发现的读回缺陷另跑修复版。
所有产出均使用同一独立隐藏评分器；成功要求正常完成、退出 0、隐藏和公开测试通过且受保护文件不变。
没有运行 Claude Code、pi 或 Hermes，不据此宣称达到四个竞品的整体水平。

## 固定条件与差异

- 相同 `rust-ledger` 原始 fixture、完整提示词、五个允许修改的文件及 600 秒任务时限。
  Task 各文件与实际二进制/源码的哈希见 [baseline.manifest.json](baseline.manifest.json)。
- 相同 DeepSeek Flash 模型、供应商环境凭据、`high` 思考档位和 **1,000,000 原生上下文**；
  来源是用户确认的 D-36。TeamAgents 通过 `context_window`，Codex 通过 `model_context_window` 明确配置。
  所有实际 TeamAgents 成员在结果中也报告 1,000,000。
- 使用独立新工作区、状态和配置；配置仅提取模型/供应商参数，不复制用户 hooks、MCP 或认证文件。
  密钥通过原有环境变量提供，归档前逐项核对凭据值未进入记录。
- TeamAgents 为工作树当前实现的冻结二进制，full-auto 保留原生文件权限与 bubblewrap；
  Codex CLI 为本机 **0.155.0**，`workspace-write`、`approval_policy=never`，从 stdin 接收同一提示词。
  为避开已实测的嵌套 mount-lock 错误，Codex 经权限流程在外层工具沙箱之外启动，保留其自身沙箱。
- 客户端并非完全相同：TeamAgents 使用 chat completions，Codex 使用 Responses。
  Codex 提示自定义模型缺少内置元数据，并自动发现本机 Skills、缩短目录描述；这些提示保留在原始 JSONL。
  TeamAgents 自行选择新增验证成员，Codex 本次没有子代理。没有强制两边采用相同工作分解。
- 首轮两边并发启动，共享供应商吞吐，启动相差约 12 秒；修复版随后单独运行。
  TeamAgents 耗时来自 CLI result，包含自身末尾公开检查；Codex 来自外层进程计时，隐藏评分均不计入任务耗时。
  因协议、工具、Skills、并发条件及计时口径不同，不应把时长比值当成受控性能提升率。

## 首轮结果

| 被测实现 | 完整成功 | 秒 | 累计输入 / 输出 tokens | 隐藏测试 | 文件保护 |
|---|---|---:|---:|---|---|
| TeamAgents 修复前 | 是 | 497.7 | 4,560,035 / 105,545 | 11/11 | 通过 |
| Codex CLI 0.155.0 | 是 | 133.5 | 662,490 / 28,773 | 11/11 | 通过 |

Token 值使用各客户端最终记录；累计输入不是单次上下文占用，不因超过 1M 就说明模型窗口被突破。
Codex 另外报告 cached input 643,584、reasoning output 16,427；TeamAgents 累计 cached input 3,790,464。
这些是供应商/客户端报告字段，没有换算费用或推断缺失的用量。

TeamAgents 首轮 114 次模型调用、151 次团队/执行工具调用；其中历史读回 29 次，
9 次以另一条读回回执为来源，最大嵌套深度 4。Codex 为 20 次命令执行；
两边工具粒度不同，这两个工具计数不能直接当作效率比值。
TeamAgents 三条失败工具回执均保留；Codex 一条非零命令退出也保留，不能挑选只成功的调用。

原始证据：

- TeamAgents：[JSONL](teamagents-before.jsonl)、[指标](teamagents-before.metrics.json)、
  [隐藏评分](teamagents-before.grade.json)、[候选补丁](teamagents-before.candidate.patch)、
  [runner 日志](teamagents-before.runner.log)、[stderr](teamagents-before.stderr)。
- Codex：[JSONL](codex.jsonl)、[指标](codex.metrics.json)、[隐藏评分](codex.grade.json)、
  [候选补丁](codex.candidate.patch)、[最终说明](codex.answer.md)、[stderr](codex.stderr)。

## 已落实的产品修复

本次实测触发了[历史读回指针修复](../../../readback-pointer-2026-09-19.md)：
旧读回页被遮蔽时推荐原始来源与页码，避免逐层读取包装后的回执。
修复先通过失败回归证实，再经单测、实际请求回归与全量检查验证。
它解决一类重复读回来源错误，不证明已解决全部任务耗时、验证分工或上下文保留问题。

## 修复版独立复跑

使用同一提示词、原始 fixture、high 档位、原生 1M、600 秒时限，在全新工作区运行，
二进制与源码哈希见 [teamagents-after.manifest.json](teamagents-after.manifest.json)。
本轮额外检查自动创建的 verifier profile，修复前后均为 `deepseek-flash`、
`context_window=1000000`、`reasoning_effort=high`。

| 项目 | 修复前 | 修复后 |
|---|---:|---:|
| 完整完成 / 隐藏 11/11 / 公开与文件保护 | 全部通过 | 全部通过 |
| 总秒数 | 497.7 | 554.9 |
| 模型调用 | 114 | 71 |
| 累计输入 tokens | 4,560,035 | 3,056,042 |
| 累计输出 tokens | 105,545 | 140,664 |
| 工具调用 | 151 | 85 |
| 历史读回 | 29 | 12 |
| 读取另一条读回回执 | 9 | 0 |
| 最大读回嵌套深度 | 4 | 1 |

修复版本次没有重复包装链，但**总耗时反而更长**，不宣称整体提速或因果性能提升。
两次代码产出与模型分工均有随机差异，修复版也不是与 Codex 同时启动。
较少调用和零嵌套是本次观测；指针修复的确定性证据来自先失败后通过的回归。
TeamAgents 在此任务上的时长、输出用量与验证分工仍有明确优化空间。

进一步检查原始轨迹发现，这里的「文件保护通过」仅指**结束后的候选文件树**。
`teamagents-after.jsonl` 第 57 行中 `[cwd: /tmp/verify]` 与命令实际 `pwd` 不同；
第 64–65 行确认验证器因目录恢复失败，在共享项目的 `tests/` 临时写入了四个验证文件，
第 66 行再将其删除。因此不能声称本次运行过程中一直遵守「不新增其他文件」的约束。
全次有 16 条 Shell 回执包含目录恢复错误（不含历史读回对同一错误的重复展示）。
后续的[Shell 目录恢复修复](../../../shell-cwd-2026-09-19.md)使用真实沙箱回归阻止该错误目录执行；
本组原始数据、评分与候选补丁保持不变。

修复版证据：[JSONL](teamagents-after.jsonl)、[指标](teamagents-after.metrics.json)、
[隐藏评分](teamagents-after.grade.json)、[候选补丁](teamagents-after.candidate.patch)、
[runner 日志](teamagents-after.runner.log)、[stderr](teamagents-after.stderr)、
[本轮产品补丁](readback-fix.patch)。

## 复跑口径

TeamAgents 临时配置只设置选定模型及 `context_window=1000000`、
`generation_options.reasoning_effort="high"`，其余当前配置字段见 manifest：

```bash
XDG_CONFIG_HOME=/path/to/isolated/config \
review/eval/run.sh --bin /path/to/frozen/teamagents \
  --only rust-ledger --timeout 600 --out /tmp/new-ledger-run
```

Codex 临时 CODEX_HOME 配置同一模型/供应商、`model_context_window=1000000`、
`model_reasoning_effort="high"`、`approval_policy="never"`，复制 fixture 后运行：

```bash
CODEX_HOME=/path/to/isolated/codex \
timeout --signal=INT --kill-after=10s 600s codex exec --json --ephemeral \
  --ignore-rules --sandbox workspace-write --skip-git-repo-check \
  --cd /path/to/fresh/fixture --color never - < review/eval/tasks/rust-ledger/prompt.md
```

正式本次运行的超时由 Python 外层进程控制，原退出状态在 `codex.exit.json`。
Codex 退出后用相同评分入口检查，隐藏测试不能复制到模型工作区：

```bash
TA_EVAL_TASK_DIR="$PWD/review/eval/tasks/rust-ledger" \
TA_EVAL_CANDIDATE_DIR=/path/to/fresh/fixture \
TA_EVAL_GRADE_OUTPUT=/tmp/codex-grade.json \
cargo test --offline --locked --manifest-path engine/Cargo.toml \
  --test eval_grader grade_candidate -- --exact --ignored --nocapture
```

这仍是六源文件的小型构造仓库。更大真实仓库、多次独立采样、其他模型与三个竞品仍需分别验收。
