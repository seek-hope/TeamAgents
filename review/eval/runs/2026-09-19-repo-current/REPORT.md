# 当前修复版的真实仓库任务（2026-09-19）

当前 TeamAgents 在原样 `repo-session-fork` 上完成一次完整成功：
CLI **completed / 0**，固定公开 **12/12**，独立隐藏 **7/7**，最终文件范围及权限检查通过。
CLI 耗时 **943.746 秒**，小于原定 **1200 秒**；包含隐藏评分的外层 runner **958.394 秒 / exit 0**。
本批只有 **1 个新样本**，不是稳定成功率或竞品水平的证明。原始三轮 **0/3** 保留不变。

## 固定条件与存储

- 输入仍为提交 `046a43e32a73794e057ae0331ee3247ee3c42179` 的 77 个文件，
  3 crate、58 个 Rust 文件、29,285 行 Rust；内容及权限逐项与原三轮输入相同。
- 提示词、公开检查、隐藏测试、允许文件、1200 秒时限均未变。任务快照在 `task/`；
  隐藏测试只在模型退出后注入独立评分副本，没有提供给模型。
- 模型配置为 DeepSeek Flash / deepseek / high / 原生 **1,000,000**，
  原生窗口来源为用户确认的 D-36；请求超时 **120 秒**，重试 **5 次**。
  `selected.config.toml` 仅含设置与凭据环境变量名，无密钥值。
- 当前二进制含 CLI 已完成目标判定修复及大窗口 L1 预算。与前次因配额中止的候选版共有的
  **35 个 core/engine 生产源码、Cargo 清单/锁文件的哈希完全相同**，见
  `previous-candidate-comparison.json`。本轮没有调整生产代码或模型提示。
- 将本批工作区、XDG 状态和评分 `TMPDIR` 放到仓库的忽略目录 `review/tmp/` 下。
  开始时该磁盘约 **404GB** 可用，结束时约 **401GB**；`/tmp` 仍约 **8.7GB 已用 / 7GB 可用**。
  模型工具与候选评分仍走原有 bubblewrap；没有因换存储位置而放宽隔离。
- 仅启动一个新样本，没有并行评测。运行期间只读取日志及准备独立原始输入，没有追加提示、
  修改候选、延长时限或人工清理候选文件。

本次成功没有证明某项修复的因果效果：与原三轮相比同时存在运行器修复、随机轨迹和存储位置差异。
前次中止对照没有有效完成样本，也不能据此声称大窗口预算优于固定 16KB。

## 独立交付证据

会话为 `proj_d9c92953e804`。最终只有 Leader 一个成员，一条 `COMPLETED` 回合，
core 的 `goal_state = done`，无待结算团队任务。原始终态见 `final-state.json`。

独立评分在可信输入副本中只叠加两个允许源码，检查并通过：

1. 失败 fork 清理目标并保留源会话。
2. 失败 open/switch 保留源会话可用。
3. 不复制团队事实、其他成员私有状态或项目工作文件。
4. 旧线性历史迁移到目标上下文。
5. 会话 profile、模型与档位在 fork 和重开后保留。
6. 非 Leader 的活动回合阻止 fork，且不取消该回合。
7. 分支树映射到目标上下文，源文件不变。

随后四个固定公开 suite 共 **12/12** 通过。评分详情在 `grade.json`，执行日志在 `grade.log`。
另外对最终目录重新比对：只修改 `engine/src/session.rs` 与 `engine/src/worker.rs`，
没有额外文件、缺失文件、受保护内容或权限变化。`candidate.patch`、`candidate-manifest.json`
与 `fixture-manifest.json` 保留了可复核的差异；没有将此历史任务的模型补丁合并回当前项目源码。
文件保护只覆盖最终状态，不代表模型执行全过程从未临时改动其他文件。

## 轨迹与用量

| 指标 | 数值 |
|---|---:|
| 模型请求 / 持久步骤 | 128 |
| 累计输入 tokens | 20,410,840 |
| 累计输出 tokens | 146,489 |
| 合计 tokens | 20,557,329 |
| 缓存输入 tokens | 15,680,512 |
| 未提供用量的请求 | 0 |
| 最后一次输入 tokens | 227,234 |
| 工具调用 | 149 |
| Shell / read_file / edit_file | 89 / 27 / 26 |
| read_history | 0 |
| 结构化 `ok=false` 工具回执 | 0 |
| 完整 Shell 结果中的非零退出 | 9 |

这些 token 数来自服务用量账本，累计输入不等于一次上下文长度，也不是费用估算。
工具外层 `ok=true` 不代表 Shell 内部命令全部成功。最终私有回合记录中的 **89** 份 Shell
结果全部可解码，其中 **9** 份带终端非零退出标记；另有 **10** 份结果含 `test result: FAILED.`
（包括同类检查的重跑，部分管道最后一步返回 0）。完整诊断在 `full-shell-diagnostics.json`，
不能把本轮描述为“零错误运行”。

轨迹中的具体摩擦包括：

- 在下一次 Shell 调用中修改已经消失的 `/tmp/probe2.py`；现行工具说明本来就明确临时文件不跨调用保留。
- 自制探针误读协议字段、共享事实载荷，修正探针后继续；这些失败不能直接当作产品缺陷。
- 模型扩展到全仓检查，遇到旧快照缺 `review/tmp/parity_scenario.json`、隔离环境没有 Codex/
  DNS 等问题。未针对每项额外失败在原始快照上重新做成对验证，不据模型自述认定所有失败都与修改无关。

`model-delivery.md` 是模型自行生成的交付报告，作为原始产物保存；其额外测试数量与原因分析
不替代独立评分器证据。本轮只认证固定任务的隐藏/公开与最终文件条件，不宣称整个历史仓库全绿。

## 可复核材料与复跑

`manifest.json` 保存冻结二进制、生产源码、任务和配置哈希；收尾确认运行器源码与任务未改变。
`teamagents.jsonl` / `teamagents.stderr` 是原始自动化输出。完整成员树和回合检查点分别压缩为
`leader-chat-tree.json.gz`、`leader-turn-checkpoint.json.gz`，用于核对 CLI 中被截断的工具参数与结果。
归档前扫描了当前环境中的凭据值；原始输入、模型输出与验证记录没有混入密钥。

在仓库根目录复跑（证据目录须为新目录；所选配置仍需可用 `DEEPSEEK_API_KEY`）：

```bash
mkdir -p review/tmp
eval_root=$(mktemp -d "$PWD/review/tmp/repo-eval-XXXXXX")
mkdir -p "$eval_root/scratch" "$eval_root/config/teamagents"
cp review/eval/runs/2026-09-19-repo-current/selected.config.toml "$eval_root/config/teamagents/config.toml"
XDG_CONFIG_HOME="$eval_root/config" TMPDIR="$eval_root/scratch" \
  review/eval/run.sh --only repo-session-fork --out "$eval_root/run"
```

该命令使用当前 `engine/target/debug/teamagents`，复现本批实现需另外核对 `manifest.json`
或用 `--bin` 指定对应冻结二进制。本批执行前重新离线构建 CLI/评分器；生产源码未变化，
沿用此前通过的 core **84** / engine **312** / TUI **104** 离线基线，不把候选快照的测试数加进去。

本轮没有运行 Codex CLI、Claude Code、pi 或 Hermes 作同题对照，也没有补齐其他供应商、
陌生仓库、多次重复或混合团队验收。长期目标保持进行中。
