# 可信隐藏评分设计与验证（2026-09-17）

本轮在 D-32 与用户优先复杂编码任务完成率的授权内，为仓库评测补充独立隐藏评分。评分器使用现有 Rust engine 的 integration test，不增加产品命令或 crate。

- 任务约定为 `fixture/`、`hidden_tests.rs`、`allowed-files.txt`；后者逐行列出允许修改的既有 Rust 源码相对路径。
- 真实 Agent 完全退出后检查候选目录，原 fixture 中未允许修改的文件必须逐字节一致，禁止符号链接及特殊文件。`target/` 与 `.git/` 不参与内容评分，且绝不复制进评分目录。
- 从可信 fixture 创建独立临时目录，仅覆盖允许的源码，再注入 `tests/hidden.rs`。隐藏测试不会进入 Agent 工作目录或提示词。除忽略的构建目录外，额外候选文件在执行前拒绝。
- 通过现有 `tools::shell_run` 在 bubblewrap、无网络、固定超时内运行 Cargo；禁止宿主执行候选代码。评分 JSON 记录明确结果、原因、候选源码 SHA-256 与隐藏测试输出。
- `run.sh` 对带隐藏测试的任务自动评分；恢复任务只在第二阶段退出后评分。任何评分错误均令整体评测失败，其余任务维持现行退出码与 JSONL 校验契约。

验证至少覆盖：原始缺陷代码失败、正确修复通过、修改公开测试被拒绝、符号链接被拒绝。通用临时小项目独立于任务参考答案，不调用真实模型。

已落地：`engine/tests/eval_grader.rs`、`review/eval/run.sh` 与 `review/eval/check-runner.sh`。

可复跑验证：

```bash
cargo test --offline --manifest-path engine/Cargo.toml --test eval_grader -- --nocapture
bash review/eval/check-runner.sh
bash -n review/eval/run.sh review/eval/check-runner.sh
```

实测结果：Rust 评分器 4 项通过、环境入口 `grade_candidate` 明确忽略 1 项；旧 runner 契约 14 项通过，新增隐藏评分 runner 契约 6 项通过，评分报告校验契约 5 项通过。隐藏评分使用真实 bubblewrap，覆盖缺陷代码失败、修复通过、公开测试篡改拒绝、符号链接拒绝、新增 build.rs 在执行前拒绝，以及恢复任务只在第二阶段评分。报告校验覆盖缺失、畸形、`ok=false`、`ok=1` 均拒绝，仅 `ok=true` 通过，并验证旧的成功报告不能被复用。编译候选代码和运行测试均在隔离环境内，无真实模型调用。

已与 `rust-ledger` 夹具对接实跑评分：原始 fixture 隐藏测试 0/11 通过、评分失败；经夹具作者准备的参考修复副本隐藏测试 11/11 及公开测试 3/3 通过、评分成功。两份 JSON 临时证据分别为 `/tmp/teamagents-ledger-baseline-grade.json`、`/tmp/teamagents-ledger-reference-grade.json`；参考副本仅供评分器有效性检查，不会复制到 Agent 工作目录。

直接评分入口（环境变量须指定真实任务与候选目录）：

```bash
TA_EVAL_TASK_DIR=/path/to/task \
TA_EVAL_CANDIDATE_DIR=/path/to/candidate \
TA_EVAL_GRADE_OUTPUT=/path/to/grade.json \
cargo test --offline --manifest-path engine/Cargo.toml --test eval_grader grade_candidate -- --exact --ignored --nocapture
```

评分 JSON 包含 `ok`、`reason`、`candidate_sha256` 与 `output`。评分先单独运行 `cargo test --offline --test hidden -- --nocapture --color never`，再运行 `cargo test --offline --all-targets -- --color never`；每条命令限时 120 秒，任一步错误令评分失败。`run.sh` 在输出目录保存 `<id>.grade.json` 与 `<id>.grade.log`；每次先删除旧报告，要求评分进程成功且本次报告合法、`ok=true`，否则整体退出 1。

独立交叉审查又复现了退出码不足以证明测试完成：候选函数调用 `std::process::exit(0)`，Cargo 成功退出而断言未执行完，旧评分器误判通过。现对 libtest 的开始数量与完成结果逐组配对，要求隐藏测试非空且全部执行成功；提前退出、结果缺失或截断均判失败。该负例已加入现有评分器集成测试，与正确修复的正例一同通过。此检查面向实际编码错误，并非对恶意候选伪造测试输出的强对抗证明。

无 bubblewrap 的 CI 中，回归仍验证评分拒绝执行，再明确跳过真实候选测试部分；正式 `grade_candidate` 不走跳过分支，隔离不可用始终失败。开发机已实际执行全部 4 项回归，另 1 项仅供指定环境调用的评分入口显式 ignored。

目前约定适用于 std-only 或依赖已缓存、允许修改现有 `src/*.rs` 的 Rust 夹具；额外新文件会被拒绝。评分器不提供任意语言适配，也不能替代真实服务或长时间模型运行验收。
