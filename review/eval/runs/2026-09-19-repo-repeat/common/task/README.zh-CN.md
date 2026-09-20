# repo-session-fork 真实仓库任务

该任务使用 TeamAgents 历史提交 `046a43e32a73794e057ae0331ee3247ee3c42179` 的固定输入，
包含 core、engine、tui 三个独立 crate、文档和示例；不复制 `review/`、当前脏工作树或 Git 历史。
共 77 个输入文件、58 个 Rust 文件、29,285 行 Rust。它是本项目的真实历史修复任务，
不是陌生第三方仓库或通用排名基准。

`fixture-source.toml` 指定完整提交与路径；`fixture/.config/teamagents/config.toml` 是显式叠加的
无凭据输入，供旧公开测试在隔离 HOME 下使用。实际评测模型配置在工作区之外。仅允许修改
`engine/src/worker.rs` 与 `engine/src/session.rs`；其余输入字节及所有文件权限必须保留。

提示词声明全部行为要求：继承有效模型配置、会话 profile 与 Leader 历史，按新上下文映射
树和旧线性记录，保持团队事实与其他成员记录隔离，失败切换保留源会话，失败 fork 清理目标，
活动成员阻止 fork。隐藏检查不增加未声明要求，也不要求采用参考实现。

`grading.toml` 指定 engine 清单和四个相关公开 suite（12 项测试）。评分先运行 7 项隐藏行为
检查，再运行公开 suite；每条评分命令上限 300 秒，全部在既有 bubblewrap 中离线构建与执行。
模型任务时限为 1200 秒。隐藏测试仅进入评分副本，不进入模型工作区。

2026-09-19 工具 HOME 隔离后的环境修订：旧公开测试依赖用户配置，现由提示词/公开命令显式指定
`XDG_CONFIG_HOME="$PWD/.config"`，独立评分通过 `grading.toml` 的 `config_home = ".config"` 使用相同输入。
固定提交、77 个输入文件、七项隐藏行为、公开 suite、允许改动和时限均未变；不挂载宿主配置、不恢复项目 HOME。
该修订只声明测试环境，但提示词和评分配置哈希已改变；新样本单独记录，不能与旧样本合并成无条件的同设置成功率。

任务冻结前已验证：

- 原始快照：12 项公开测试通过；隐藏检查 2/7，通过的是事实隔离和活动成员拒绝，其余 5 项失败。
- `/tmp` 中参考修复：隐藏 7/7、公开 12/12 和最终文件保护通过。参考没有进入任务输入。
- 隐藏探针初版曾错误读取动作回执的 `decision`，按真实 `ok` 字段校正后重新验证；校正前结果不算有效负例。
- 各用例独立配置环境，树/线性历史缺陷不会被会话 profile 缺失提前掩盖。

可在仓库根目录准备并评分原始负例：

```bash
mkdir -p /tmp/new-repo-fork-baseline
TA_EVAL_TASK_DIR="$PWD/review/eval/tasks/repo-session-fork" \
TA_EVAL_STAGE_OUTPUT=/tmp/new-repo-fork-baseline \
cargo test --offline --locked --manifest-path engine/Cargo.toml \
  --test eval_grader stage_fixture -- --exact --ignored --nocapture

TA_EVAL_TASK_DIR="$PWD/review/eval/tasks/repo-session-fork" \
TA_EVAL_CANDIDATE_DIR=/tmp/new-repo-fork-baseline \
TA_EVAL_GRADE_OUTPUT=/tmp/new-repo-fork-baseline.grade.json \
cargo test --offline --locked --manifest-path engine/Cargo.toml \
  --test eval_grader grade_candidate -- --exact --ignored --nocapture
```

第二条命令应失败。真实模型运行使用 `review/eval/run.sh --only repo-session-fork`，
需先设置隔离的模型配置与原生上下文。最终文件保护不等于全过程文件审计；
公开验收也不是全仓库全部测试。真实完成率须以完整运行记录为准。
