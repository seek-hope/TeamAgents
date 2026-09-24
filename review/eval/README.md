# 固定任务评测

> **状态（2026-09-24，R29 之后）**：本文描述的是 v1 固定任务运行器 `run.sh` 与其评分器，已随 R29 退役；`tasks/`、`runs/` 作为历史证据保留，不再复跑。
> 当前评测入口是 [`r2-p6/run.py`](r2-p6/run.py)（A/B/C 三组真实模型对照）与各阶段 `review/*.md` 记录，真实模型验证的口径见 [开发说明 § 真实模型验证](../../docs/DEVELOPMENT.md#真实模型验证)。

`tasks/<id>/` 每个任务三件套：`prompt.md`（真实提示词）、`checks.txt`（每行一条验收命令，
在隔离 Shell 里按顺序执行）、可选 `fixture/`（先拷进工作目录的初始文件）。
两阶段任务（`resume.md`，用于"中断后继续"）：阶段 1 用 `prompt.md` 与 `timeout.txt`（`expect.txt`
是它的期望退出码，通常 124），随后自动 `--resume` 同一个会话跑 `resume.md`（`expect-resume.txt`
是阶段 2 的期望码，默认 0），汇总表只统计阶段 2。注意阶段 1 的超时要宽于"第一步做完"的时间，
否则被中断的是还没开始的工作，任务会变得不稳定。

可选覆盖项：`mode.txt`（`full-auto` 默认 / `approval` 不带 `--full-auto`）、`timeout.txt`
（该任务的秒数）、`expect.txt`（期望退出码——考的是 CLI 契约而不是产出文件时用它）。

真实仓库任务可增加 `fixture-source.toml`，指定本仓库的完整 40 位提交与相对路径列表。
Rust 评分器从 Git 对象准备输入，不读取脏工作树，再叠加显式 `fixture/` 文件。准备失败时
runner 返回失败且不启动模型。任务根目录的隐藏检查、说明和参考材料不复制到工作区。

```bash
cargo build --offline --manifest-path engine/Cargo.toml   # 或被评测的版本
review/eval/run.sh --timeout 600                    # 全部任务；--only ID 只跑一个
review/eval/run.sh --out /tmp/evals/x --keep        # 指定输出目录
```

每个任务在**全新工作目录**里跑一次真实模型回合（`exec --json --full-auto`），结果写到
`$OUT/<id>.jsonl`（可复跑、可 diff）。脚本最后按 JSONL 打印一张汇总表：status、exit code、
耗时、真实 tokens（`result.usage`）与验收是否通过。

退出码：全部任务符合预期且验收通过返回 0；任一任务失败、缺失/损坏最终结果、验收失败或
两阶段退出码不符返回 1；参数错误或 `--only` 没有匹配任务返回 2。`expect.txt` 指定的
批准/超时退出码属于被验证的正常契约，不能直接把所有非零 agent 退出码都当作评测失败。
汇总只读取本次实际执行的结果，旧 JSONL 不参与评分。无需模型即可检查评测入口：

```bash
bash review/eval/check-runner.sh
```

报告纪律：没有实际运行的模型、命令或指标不得写进报告；模拟服务与被测对象要分开记录。
`runs/` 下按日期保存真实跑过的结果。
模型评测必须使用该模型的原生上下文长度，并记录数值及来源；未知时先核实，不得自行缩小
真实模型的窗口。DeepSeek Flash 按用户确认的 1M 配置（`context_window = 1000000`），见 D-36。

## 真实 Chat 模型冒烟矩阵

`engine/tests/live_models.rs::live_chat_model_matrix`（**该入口已随 R29 退役**，下述命令仅存档）提供统一的显式启用入口：
用 [TOML 清单](live-models.example.toml)选取已有 profile，逐一验证文件工具结果续接、
同会话 Shell、关闭重开后的历史与用量。精确模型名、原生窗口和来源必填，
保留原 profile 的请求设置与生成选项。报告随阶段保存；缺配置或凭据明确跳过，
任一条目未实际通过都会非零退出。默认 `make check` 仅运行五项离线契约，真实入口显示 ignored。
复跑命令见[开发说明](../../docs/DEVELOPMENT.md#真实-chat-模型矩阵)。

本批仅 DeepSeek Flash 原生 1M / high 实跑，三阶段通过，原始计数仍保留一次无效工具调用。
此类轻量协议冒烟不计入仓库任务完成率，不证明五家混队或通用竞品水平。
证据见[首批报告](runs/2026-09-19-live-models/REPORT.md)。

## 独立隐藏评分

任务可提供 `hidden_tests.rs` 与 `allowed-files.txt`，后者逐行列出允许修改的现有 Rust `src/` 源文件。
Agent 退出后，评分器检查其余输入文件逐字节不变、所有文件权限不变，并拒绝额外文件、
符号链接与特殊文件；根目录 `target/`、`.git/` 及可信输入中各 Cargo 清单旁的 `target/` 排除。
评分只将允许的源文件叠加到可信输入副本，再注入隐藏测试。
候选编译与测试通过既有无网络 bubblewrap 执行；隐藏测试不进入 Agent 工作区。

`grading.toml` 可指定 `manifest`（默认 `Cargo.toml`）、`public_tests`（默认 all-targets）
及 `timeout_seconds`（默认 120，范围 1–600）。可选 `config_home` 指向固定输入中的相对配置目录，
例如 `.config`；评分器将它作为评分副本内的绝对 `XDG_CONFIG_HOME`，不继承宿主 HOME/配置。
未声明时不自动发现配置；越界、缺失或文件路径在执行候选前拒绝，配置文件仍受最终文件保护。
提示词与 `checks.txt` 中的公开命令也须选择同一测试配置，不能只修正隐藏评分环境。
隐藏测试注入所选 crate 的 `tests/hidden.rs`。
输入仍受单文件 10 MiB、总量 64 MiB 与 4096 个文件限制。文件保护仅检查最终候选状态，
不表示模型执行全过程没有临时创建、删除或更改文件。

结果保存在 `<id>.grade.json` 与 `<id>.grade.log`。评分失败、隔离不可用、测试提前退出、
报告缺失或不明确通过，均令 runner 返回 1；恢复任务仅在第二阶段结束后评分。
具体实现与边界见 [隐藏评分记录](../hidden-grader-2026-09-17.md)。

## 任务集

| 任务 | 考什么 |
|---|---|
| `rust-fix` | 修一个失败测试的 Rust crate（`cargo test` 验收；同时验证沙箱里工具链可用） |
| `rust-ledger` | 多文件收付款程序：精确金额、CSV、幂等、失败原子性与 CLI；11 项独立隐藏测试并检查用户文件保护 |
| `repo-session-fork` | 真实 TeamAgents 历史仓库的会话分叉/切换修复：固定 Git 输入、3 crate、约 2.9 万行 Rust，7 项隐藏与 12 项相关公开检查 |
| `edit-integrity` | 只改指定段落里的同名项，其它段落必须原样（验收脚本逐段断言） |
| `long-output` | 命令输出超过 200KB 预览上限，必须从完整输出里取值（考制品/分页读取路径） |
| `team-collab` | 两个独立子任务：要求 Leader 自己组队（add_agent + assign_task）并行完成后汇总，考组队与成员执行链路 |
| `approval-gate` | 需要批准的操作在非交互模式下必须停在待批准（exit 3）、不执行、也不伪造成功 |
| `interrupted-recovery` | 任务被超时中断：副作用停在中途、进程被杀、不得声称完成（exit 124） |

## 已知缺口

供应商矩阵需要对应凭据；会话级磁盘配额、跨会话同目录写入仍需治理（同会话成员已有跨进程文件锁）。
多数固定小任务使用公开检查；`rust-ledger` 与 `repo-session-fork` 提供隐藏验收。
后者扩展到真实历史仓库，但仍不是陌生第三方仓库、多任务工程完成率或通用竞品排名。
`repo-session-fork` 的首批三次完整成功率为 0/3；两份允许源码单独评分可通过，但分别存在超时、
额外产物和 CLI 状态误判，不能改记成功。校准、失败轨迹与后续修复见
[真实仓库记录](runs/2026-09-19-repo-session-fork/REPORT.md)。
本轮实际结果见 [2026-09-17 记录](runs/2026-09-17-coding-review/REPORT.md)。
新增多文件任务的原生上下文结果见 [rust-ledger 评测](runs/2026-09-17-rust-ledger/REPORT.md)。
2026-09-19 已增加首个同任务、同模型的 Codex CLI 起步对照，并根据真实重复读回修复后复跑，
见[对照记录](runs/2026-09-19-ledger-comparison/REPORT.md)。客户端协议、Skills 与分工仍有差异；
单次小仓库通过不代表通用排名，Claude Code、pi、Hermes 和更大仓库仍待验证。
随后修复 Shell 目录恢复失败后在错误位置继续执行的问题，同条件独立复跑完成并通过最终验收，
见[Shell 恢复实跑](runs/2026-09-19-shell-cwd/REPORT.md)。最终文件保护不等于运行全过程从未越出指定修改范围，
轨迹中的临时改动和失败回执也必须保留。
2026-09-19 的 Chat 大窗口预算调整已通过本地实际请求回归和 `make check`；同条件真实历史仓库
对照因共享 `/tmp` 配额在完成前中止，没有产生可评分的 `result`，不得把它记作成功或失败样本，
见[上下文预算记录](../context-budget-2026-09-19.md)。
随后保持原任务和 1200 秒时限，把工作区/状态/评分临时目录放到磁盘上的 `review/tmp/`，
当前版首次完整通过：CLI completed/0，公开 12/12，隐藏 7/7，最终文件保护通过，
943.746 秒。该批仅 1/1，原三轮 0/3 不重写；没有证明稳定成功率或单项预算修复的因果效果。
原始轨迹、Shell 失败、允许源码补丁及复跑命令见[当前版仓库实跑](runs/2026-09-19-repo-current/REPORT.md)。
随后完成同题 Codex CLI 对照：首次尝试因读取工作区外隐藏测试作废；补充文件系统读取隔离并通过无模型探针后，
全新样本 **472.672 秒 / exit 0**，独立隐藏 7/7、公开 12/12、最终文件保护通过。
模型自身运行公开检查有一项受原生网络策略限制；该结果与独立评分分开报告。
与 TeamAgents 的 943.746 秒样本均为单次观测，工具、协议及沙箱差异仍在，不据此作通用排名。
原始有效/污染轨迹、条件和复核命令见[仓库同题对照](runs/2026-09-19-repo-codex-comparison/REPORT.md)。

工具 HOME 隔离后，当前 `repo-session-fork` 显式使用夹具 `.config`，避免旧公开测试读取错误目录。
同一参考补丁重新校准为隐藏 7/7、公开 12/12，原始缺陷快照仍为隐藏 2/7。
固定源码输入、行为要求、隐藏测试和时限未变，提示词与评分环境声明已修订；新样本不能与历史成绩直接合并。
本环境下已预登记并完成三次顺序实跑：完整交付 **3/3**，每次 CLI completed/0、隐藏 7/7、公开 12/12、
最终文件保护通过，CLI 分别 938.784 / 750.632 / 482.324 秒。三轮各有 7 次 Shell 非零退出，
额外全仓测试失败也保留；单任务三次成功不能替代其他仓库、任务或供应商验收。
输入冻结、全部轨迹、补丁重放和校验清单见[三次重复记录](runs/2026-09-19-repo-repeat/REPORT.md)。
