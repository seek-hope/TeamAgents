# TeamAgents coding agent 能力与评测审查

审查日期：2026-09-17。范围：TUI/CLI、交付验证、现有真实模型评测，以及 Codex CLI、Claude Code、pi、Hermes 官方资料对照。产品实现只读检查；另按 D-32 修复评测脚本并新增无凭据回归探针。此文件不授权超出实施方案的新功能。

当前结论：TeamAgents 已有可用于编码的文件/Shell 工具、权限、持久回合、多成员委派、模型切换、恢复、上下文压缩、结构化执行与验收命令。现有证据不足以证明已达到或超过四个成熟产品：已有模型实测集中在 DeepSeek 与短小固定任务；竞品资料只证明其公开能力，不证明相对任务完成率。

本分项审查没有调用真实付费模型，没有运行四个竞品做任务竞赛。主审随后运行的 TeamAgents 真实任务见 [本轮评测记录](eval/runs/2026-09-17-coding-review/REPORT.md)；这些记录不构成竞品对照。官方页面于本轮直接抓取，详细来源及可复跑证据补充如下。

## 优先缺口

1. **评测脚本不能阻止失败发布（本轮已修复）。** 原 `review/eval/run.sh` 记录失败但没有累积失败退出状态；末尾成功的 `echo` 使整批返回 0。用只输出失败 result、退出 1 的假二进制离线复现，原脚本退出 0。本轮让实际返回码、JSONL 结果、全部验收命令与两阶段期望共同决定整批状态；未知任务返回 2，失败返回 1，符合预期的批准/超时任务仍通过。
2. **编码质量没有足够的盲验。** `rust-fix` 的输入仅一个把减法改加法的函数；检查命令在同一可编辑工作区执行，模型可以修改公开测试。应保留现有契约任务，另外加入仓库级、隐藏测试、用户脏改动保护与重复运行的编码任务。
3. **交付审查不完整。** TUI `v` 只显示所选成员最近一次 `edit_file` / `edit_files` / `write_file` 返回文本；Shell、外部 Codex、多个编辑批次、恢复后的完整变更不等于这一视图。需要以明确基线与当前工作区的真实差异来形成交付审查证据。
4. **团队工作过程仍难完整检查。** 已有工具日志、状态、批准和计划，但没有完整成员对话/工具历史浏览；此项在方案与验收表中已经明确待验收。应先补足现有产品承诺，并维持成员间私有上下文权限边界。
5. **日常开发环境与长任务缺乏矩阵验证。** 沙箱里 Rust 工具链已覆盖，HOME 下的 nvm/pyenv 等开发环境仍是已知缺口；多供应商、真实上下文压缩、持续开发服务、跨步骤纠错的真实验收不能由离线单测数量代替。

## 基准与授权

实施基准：`TeamAgents-Implementation-Plan.zh-CN.md` §1、§12、§13、§17、§18；`docs/DECISIONS.md` D-32 已授权稳定性、流式/上下文/恢复、交付验证、结构化 CLI 与回归评测改进。保留 Rust 三 crate、core 单事务权威、操作级批准、成员隔离与已有会话兼容性。新的产品范围（跨平台、跨会话记忆、插件市场等）只是候选，不能因竞品具有该能力而自动引入。

## 官方资料来源

以下资料于 2026-09-17 抓取，属于公开接口/功能说明，不作为本地实测成绩：

- Codex CLI：<https://developers.openai.com/codex/cli/features/>（当前页面重定向至 CLI 总览；规范入口 <https://developers.openai.com/codex/cli/> 实际到达 <https://learn.chatgpt.com/docs/codex/cli>，包含交互、`codex exec`、会话恢复、图像、子代理、MCP、权限及专用代码审查）。
- Claude Code：<https://code.claude.com/docs/en/overview>（编码、终端、CLAUDE.md、Skills、Hooks、MCP、多代理和非交互调用）。
- Claude Code 检查点：<https://code.claude.com/docs/en/checkpointing>。
- pi：<https://github.com/badlogic/pi-mono/blob/e98f287ee498e0116546f4e9aa083fdec9793cd2/packages/coding-agent/README.md>（实际抓取 raw README；以当时 main SHA 固定并再次抓取比对一致；基础四工具、树状历史、压缩、消息队列与扩展）。
- Hermes：<https://github.com/NousResearch/hermes-agent/blob/98f758ae7e8db83c2bb9214c3b35adf41df15f03/README.md>（同样固定并复核 SHA；终端、委派、Skills/记忆、多终端执行后端与模型配置）。
- Hermes 工具：<https://hermes-agent.nousresearch.com/docs/user-guide/features/tools/>。

## 能力对照及其实际含义

“文档列出”只代表本轮抓取的官方材料这样描述；“未核对”不等于产品不支持。TeamAgents 一栏来自源代码、方案及已保存的历史证据。本轮没有安装、升级或执行竞品。

| 每日编码能力 | TeamAgents 当前状态 | 官方资料能支持的竞品参照 | 建议与验收门槛 |
|---|---|---|---|
| 文件编辑、Shell、测试闭环 | 文件读写/搜索、CAS 编辑、多文件编辑、Shell；`exec --check` 收集真实命令状态 | Codex CLI、Claude Code 官方总览列出读代码、修改、执行与验证；pi 默认 `read/write/edit/bash`；Hermes 列出 `terminal/process/read_file/patch` | 先用相同仓库任务量出可靠率，工具数量不能证明修复质量 |
| 交付前改动审查 | `App::record_review` 只保留每成员最新文件工具结果；`replay_log` 清除该缓存；无工作区完整差异视图证据 | Codex 有专用审查：未提交改动、提交、基线分支；Claude Code 有可视化 diff 与文件工具检查点 | 增量交付里展示所有受影响文件、真实 diff、测试结果；Shell/外部成员造成的修改也计入，恢复后仍可查看；不得把共享文件修改武断归到某成员 |
| 撤回与重新尝试 | D-26 对话树、rewind、fork；只回退模型记忆，不回滚文件和团队事实 | pi `/tree` 可检索全树并切分支；Claude Code 支持单独恢复代码/对话 | 完善分支可发现性属于已有树历史体验；新增文件撤销契约另行设计确认，保持当前用户改动与副作用边界 |
| 长任务与上下文 | 自动遮蔽/压缩、`read_history`、计划、用量与恢复；真实长会话质量尚无统一对照 | pi 手动/自动压缩及全树浏览；Codex 会话恢复与子代理；Claude Code 会话/恢复；Hermes 跨会话检索与记忆 | 跨压缩后继续修复、不丢用户要求、不重复执行副作用；记录失败恢复所需人工介入次数 |
| 团队并行与工作隔离 | 显式 ACL、共享空间、持久任务、worktree、Codex 成员；有 DeepSeek 短任务及中断恢复历史 | Codex 子代理；Claude Code 多代理；Hermes `delegate_task`；pi README 明确无内建子代理，推荐扩展或实例编排 | 这是 TeamAgents 可继续强化的方向；用同总预算证明并行带来完成率/耗时收益，并测试冲突、任务接管和取消后的状态收敛 |
| 开发工具与进程环境 | Rust 沙箱工具链已有覆盖；`shell_run_stateful` 保留 cwd/export，但 `stdin=null`，每次独立 bwrap，无持久进程句柄/交互输入工具 | Hermes 明确提供后台 terminal 与 process 的 poll/wait/log/kill；pi 默认 bash，本文未核对其交互进程扩展 | 增加真实 Node/Python 项目验收与清晰 doctor 提示；如需 dev server/PTY，先定义进程生命周期、批准和恢复契约，不能靠静默取消隔离解决 |
| 项目规则与扩展 | Chat 注入当前 cwd/用户 AGENTS.md 与显式 instruction_files；Skills、MCP stdio/HTTP、hooks；不等于递归目录规则自动生效 | Codex Skills/插件/MCP；Claude CLAUDE.md/Skills/hooks/MCP；pi 自动读取父目录规则、有扩展/RPC；Hermes Skills/MCP/记忆 | 补测从子目录启动、不同工作区与成员的指令边界；层级规则和新增跨会话记忆必须分别说明语义，不能把记忆作为通用稳定性的替代物 |
| 自动化输出 | `exec --json` 的 session/tool/event/result、验证及用量；本轮修复 runner 失败状态 | Codex `exec`、Claude `-p`、pi JSON/RPC；Hermes 本轮仅核对 CLI/工具说明，未核对等价结果 schema | 错误结果、批准、超时、缺失最终记录都应可靠影响任务状态与 CI 退出码 |

竞品限制也应保留：Claude Code 检查点文档明确不覆盖 Bash 修改，通常不恢复其他子代理的修改，也不替代 Git；pi README 明确不内建 MCP、子代理、权限弹窗或 plan mode，依赖扩展/外部隔离完成相应工作。这说明“水平相当”应按任务和交互结果定义，而非凑齐每个产品的功能列表。

## 已有真实证据与剩余不确定性

`review/eval/runs/2026-09-15-deepseek-lean/REPORT.md` 记录了 13 个固定任务，含文件编辑、Rust 修复、长输出读回、协作、批准、中断、恢复与 Codex 成员。相应 JSONL 已保存。这是可取的服务闭环基础，但它们使用相同任务的单次运行，且没有四个竞品的成对结果；不能外推一般代码库上的排名。

特别是 `rust-fix/fixture/src/lib.rs` 只有一个错误减法函数，`team-collab` 是两个独立的小函数，公开测试位于模型可编辑的工作区。验收命令有返回 0 的证据，但尚不能排除修改公开测试导致的误判。`edit-integrity` 验证了三个段的 retries 值，不证明所有其它字节完全不变。下一轮必须把公开契约测试和独立隐藏评分分开。

凭据口径：本分项审查未检查或导出本机密钥，也未发起模型请求。主审已补跑本机配置的 DeepSeek 任务；T7 五家供应商的真实完成情况仍以 `docs/ACCEPTANCE.md` 的逐项记录为准，不能把单一供应商的冒烟外推成五家验收或竞品比较。

## 公平比较方案（待运行）

先固定任务、评分和版本，再运行；不得看过某一工具的成绩后专门调整其余工具提示词。

1. **同模型轨与产品默认轨分开。** 同模型轨固定服务端实际模型 ID/版本、供应商端点、推理档位、上下文与输出上限。仅纳入已验证支持该模型/协议的工具；如果 Claude Code 或 Codex CLI 不支持同一路径，标记 N/A，不能悄悄换模型。若使用协议网关，五方都使用相同网关并记录转换限制。各产品推荐默认模型另作产品体验轨，不能用于证明 harness 优劣。
2. **同任务。** 建议先固定 20 个仓库级任务：6 个真实缺陷修复、4 个跨文件功能、3 个重构与兼容性、3 个长上下文/中断恢复、2 个脏工作区与并发冲突、2 个权限/拒绝与结果诚实性。每项固定仓库 commit、用户未提交补丁、相同初始文件、提示词和网络/依赖快照；分别覆盖 Rust、Python、TypeScript。现有 13 个小任务继续作为协议回归，不充当全部编码基准。
3. **同预算。** 每项设相同墙钟上限、总输入/输出 token 或成本上限；并行成员的预算求和，不能给 TeamAgents 每个成员各一份总预算。记录系统提示及模型调用开销、缓存读写、压缩调用和失败重试。先测单 agent 基线，再在相同全队预算与并发上限下测团队价值。推理 token 不可得时标为缺失，不能假定为 0。
4. **同环境与权限。** 在相同硬件/依赖镜像、任务 workspace 和允许网络下执行；保持原生提示与工具，使测量对象仍是完整 harness。不可对一个产品给全主机权限、对另一个禁止依赖访问。不能配齐的权限或后台能力单列差异。
5. **多轮次。** 每个任务每个产品至少 3 次独立新会话，发布级比较建议 5 次；随机交错顺序。单次“成功/失败”的单位是完整任务，失败与超时必须纳入分母，不反复重跑到成功再保留最好一条。恢复任务按同一试验的两阶段计入总耗时/成本。
6. **独立评分。** 隐藏测试与评分程序放在 agent 不可访问且不可修改的评测环境中，评分前记录候选 diff，随后只把产物交给固定 grader。检查公开测试/配置删改、用户原有补丁保护、范围外文件变化及残留进程；人工盲审可维护性，但不覆盖失败的功能门槛。原测试保留成功也不能替代隐藏行为测试。
7. **报告指标。** 主指标是隐藏测试全部通过且没有越界/测试篡改的任务完成率；同时报告 p50/p95 延迟、每个成功任务的总成本（含失败尝试）、工具错误率、人工介入次数、恢复成功率、权限违规数。按任务配对报告差值与置信区间；20 个任务、少量重复只能用于初始排序，区间重叠时不声称领先。

每次原始记录至少保存：agent 名称/版本/commit、模型实际 ID、供应商、推理设置、任务版本、环境摘要、权限配置、预算、起止时间、完整脱敏 JSONL、所有阶段退出码、最终 diff、公开/隐藏评分、usage 与成本可得性。密钥永不进入报告。

建议执行顺序：本轮完成 runner 可靠退出；下一批完成完整变更审查和仓库级隐藏评分；随后跑真实多模型/长任务矩阵，再根据最常见失败类型改工具、上下文或调度。跨平台、插件市场、自动记忆、后台进程/PTY 等新契约按实际收益另立决策；不以扩张范围替代现有首版验收。

## 本轮评测脚本修复与复核

改动仅涉及 `review/eval/run.sh`、`review/eval/check-runner.sh` 和本报告。`run.sh` 当前保证：

- 无匹配任务/缺失参数返回 2，不能空跑后报告成功。
- 默认任务期望退出 0；显式 `expect.txt` 的批准/超时属于正常契约，符合期望可通过。
- 两阶段任务的第一阶段失败也导致整批失败；缺 session_id 不启动无意义的恢复。
- 缺失/破损最终 JSONL、进程与 result 退出码不一致、缺少或失败的验收命令都返回 1。
- 汇总只使用本次实际执行的文件；复用 `--out` 时，旧 JSONL 不冒充新证据。

离线复核命令：

```bash
bash -n review/eval/run.sh
bash review/eval/check-runner.sh
git diff --check -- review/eval/run.sh review/eval/check-runner.sh review/capabilities-coding-agent-2026-09-17.md
```

本轮结果：语法检查通过；14 项 runner 契约检查通过；差异空白检查通过。探针使用临时目录和假 agent，仅验证评测脚本自身，**不是模型任务成绩**。覆盖正常完成、实际失败、验收失败/缺失、非法 JSON、缺最终记录、退出码矛盾、预期批准、两阶段成功/第一阶段失败/第二阶段失败/无 session、未知任务和缺失参数；每个有输出目录的样例另放一份破损旧 JSONL 证明不会污染本次汇总。

代码证据可复查：

```bash
rg -n 'record_review|reviews.clear|replay_log|on_tool_result' tui/src/app.rs
rg -n 'rewind_points|fork_session|chat_tree.json' engine/src/worker.rs
rg -n 'stdin\(Stdio::null|toolchain_mounts|shell_run_stateful' engine/src/tools.rs
rg -n 'for candidate|instruction_files' engine/src/session.rs
cat review/eval/tasks/rust-fix/fixture/src/lib.rs
cat review/eval/tasks/edit-integrity/checks.txt
cat review/eval/runs/2026-09-15-deepseek-lean/REPORT.md
```
