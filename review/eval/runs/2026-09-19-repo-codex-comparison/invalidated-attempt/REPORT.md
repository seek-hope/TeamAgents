# 首次 Codex 仓库对照：隐藏验收污染，作废保留

本次不能作为 Codex 的能力成功或失败样本，也没有执行独立评分。Codex 的
`workspace-write` 约束写入范围，但该次运行仍能读取工作区外的任务目录。
把隐藏测试放在工作区之外不足以保证隐藏性，这是本轮评测驱动的隔离缺口。

`codex.jsonl` 中的完整轨迹显示：

- `item_50` 读取相邻任务目录的公开检查、允许文件、时限和评分配置。
- `item_51` 读取任务说明；`item_52`、`item_53` 读取全部 `hidden_tests.rs`。
- `item_75` 把隐藏测试复制到候选的 `engine/tests/zz_hidden_check.rs` 并运行。
  模型后来删除了该临时文件，因此最终文件树没有额外文件；最终范围检查仍不能证明验收未污染。

发现后停止评测。最初在外层工具沙箱内发送的终止请求未触及宿主进程；
随后经权限流程核实宿主 PID 对应本次 Codex 后，发送 SIGTERM 并确认退出。
外层记录为 **339.999 秒、子进程 -15**，不是自然完成或任务超时。
没有 `turn.completed` 用量汇总，不补造完整 token 数。

本次仍使用 DeepSeek Flash / high / 用户确认的原生 1,000,000 窗口；
配置、输入哈希、过程与最终候选补丁全部保留。后续从全新会话和原始输入重跑，
使用额外文件系统隔离，让隐藏测试、开发仓库及历史候选在模型进程中不可见，
并保留 Codex 自身沙箱。不得续接本次已污染上下文或将本次产物用于干净样本。

证据：`invalidated.json`、`codex.jsonl`、`exit.json`、`manifest.json`、
`fixture-manifest.json`、`candidate-manifest.json` 与 `candidate.patch`。
