# 同条件真实仓库任务三次重复（2026-09-19 起）

预登记的三个新样本已全部执行，完整交付结果为 **3/3**。统计同时要求 CLI 正常完成、
固定公开验收通过、独立隐藏评分通过，以及最终文件范围与权限符合任务要求。
这是同一个历史仓库任务、同一环境声明下的三次观测，不证明通用稳定成功率或竞品水平。

实际开始：`2026-09-19T23:19:40.331979+08:00`；实际结束：`2026-09-19T23:57:11.315406+08:00`。时间均带 Asia/Taipei 对应的 UTC+08:00 偏移。
归档目录按批次开始日期命名。

## 逐轮结果

| 样本 | CLI 状态/退出码 | CLI 秒 | 含评分的 runner 秒 | 隐藏 | 独立公开 | 文件保护 | 完整交付 |
|---|---|---:|---:|---:|---:|---|---|
| 1 | completed/0 | 938.784 | 965.282 | 7/7 | 12/12 | 通过 | 成功 |
| 2 | completed/0 | 750.632 | 777.573 | 7/7 | 12/12 | 通过 | 成功 |
| 3 | completed/0 | 482.324 | 507.709 | 7/7 | 12/12 | 通过 | 成功 |

逐轮 `execution.json`、`teamagents.jsonl`、`grade.json` / `grade.log` 保存原始结果；
`metrics.json` 给出完整计数。`final-state.json` 保存结束后的目标、任务、回合与事件。
模型自行提交的报告和 `goal_done` 不替代独立评分。

## 固定条件

- 输入为提交 `046a43e32a73794e057ae0331ee3247ee3c42179` 的 77 个文件，3 crate、58 个 Rust
  文件、29,285 行 Rust。内容、大小和权限与旧基准逐项相同，见 `common/fixture-comparison.json`。
- 仅允许修改 `engine/src/worker.rs`、`engine/src/session.rs`。隐藏七项行为在模型退出后的
  独立可信副本中检查，再执行四个公开 suite，共 12 项；最终所有其他输入及权限受保护。
- DeepSeek Flash / deepseek / high / 原生 **1,000,000**，窗口来源为用户确认的 D-36；
  请求超时 **120 秒**、最多重试 **5 次**。仅使用 `common/selected.config.toml` 声明的凭据环境变量。
- 每轮任务上限 **1200 秒**，独立评分每条命令 **300 秒**。工作区、状态和评分临时目录放在
  磁盘上的 `review/tmp/`，执行及评分沿用原有 bubblewrap。
- 三轮顺序执行，每轮使用全新工作区与会话。事前登记顺序和停止条件，不因成功提前停止，
  不丢弃超时或失败样本，不修改候选、不补发提示、不延长时限。
- `common/repetitions-registration.json` 记录二进制、源码、配置和任务的 **49 项**冻结哈希。
  监督脚本每轮前后核对；运行期间没有改变这些输入。`common/implementation.patch` 可从登记的
  Git HEAD 重放本批源码和任务，47 个文件均重新核对为登记哈希，见 `common/implementation-replay.json`。

## 本次环境声明与历史成绩

工具 HOME 改为成员私有目录后，旧公开测试不再隐式读取项目 `.config`。启动模型前，
已知正确的参考补丁仍通过隐藏 7/7，却在公开 `fork_rewind` 报 `unknown model profile leader_main`。
因此先在评分配置中增加显式 `config_home = ".config"`，并同步提示词及公开命令中的
`XDG_CONFIG_HOME="$PWD/.config"`。没有挂载宿主配置，也没有取消私有 HOME。

修正环境后，同一参考补丁通过隐藏 7/7、公开 12/12 和文件保护；原始缺陷快照仍为隐藏 2/7，
五项预期缺陷失败。配置路径校验、引用转义、HOME 独立及配置保护由两项新增回归覆盖。
评分器 8 项普通回归与 runner 30 项契约通过，完整 `make check` 为 core **90** / engine
**336** / TUI **104**，另有 3 项显式 ignored；这些测试数不等于真实供应商验收数。

固定源码输入、隐藏行为、公开 suite 和时限没有变化，但提示词及评分环境声明哈希改变。
因此本组三次不能与历史 **0/3**、此前修复版的 **1/1** 或 Codex 单样本直接合并成同设置成功率，
也不能单独归因于某项产品修复。新旧任务快照和本批修改在 `common/` 中保留。

## 轨迹与实际错误

| 样本 | 模型请求 | 累计输入 tokens | 累计输出 tokens | 工具调用 | read_history | 外层工具失败 | Shell 非零退出 | 含测试失败的 Shell 结果 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 122 | 12,893,910 | 143,529 | 155 | 0 | 0 | 7 | 4 |
| 2 | 94 | 11,259,816 | 102,879 | 116 | 0 | 0 | 7 | 6 |
| 3 | 70 | 7,228,216 | 81,542 | 86 | 0 | 0 | 7 | 4 |

累计输入是请求账本的总和，不是一次上下文长度或费用估算。完整工具回执来自各成员的回合
检查点，并按会话、成员、回合和调用 ID 对应到 JSONL；所有 Shell 回执均可解码。
`ok=true` 只说明工具调用外层返回正常，Shell 非零退出与管道掩盖的测试失败分别统计。
完整诊断见各样本 `full-shell-diagnostics.json`，压缩的成员树、历史、检查点和长输出在 `session/`。

前两轮均有重复 `cd engine`、模型自制探针字段/函数错误，以及扩大运行历史全仓测试的失败。
记录中可见缺 Codex CLI、无 DNS 和旧 TUI 夹具缺失；没有对每项额外失败逐一重跑原始快照，
因此不把所有额外失败都归结为环境，也不宣称全仓测试通过。第三轮另有自制探针遗漏
`action_id` / `base_revision`、对临时脚本跨 Shell 调用保留的错误假设；修正后继续完成。
第一轮自主增加一个复核成员，第二轮由 Leader 单独完成；同一提示词允许这些不同执行轨迹。
每轮成员的原生窗口和持久用量都保存在 `metrics.json` 的 `usage_by_agent`。

## 证据复核与复跑

每轮 `candidate.patch` 均在独立目录从固定输入重放，结果哈希与实际候选一致。
模型产出的历史任务补丁没有合并到当前产品源码。最终文件保护不证明执行全过程没有临时文件改动。
归档前检查原始文本和压缩内容是否含当前环境中的凭据值；结果和范围见 `verification.json`。
`SHA256SUMS` 覆盖归档中的其他文件。

从仓库根目录启动新的单轮（需可用的 `DEEPSEEK_API_KEY`，目录必须全新）：

```bash
mkdir -p review/tmp
eval_root=$(mktemp -d "$PWD/review/tmp/repo-repeat-new-XXXXXX")
mkdir -p "$eval_root/scratch" "$eval_root/config/teamagents"
cp review/eval/runs/2026-09-19-repo-repeat/common/selected.config.toml "$eval_root/config/teamagents/config.toml"
XDG_CONFIG_HOME="$eval_root/config" TMPDIR="$eval_root/scratch" \
  review/eval/run.sh --only repo-session-fork --out "$eval_root/run"
```

复跑入口使用当前二进制和任务；复现本批实现需另行从记录的 HEAD 与 `common/implementation.patch`
恢复并构建，核对冻结清单。`common/run-repetitions.py` 保存本批三轮监督逻辑，只作执行证据，不能在
本归档目录原地启动。原样保留批次的前置检查、校准输出及任务快照。

本批没有新增陌生第三方仓库、需求实现/重构、多供应商混队、真实模型下 TUI 全流程或另外
三个竞品的验收。目标仍在进行中；下一步应扩展代表性任务并完成服务、故障与发行验收。
