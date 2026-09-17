# 固定任务评测

`tasks/<id>/` 每个任务三件套：`prompt.md`（真实提示词）、`checks.txt`（每行一条验收命令，
在隔离 Shell 里按顺序执行）、可选 `fixture/`（先拷进工作目录的初始文件）。
两阶段任务（`resume.md`，用于"中断后继续"）：阶段 1 用 `prompt.md` 与 `timeout.txt`（`expect.txt`
是它的期望退出码，通常 124），随后自动 `--resume` 同一个会话跑 `resume.md`（`expect-resume.txt`
是阶段 2 的期望码，默认 0），汇总表只统计阶段 2。注意阶段 1 的超时要宽于"第一步做完"的时间，
否则被中断的是还没开始的工作，任务会变得不稳定。

可选覆盖项：`mode.txt`（`full-auto` 默认 / `approval` 不带 `--full-auto`）、`timeout.txt`
（该任务的秒数）、`expect.txt`（期望退出码——考的是 CLI 契约而不是产出文件时用它）。

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

## 独立隐藏评分

任务可提供 `hidden_tests.rs` 与 `allowed-files.txt`，后者逐行列出允许修改的现有 `src/*.rs`。
Agent 退出后，评分器检查其余 fixture 文件逐字节不变，并拒绝额外文件、符号链接与特殊文件；
根目录 `target/`、`.git/` 排除。评分只将允许的源文件叠加到可信 fixture 副本，再注入隐藏测试。
候选编译与测试通过既有无网络 bubblewrap 执行；隐藏测试不进入 Agent 工作区。

结果保存在 `<id>.grade.json` 与 `<id>.grade.log`。评分失败、隔离不可用、测试提前退出、
报告缺失或不明确通过，均令 runner 返回 1；恢复任务仅在第二阶段结束后评分。
具体实现与边界见 [隐藏评分记录](../hidden-grader-2026-09-17.md)。

## 任务集

| 任务 | 考什么 |
|---|---|
| `rust-fix` | 修一个失败测试的 Rust crate（`cargo test` 验收；同时验证沙箱里工具链可用） |
| `rust-ledger` | 多文件收付款程序：精确金额、CSV、幂等、失败原子性与 CLI；11 项独立隐藏测试并检查用户文件保护 |
| `edit-integrity` | 只改指定段落里的同名项，其它段落必须原样（验收脚本逐段断言） |
| `long-output` | 命令输出超过 200KB 预览上限，必须从完整输出里取值（考制品/分页读取路径） |
| `team-collab` | 两个独立子任务：要求 Leader 自己组队（add_agent + assign_task）并行完成后汇总，考组队与成员执行链路 |
| `approval-gate` | 需要批准的操作在非交互模式下必须停在待批准（exit 3）、不执行、也不伪造成功 |
| `interrupted-recovery` | 任务被超时中断：副作用停在中途、进程被杀、不得声称完成（exit 124） |

## 已知缺口

供应商矩阵需要对应凭据；会话级磁盘配额、跨会话同目录写入仍需治理（同会话成员已有跨进程文件锁）。
多数固定小任务使用公开检查；`rust-ledger` 提供隐藏验收，但单项小型仓库仍不能代表大型工程完成率或竞品对照。
本轮实际结果见 [2026-09-17 记录](runs/2026-09-17-coding-review/REPORT.md)。
新增多文件任务的原生上下文结果见 [rust-ledger 评测](runs/2026-09-17-rust-ledger/REPORT.md)。
