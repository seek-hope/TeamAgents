# TeamAgents × DeepSeek V4.1 Flash：评测轨迹与设计改进分析

日期：2026-09-23。本轮只读分析产品代码，新增本报告和复算证据；未实施新的设计改动，未调用模型重跑评测。

**结论**

当前证据不足以证明 TeamAgents 相对同配置的基础 Agent 提升了成功率；轨迹则明确显示，执行环境、失败恢复、完成判定和验证方法存在足以损害成功率的问题。下一阶段应优先完善这些闭环，并用对照实验判断协作策略的收益。

单人成队是 TeamAgents 的正常形态，也是实现方案第 2.2 节明确要求支持的能力。评价对象应是整个系统选择、执行和验证方案的质量，而不是是否调用成员。六题复测由 Leader 独立完成完全合理，既不能据此批评组队设计，也不能将六题改善归因于多成员协作。

**数据口径和官方对照**

- 原始基线：`2026-09-22-tb21-teamagents-flash/run1` 的 89 题，由 `repair` 中的 13 题逐题替换。机械计分 53/89。
- 修复复测：`2026-09-22-tb21-fullauto-fix` 的六题，全部从 0 变为 1。替换后的机械计分为 59/89 = 66.3%，不是新版完整重跑的成绩。其余 83 题仍来自旧实现，剩余 30 个零分也不能叫作“新版的 30 个失败”。
- 用户提供的[官方发布页](https://api-docs.deepseek.com/zh-cn/news/news260910/)及其[榜单图片](https://api-docs.deepseek.com/zh-cn/img/v4.1_260910_benchmarktable_cn.png)给出 V4.1 Flash 的 Terminal-Bench 2.1 成绩 **90.6%**，并确认 `deepseek-flash` 对应 V4.1 Flash。
- [官方模型卡](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash/blob/main/README.md)注明最大推理强度、1M 上下文；scaffold 对照使用每题 N=3、每 Agent 最多 500 步、temperature=1.0、top_p=0.95，TB 2.1 禁止网络访问。本次使用 `high`、1M、每题一次；生成配置未显式固定采样参数，任务中存在成功的外网访问。因此不能将约 24.3 个百分点的表面差距全部归因于团队设计。
- 原始 89 题的留存 JSONL 中共有 **6,684 条可解析 tool 记录**，不是旧报告快照中的 6,456 条；有 3 条不可解析行，不能据此还原缺失调用。超时任务有缺失终态及 usage 的情况，缺失用量不是零消耗。

复算脚本：[review/tmp/tb21_design_audit.py](tmp/tb21_design_audit.py)。运行：

```bash
python3 review/tmp/tb21_design_audit.py > review/tmp/tb21-design-evidence.txt
```

输出：[aggregate.json](tmp/tb21-design-evidence/aggregate.json)、[summary.json](tmp/tb21-design-evidence/summary.json)。脚本按上述顺序合并 trial，保留原始分数，不自动重判污染或基础设施失败；并将剩余 30 题的 verifier 日志快照保存到 `review/tmp/tb21-design-evidence/<task>/test-stdout.txt`。这些 probe/快照位于仓库约定的临时审查目录，原始归档不变。

**发现一：评测本身还不能作为干净的能力基线（优先级 P0）**

存在已计为通过、但访问了 benchmark 解答或预期答案的记录：

| 任务 | 可定位的证据 | 对成绩的影响 |
| --- | --- | --- |
| `count-dataset-tokens` | `run1/count-dataset-tokens__*/agent/teamagents.jsonl` 第 28 行实际返回上游 `solution/solve.sh` 和测试内容；第 29 行开始直接写答案 | 已计为通过；不能作为未接触答案的解题证据 |
| `mteb-retrieve` | 对应 JSONL 第 35–40 行检索 benchmark 仓库；第 43、49、51 行明确比较上游 grader 的预期答案，并称根据上游定义核对了关键 prompt 选择 | 已计为通过；存在答案暴露及依据 grader 校准方案的记录 |
| `torch-pipeline-parallelism` | 对应 JSONL 第 45 行下载任务目录，含 `solution/solve.sh`；第 47 行读取下载的测试，第 58 行运行它 | 原本已计为零；仍说明评测信息边界没有建立 |

这不等于所有出现 `test_outputs.py` 的调用都违规。例如 `break-filter-js-from-html` 的任务环境本身提供了 `/app/test_outputs.py`；用户工程的正常测试、任务明确提供的检查器也可以使用。判断必须看来源，不能按文件名一刀切。

[Terminal-Bench 官方完整性说明](https://www.tbench.ai/news/leaderboard-integrity-update)明确将从互联网寻找解答列为 reward hacking，并对相关 trial 记零。这里至少两道已通过题受污染，需要审计后重跑；66.3% 只能继续称为原始机械合并分数。本报告不擅自给出一个已经认证的“修正总分”。

此外，四个零分的 verifier 没有正常跑到任务断言：`qemu-startup`、`qemu-alpine-ssh` 安装依赖 404，`torch-pipeline-parallelism` 下载依赖超时，`winning-avg-corewars` 下载 uv 失败。它们的 Agent 轨迹也存在问题，不能直接改成通过，但这些零分不能全当成模型解题失败。

`prove-plus-comm` 则根本没启动 TeamAgents：原始 `trial.log` 显示 Docker exec 无法进入不存在的 `/app`。适配器仍将默认 `workdir` 固定为 `/app`（`teamagents_agent.py` 的 options 和 `run`）。这是六题复测没有覆盖的独立缺陷。应尊重任务镜像的工作目录，并明确上报启动失败，不能靠创建一个空 `/app` 掩盖真实任务位置。

改进应包括：锁定模型配置、数据集 digest、二进制及适配器哈希；保留任务原生时限和资源约束；禁止向执行 Agent 提供评测解答及隐藏测试；按官方网络条件准备环境；分别报告启动、模型传输、Agent 超时、verifier 基础设施和真实断言失败。评测侧限制不应悄悄改变用户已经确认的 full_auto 主机执行语义。

**发现二：环境故障会诱发大量无收益的“协作”，系统缺少停止扩大的机制（P0）**

旧轨迹中，88/89 题出现 Shell 的 `Failed RTM_NEWADDR`，累计 802 次。这些调用当时仍以工具 `ok=true` 返回。模型需要从字符串里发现执行根本没有发生，之后经常尝试另一个成员或私有 helper，但它们使用同一执行器，不能解除这个故障。

最极端的 `log-summary-date-ranges` 留存 1,388 次工具调用、1,077 次 `read_file`、39 次 `assign_task`。模型把本可以用程序统计的工作变成人工分片阅读和汇总。修复执行层后，同题 15 次工具调用、约 34 秒通过。六题合计工具调用由 1,800 降至 141；复测还同步改变了提示词和适配器，所以这些是修复包效果，不是严格分离后的 Shell 单项因果估计。

应把下面两种决定分开：

- 能力/环境故障：执行器不可用、路径不可见、模型接口不可达。交给运行时诊断和有界恢复；再次派同能力成员通常没有新信息。
- 可并行的解题工作：实现互不干扰的组件、查证一个独立假设、构造反例。允许 Leader 根据预期收益选择成员或自己完成。

D-41 已修复 full_auto 的执行语义，也补充了提示词；尚不能说明其他 83 题已消除这些行为。仍应完善结构化 Shell 回执（退出码、是否真正启动、超时原因、工作目录、输出引用），让失败不只藏在自然语言输出末尾。永久错误与普通命令非零退出要分别处理。

**发现三：永久启动失败可以形成调度风暴，耗尽全局预算（P0）**

`repair/qemu-startup__AiinXJg/agent/teamagents.jsonl` 第 54 行新增 `runtime_kind=codex` 的成员，但容器没有 Codex。第 62 行起，约 **10.802 秒产生 998 个 `run_failed`**，错误全部是 `cannot start codex app-server: No such file or directory`；最终触发 `max_turns_per_goal=1000`。这是运行时问题，不是 998 次模型推理，也不是模型主动调用了 998 次重试。

当前代码仍保留对应的触发链：

1. `engine/src/codex.rs::applied_delivery_ids` 只返回确认送达外部执行器的输入；启动失败时没有确认。
2. `engine/src/runtime.rs::finalize` 仅确认上述已应用的 delivery。
3. `core/src/control.rs::finalize_run_inner` 只 ack 传入的 delivery；失败后成员回到 Idle。
4. `schedule_inner` 看到未消费输入和无活动回合，可以立即安排新回合。

“不把未送达输入当已消费”的原则应保留，但需要给不可重试的启动错误增加失败停放状态：保留原输入、只产生一次可处理的错误通知、等待运行时配置修复或明确重试；对临时传输错误实施有次数和截止时间的退避。新增成员时应验证执行器能力。修复验收应证明缺失二进制不再生成新回合风暴，同时输入没有丢失，恢复后也不会重复执行副作用。

**发现四：完成状态与交付正确性脱节，复核容易重复错误假设（P1，最直接的解题质量问题）**

原始基线有 64 题返回 `completed`，其中 16 题 reward=0，即完成声明中有 25% 未获 grader 接受。替换六题后，剩余零分里仍有 11 个来自旧版的 `completed`。不能据此说“模型完全不验证”：不少任务确实跑过检查，但检查了错误环境、错误解释或不完整的条件。

| 任务 | 轨迹说明的问题 | 应改变的验证方式 |
| --- | --- | --- |
| `multi-source-data-merger` | `signal_done` 的 summary 明写 `STATUS: BLOCKED`，承认没有生成最终文件，运行时仍落 completed；旧 sandbox 看不到 `/data` 是前置原因 | 明确区分 blocked 与成功；完成申请携带实际交付物及检查回执，不能以交付了说明文档替代任务产物 |
| `chess-best-move` | 用户要求列出所有获胜走法；Leader 与 helper 一致只给一个，漏解仍完成 | 先验证原任务的“所有”要求，用枚举或反例寻找漏项；helper 不应只收到 Leader 提出的候选结论 |
| `dna-assembly` | verifier 声称 8/8 PASS；委派时 Leader 指定了退火区的切分方式，并给出预期 Tm；grader 测得一对差值 6.203812°C，超过 5°C | 从原始模板和最终引物重新推导实际匹配区；实现者的区域划分和期望数值只能是待检验假设 |
| `dna-insert` | helper 与 Leader 都认为所有约束通过，grader 测得温差约 5.72°C | 对多种可匹配边界构造反例，不能只在选定切分下重复计算 |
| `pytorch-model-cli` | 将单张图片看起来像某数字、程序输出同数字作为最终确认；grader 的预测一致性失败 | 和参考模型做多输入数值对照，验证权重布局、预处理和每层结果；视觉观感不是模型等价性的 oracle |
| `raman-fitting` | 自检拟合与图片能相互一致，但最终峰位与 grader 显著不同 | 在拟合前独立确认坐标列、单位、峰的物理含义；不能只验证自己选定峰的拟合残差 |
| `query-optimize` | 已验证结果一致，并相对原始慢查询大幅提速；仍不及隐藏 golden 的性能要求 | 这是继续搜索优化方案的空间，不应误称“没有测性能”；隐藏 golden 阈值没有写进用户任务，不能要求 Agent 事先知道 |

现有 `signal_done` 只接收 summary；`completion_blockers` 主要检查任务、活动回合、批准及未知结果。DP-12 已有“Leader 判断 + 硬约束”，但交付物正确性的硬约束还没有形成执行闭环。CLI `--check` 虽能在结束后检查并影响退出码，本次没有配置，而且失败结果没有自动返回工作循环继续修复。日志中 `verification=[]` 只表示没有这些 CLI 检查，不能据此否认模型自行运行的测试。

建议将“申请完成→运行验收→缺陷反馈→修复→重新验收”做成可追踪流程。验收必须来自用户任务或公开工程契约，不能来自隐藏 grader；记录执行命令、环境、退出码、产物版本和未验证项。静态审查、fixture 测试、真实目标环境测试应明确区分。交付物修改后，应失效与之相关的旧检查结论。

独立审查应获得原始需求、输入和待检验产物，并自行选择反例；避免将实现者推导的算法解释和期待结果预先设成审查标准。增加一个相同假设的模型回合，并不自动增加验证的独立性。

**发现五：缺少共享截止时间、及时交付和无进展反馈（P1）**

- 适配器未默认把 Harbor 的任务截止时间传给 CLI；CLI 默认 1200 秒，core 回合默认也为 1200 秒，而任务自身时限并不相同。代码有超时终止，但 Leader/成员的常规上下文没有统一的剩余任务时间。
- `gpt2-codegolf` 超时时仍在 `/app/_work/full.c` 上调试，规定的 `/app/gpt2.c` 未生成；`gcode-to-text`、`extract-moves-from-video` 也没有最终输出。不能声称尽早写文件就会正确，但系统可以推动更早建立并持续更新可执行的候选交付物。
- `winning-avg-corewars` 已出现通过自定初步阈值的候选，随后一次搜索命令占用 900 秒并超时。最终 verifier 自身也失败，故不能据此说“本来一定会通过”；可以确认的是搜索没有有效受剩余预算约束。
- `regex-chess` 和 `tune-mjcf` 因模型流读取超时终止；`mailman` 因模型请求 DNS 错误终止。当前请求 timeout=120、max_retries=5；没有完整逐次重试记录，无法断言每次失败的具体原因和重试消耗。

应让外层任务截止时间贯穿调度、模型调用、工具和子任务；规划时保留验收及收尾时间，长命令支持运行句柄与增量输出，避免把整个剩余窗口交给一次阻塞调用。结合耗时、失败类型、产物变化和新证据判断是否换路线或停止无效委派。重试上限应受总截止时间约束，不应只靠每回合次数。

对长推理，应区分首包等待、流读取停滞和整体时间预算；保留已确认执行结果，不能在恢复模型响应时盲目重放工具。日志没有显示普遍撞到 200 模型步上限；唯一明确的 `limit_reached` 是上述 1000 回合风暴。因此没有证据支持把默认步数全面增大当作首要修复。

**协作模式的实际证据：应优化选择质量，不追求更多成员**

以下分组只按轨迹中是否出现 `assign_task` / `run_subagent` 区分，是观察统计，任务难度与环境故障没有控制，不能比较后就断言某种模式更强：

| 观察到的路径 | 题数 | 原始通过数 |
| --- | ---: | ---: |
| 使用团队任务委派（可同时使用私有 helper） | 25 | 14 |
| 只使用私有 helper、没有团队任务委派 | 26 | 17 |
| 未使用上述两类委派 | 38 | 22 |

六题修复复测均未调用 `assign_task` 或 `run_subagent`，是合法且有效的单人成队路径。原始 36 次私有 helper 调用中可见大量 Shell 探测；实现上的 `run_private_subagent` 是同步的嵌套循环，共享父回合步数和执行权限，不会因为叫了 subagent 就自动获得墙钟时间上的并行收益。它仍可能通过隔离上下文改善判断，这一点要实验验证。

也有值得保留的正例：`sam-cell-seg` 中 API 和几何/输出审查成员报告了吞异常、CSV 表头改写、重叠边界和 NumPy 切片保留整幅图像等具体缺陷；后续代码有修改、复审，最终题目通过。这个轨迹支持“成员能提供可操作的新信息”，尚不证明没有成员就不能通过。`cancel-async-tasks` 的独立验收还使用真实子进程 SIGINT 检查清理行为，是有价值的验证方式。

因此不建议强制每题组队，也不建议取消团队能力。应保留 Leader 自主完成、独立审查、并行实现等路径，并记录每次委派的目的、交付物、独立性及采纳结果。故障诊断不得无限复制；紧密耦合的修改不宜为了组队拆开；可并行任务应有清晰的写入范围与接口。当前没有足够证据将共享文件竞争列为本次主要根因，不宜优先做一套复杂的合并机制。

**可观测性缺口（P0/P1）**

当前 `exec --json` 中工具参数只保留前 500 字符、结果前 2000 字符（`chat.rs::bounded_arguments/bounded_result`）。这对界面预览合理，但不足以承担完整评测轨迹：长脚本、完整模型响应、私有 helper 的推理过程与每次 API 重试都不能从这些行还原；工具行也没有可用于计算各次耗时的起止时间。不能把这里的预览截断误称为模型上下文截断。

应将界面预览和可审计轨迹分开，保留脱敏的完整工具请求/结果或稳定附件、模型请求配置、响应/工具边界、增量 usage、重试及中断原因；按 Agent 和任务记录产物变更与验证结果。请求头、凭据不进入记录。支持导出标准轨迹，先让评测可靠复算，再用它指导策略优化。

**改进顺序与验证方式**

| 顺序 | 工作 | 怎样证明有效 |
| --- | --- | --- |
| 1 | 修复任务 cwd、永久启动失败重派、结果与 verifier 错误分类；补全轨迹；审计答案暴露 | `prove-plus-comm` 能在原始工作目录启动；缺失执行器只产生有界失败且输入不丢；中断也有非零的已知用量；污染样本单列 |
| 2 | 在 D-41 基础上补齐结构化执行结果、环境身份与长任务生命周期；传递统一截止时间 | 在同一真实任务环境交付并复验；故障不再被识别成正常工具成功；任务结束前能停止无收益搜索并保存状态 |
| 3 | 建立完成申请与可执行验收闭环，改进独立审查输入 | 明确 blocked 的任务不会落成功；每个通过检查可追溯到产物版本；审查可以推翻实现者假设并推动修复 |
| 4 | 用受控对照优化何时独立、何时委派、何时审查 | 在相同资源和模型配置下提高实际通过率，或在相同通过率下降低成本/耗时 |

对照至少应包括：A，可靠工具上的单 Leader；B，当前自主组队策略；C，同一策略加上完成/验收闭环；D，在 C 上改进按需委派与独立审查。A 是 TeamAgents 自身的一种合法运行模式，而不是排除在 TeamAgents 之外的对照。

用同一数据集快照、同一模型与推理强度、原生 1M 上下文、相同任务时限和资源；先在开发子集诊断，最终独立全量重跑。不能把各版成功题择优拼接当新版本全量成绩。为了对齐官方可使用其 N=3 口径，报告每次运行、均值和配对的不确定性，不做 best-of-3。还应单列固定总 token/费用预算的对照，避免多成员以更多推理消耗获得的收益被误称为等成本增益。

核心指标是任务成功率、失败种类、错误完成率、总成本、墙钟时间和有效修复；辅助指标是委派后产生新证据/被采纳修复的比例。成员数量、工具调用数量、写了多少报告都不是成功指标。

其中永久错误停放属于现有运行可靠性的修复；完成协议、预算传递、验证闭环和策略接口若超出现行方案，需要按仓库约定确认后记入 DECISIONS，再实施。本轮未将这些建议直接写进产品代码。

**剩余 30 个零分的逐题观察**

下表是修复前轨迹的诊断，不预测新版得分，也不将多重因素强行归成唯一根因。L 为对应原始 JSONL；V 为 verifier 日志快照。启动失败无 L。

| 任务与证据 | 观察与改进方向 |
| --- | --- |
| `adaptive-rejection-sampler` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/adaptive-rejection-sampler__svDHDyh/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/adaptive-rejection-sampler/test-stdout.txt) | 超时；大量工作用于在 /app 重建 R 运行环境，最终 ars.R、Rscript 和样本文件不满足验收。先解决环境，再验证算法。 |
| `build-cython-ext` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/build-cython-ext__SzFT45t/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/build-cython-ext/test-stdout.txt) | 超时；把包装入 sandbox 的临时 HOME，成员仍在复验安装；grader 侧无法 import pyknotid。验证环境与交付环境不一致。 |
| `caffe-cifar-10` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/repair/caffe-cifar-10__ATUJnFR/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/caffe-cifar-10/test-stdout.txt) | 1200 秒回合上限；grader 侧缺 libglog.so.1，模型文件和训练输出缺失。环境准备吞掉执行预算。 |
| `chess-best-move` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/chess-best-move__2mkxNnV/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/chess-best-move/test-stdout.txt) | completed；漏掉另一获胜走法。helper 与 Leader 的一致意见没有覆盖用户要求的穷尽性。 |
| `compile-compcert` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/compile-compcert__xMvNXNM/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/compile-compcert/test-stdout.txt) | completed；grader 找不到 /tmp/CompCert/ccomp。大量工作绕过临时 /tmp 和安装限制，未形成目标环境的持久交付。 |
| `dna-assembly` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/dna-assembly__aRg8N46/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/dna-assembly/test-stdout.txt) | completed；独立成员报告 8/8 PASS，但实际退火区域定义未被独立挑战，grader 的温差断言失败。 |
| `dna-insert` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/dna-insert__7PWXTUf/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/dna-insert/test-stdout.txt) | completed；helper 复核仍漏掉退火区边界问题，grader 的温差断言失败。 |
| `extract-moves-from-video` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/extract-moves-from-video__ZEoFbGR/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/extract-moves-from-video/test-stdout.txt) | 1200 秒超时；存在重复 Shell 故障、帧/图像探索，最终 solution.txt 缺失；不能仅凭工具次数分离环境与视觉推理的贡献。 |
| `filter-js-from-html` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/filter-js-from-html__XNkhyok/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/filter-js-from-html/test-stdout.txt) | 1200 秒回合上限；已有实现和审查，但仍有对抗输入未过滤。属于实现正确性与有限时间内修复能力的共同问题。 |
| `gcode-to-text` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/gcode-to-text__JUeNZnz/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/gcode-to-text/test-stdout.txt) | Harbor 超时；大量读取、绘图/看图，最终 out.txt 缺失。需要尽早建立候选答案并控制探索时间。 |
| `git-multibranch` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/git-multibranch__6PgZKfM/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/git-multibranch/test-stdout.txt) | Harbor 超时；多成员在替代目录和单次 shell 内验证组件；实际 HTTPS/SSH 部署链路未通过。 |
| `gpt2-codegolf` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/gpt2-codegolf__Yx3999h/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/gpt2-codegolf/test-stdout.txt) | Harbor 超时；仍在 _work/full.c 上调试，目标 gpt2.c 缺失。环境损耗之外，还需候选交付与数值差异定位。 |
| `hf-model-inference` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/hf-model-inference__DYozrk2/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/hf-model-inference/test-stdout.txt) | completed；grader 连接 5000 端口被拒绝。与旧服务生命周期限制直接相关，尚未在修复版复测。 |
| `install-windows-3.11` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/install-windows-3.11__qzkH5ir/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/install-windows-3.11/test-stdout.txt) | 1200 秒超时；grader 看不到 QEMU、VNC 端口及 monitor socket。旧 PID/tmp 生命周期限制与任务耗时并存。 |
| `mailman` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/mailman__8EETpAa/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/mailman/test-stdout.txt) | 模型 API DNS 错误终止；grader 也连接不到邮件服务。不能把传输中断与服务部署问题混成一种失败。 |
| `mcmc-sampling-stan` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/repair/mcmc-sampling-stan__f3wdQdH/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/mcmc-sampling-stan/test-stdout.txt) | 1200 秒超时；grader 侧 rstan 未安装，采样输出也不符合要求。需要在目标环境完成依赖和实际采样验证。 |
| `multi-source-data-merger` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/multi-source-data-merger__dG69EWb/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/multi-source-data-merger/test-stdout.txt) | completed，但 summary 明确 BLOCKED 且没生成结果；旧 sandbox 看不到 /data，fixture 测试代替不了真实数据执行。 |
| `prove-plus-comm` [启动日志](tmp/tb21-design-evidence/prove-plus-comm/trial.log) / [V](tmp/tb21-design-evidence/prove-plus-comm/test-stdout.txt) | 适配器启动失败：镜像没有 /app；没有任何 TeamAgents tool 记录。先修默认 cwd 发现与启动失败上报。 |
| `pytorch-model-cli` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/pytorch-model-cli__khPWFYQ/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/pytorch-model-cli/test-stdout.txt) | completed；6 项中预测一致性失败，单图视觉确认不足以证明复现模型。 |
| `qemu-alpine-ssh` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/repair/qemu-alpine-ssh__YtX5vsS/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/qemu-alpine-ssh/test-stdout.txt) | incomplete；所有 31 次 shell 都含 bwrap 错误；verifier 另有依赖 404、uvx 缺失，未正常运行断言。 |
| `qemu-startup` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/repair/qemu-startup__AiinXJg/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/qemu-startup/test-stdout.txt) | failed；执行器不可用引发 998 次失败回合；verifier 另有依赖 404、uvx 缺失。双重问题，不能推断修复后直接通过。 |
| `query-optimize` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/query-optimize__LtcUEkf/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/query-optimize/test-stdout.txt) | completed；结果检查通过，但性能未达到 hidden golden 的要求。确实做过计时，需要更强优化搜索，不能声称完全没验证。 |
| `raman-fitting` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/raman-fitting__4SAUkak/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/raman-fitting/test-stdout.txt) | completed；两组峰参数与 grader 不符。自洽的图像/拟合残差没有保证选对坐标与目标峰；具体语义根因还需独立输入解析检查。 |
| `regex-chess` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/regex-chess__PaNLPhE/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/regex-chess/test-stdout.txt) | 模型流读取超时后 failed；最终 re.json 缺失。需要传输恢复、推理预算与渐进落盘，不宜直接认定模型无法解题。 |
| `sanitize-git-repo` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/sanitize-git-repo__LUfNbtn/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/sanitize-git-repo/test-stdout.txt) | completed；清理中重写并回收历史，grader 比较所需旧 SHA 无法解析。需最小范围修改和保留可验证基线；仅凭此不能断言工作树存在额外改动。 |
| `sqlite-with-gcov` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/sqlite-with-gcov__jmhqEFi/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/sqlite-with-gcov/test-stdout.txt) | completed；成员构建并复核了 workspace 产物，但 grader 的 PATH 中找不到 sqlite3。交付位置和运行环境没有达成一致。 |
| `torch-pipeline-parallelism` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/torch-pipeline-parallelism__qnACH6P/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/torch-pipeline-parallelism/test-stdout.txt) | Harbor 超时；下载并执行过上游隐藏测试，轨迹受污染；正式 verifier 又因依赖下载超时未完成，不能据零分归因于算法。 |
| `train-fasttext` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/train-fasttext__8hdZinf/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/train-fasttext/test-stdout.txt) | 1200 秒超时；曾委派相同 Shell 探测，随后准备环境和数据，最终 model.bin 缺失。需减少前置环境损耗并控制训练/验证预算。 |
| `tune-mjcf` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/tune-mjcf__NM8jCDn/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/tune-mjcf/test-stdout.txt) | 模型流读取超时后 failed；grader 的速度目标未达成。应分别报告未完成优化与模型传输中断。 |
| `winning-avg-corewars` [L](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/run1/winning-avg-corewars__k2wiYCA/agent/teamagents.jsonl) / [V](tmp/tb21-design-evidence/winning-avg-corewars/test-stdout.txt) | 1200 秒超时；有初步候选后仍跑了 900 秒搜索；正式 verifier 下载 uv 失败。存在预算问题，但不能认定已有候选一定能过。 |
