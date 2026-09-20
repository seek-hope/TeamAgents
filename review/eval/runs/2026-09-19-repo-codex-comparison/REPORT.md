# 真实仓库同题对照：TeamAgents 与 Codex CLI（2026-09-19）

Codex CLI 0.155.0 在原样 `repo-session-fork` 上正常退出 **0、472.672 秒**，
随后通过独立隐藏 **7/7**、公开 **12/12** 及最终文件内容/权限检查。
与[此前 TeamAgents 样本](../2026-09-19-repo-current/REPORT.md)相比，两边各有一个有效交付通过样本。
首次 Codex 尝试读到了工作区外的隐藏测试，已中止、单独保留并作废，不计入能力成功或失败。
这不是稳定成功率、因果性能实验或四个竞品的整体排名。

## 独立交付结果

| 项目 | TeamAgents 当前修复版 | Codex CLI 0.155.0 |
|---|---:|---:|
| 正常完成 / 退出 0 | 是 | 是 |
| 任务耗时 | 943.746 秒 | 472.672 秒 |
| 独立隐藏检查 | 7/7 | 7/7 |
| 独立公开检查 | 12/12 | 12/12 |
| 最终文件内容、范围及权限 | 通过 | 通过 |
| 累计输入 tokens | 20,410,840 | 9,202,881 |
| 累计输出 tokens | 146,489 | 76,579 |
| 缓存输入 tokens | 15,680,512 | 9,108,480 |
| 工具调用 | 149（含 89 次 Shell） | 96 次命令执行 |
| Shell / 命令非零退出 | 9 | 6 |
| 工具结果含 `test result: FAILED.` | 10 份 | 9 份 |

用量取自各客户端的最终账本。累计输入不等于单次上下文占用，也没有换算费用。
Codex 另报告 reasoning output **53,774**；它的 exec JSONL 没有完整模型请求账本，
不能把一个 `turn.completed` 当作只请求模型一次。TeamAgents 明确记录 128 次模型请求。
工具粒度不同，调用次数不作效率比值；失败输出可能包含重复检查或管道，不等于同样数量的独立缺陷。

表内公开检查指**模型退出后的独立评分**。Codex 在模型回合内运行指定命令时实际为 **11/12**：
`model_discovery_does_not_block_worker_requests` 创建本地 socket 被原生沙箱拒绝。
无模型探针在同一外层文件系统中能创建并绑定回环 socket，加入 Codex `:workspace` 后在
socket 创建时得到 `EPERM`，见 `codex/loopback-boundary.json`。
随后两边均在同一评分器的原有 bubblewrap 中通过全部 12 项，不能把这两个执行环境混为一谈。

两边都扩大执行过全仓检查，并有失败。Codex 的完整输出保留了网络/嵌套沙箱错误、
旧 TUI 快照缺少 `review/tmp/parity_scenario.json`、自制探针参数错误及后续修正。
没有对每一项扩展失败都做原始快照成对校验，不照抄模型自述为“全是环境问题”，也不宣称全仓全绿。

## 相同条件与差异

- 相同完整提示词、1200 秒时限、两个允许源码、隐藏测试与评分器。
  固定提交仍为 `046a43e32a73794e057ae0331ee3247ee3c42179`，
  77 文件、3 crate、58 个 Rust 文件、29,285 行 Rust；输入内容与模式位逐项相同。
- 相同 DeepSeek Flash / high / 原生 **1,000,000**，原生窗口来源是用户确认的 D-36。
  两边使用同一个供应商凭据环境变量，配置和记录不含密钥。
- TeamAgents 使用 chat completions，Codex 使用 Responses。TeamAgents 请求超时 120 秒、
  重试 5 次；Codex 显式配置流空闲超时 120 秒、请求及流重试各 5 次。
  超时和重试的语义不同，不能称作完全对齐。
- 两次顺序独立运行，没有同步供应商吞吐。TeamAgents 耗时来自 CLI result，
  Codex 来自外层进程计时；两者都不含独立隐藏评分。
- 保留各自工具、指令、Skills 和工作分解。Codex 的自定义模型元数据缺失及 Skills 描述缩短
  两条警告原样保留。没有强制相同的调用/步数预算，也没有向模型追加提示或延长时限。
- Codex 加外层文件系统隔离，保留原生 `workspace-write` 与 `approval_policy=never`；
  TeamAgents 继续使用原有工具隔离。Codex 的 `/tmp` 在本次模型进程内持续存在，
  TeamAgents 的临时目录按工具调用重建；两边的网络限制也不同。
- 工作区、状态及评分临时目录均使用 SSD 上的 `review/tmp/`。
  模型候选补丁只作为历史任务产物保存，没有合并回当前项目生产源码。

这些差异限制了时长、token 和工具次数的比较。本次观测中 Codex 用时和累计用量更少，
但不能据两个单样本断言一般性能优势，也不能单独归因于 TeamAgents 的 L1 工具结果预算。
TeamAgents 原三轮 **0/3** 与此前配额中止记录保持不变。

## 隐藏材料的读取隔离

首次 Codex 尝试使用原生 `workspace-write`，它仍能读取工作区外的文件。
模型读取了相邻目录的完整隐藏测试，临时复制进工作区运行后又删除；
仅看最终文件范围会漏掉这类污染。该次 **339.999 秒 / SIGTERM** 中止，
没有独立成绩或完整用量汇总，详见 [作废记录](invalidated-attempt/REPORT.md)。

有效样本使用新输入、新 CODEX_HOME 和新会话。外层 bubblewrap 只暴露公开工作区、
独立 Codex 状态、系统文件、只读 Rust 缓存和用户技能目录，没有挂载开发仓库、
隐藏任务目录、历史候选或评测日志。模型启动前的无模型探针确认：

1. 隐藏测试、开发仓库和旧配置不可见。
2. 公开工作区可写，原无凭据测试配置可读。
3. Codex 内层沙箱拒绝写入外层本来可写的 `/codex-home`。
4. 离线工具链与供应商 DNS 可用。

证据在 `codex/native-boundary.json`、`preflight.json`、`preflight-*.log`
与 `network-preflight.json`。早期工具链路径及 CLI 语法探针错误也保留，
不计入真实模型样本。新样本 96 条完成命令中没有首次尝试的隐藏测试读取命令。

运行前配置与冻结哈希一致；运行后仅新增 `/workspace` 的 `trust_level="trusted"` 项目记录，
模型、provider、原生窗口、思考档位及沙箱设置未变，见 `config-change.json`。
初始配置按原序列化方式重建后与运行前 SHA-256 完全一致，同时保存了最终配置，
没有把文件发生变化说成字节未变。

## 证据与复核

`comparison.json` 保存两边指标及条件差异。Codex 的证据位于 `codex/`：

- `manifest.json`：冻结二进制、评分器、当前源码、配置、任务和临时驱动的 SHA-256。
- `codex.jsonl`、`codex.stderr`、`exit.json`：原始轨迹与退出记录。
- `grade.json`、`grade.log`：独立评分和完整测试结果。
- `candidate.patch`、两个文件清单：最终源码差异、输入内容及权限。
- `command-diagnostics.json`、`client-warnings.json`：包括失败在内的命令和启动警告。
- `answer.md`、`agent-messages.md`：模型自述，不能替代独立评分。
- `task/`：原样任务；`selected.config.toml`、`post-run.config.toml`：前后配置。
- `mount-arguments.json`、`command.json`：实际挂载集合和启动命令。

临时 Rust 评分器、Codex 二进制和 Python 驱动保留在忽略目录
`review/tmp/repo-fork-codex-isolated-20260919/`，不是新的产品运行入口。
它们的哈希与 manifest 对照后，可重跑无模型读取隔离探针：

```bash
python3 review/tmp/repo-fork-codex-isolated-20260919/native_probe.py
```

使用冻结评分器复核本次候选（先确保同目录 `bin/eval-grader` 的哈希符合 manifest）：

```bash
eval_root="$PWD/review/tmp/repo-fork-codex-isolated-20260919"
TA_EVAL_TASK_DIR="$eval_root/task" \
TA_EVAL_CANDIDATE_DIR="$eval_root/work" \
TA_EVAL_GRADE_OUTPUT="$eval_root/regrade.json" \
TMPDIR="$eval_root/scratch" \
  "$eval_root/bin/eval-grader" grade_candidate --exact --ignored --nocapture
```

重新采样必须新建目录和会话，从 `stage_fixture` 生成原始输入，再按记录的挂载集合运行；
不能复用本次已修复候选。归档前扫描了当前凭据值；`SHA256SUMS` 覆盖本目录的全部证据。
本轮没有修改 Rust 产品或评分逻辑，未为文档重复整套 `make check`，当前离线基线仍为
core **84** / engine **312** / TUI **104**。Claude Code、pi、Hermes、陌生仓库和多次采样仍待验收，
长期目标继续进行中。
