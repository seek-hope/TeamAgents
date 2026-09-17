# 真实 DeepSeek 编码任务验证（2026-09-17）

使用本机已有 `leader_main` 配置，实际模型为 `deepseek-flash`。每项使用独立临时工作目录与状态目录，运行真实 `teamagents exec --json`、真实 Shell/bubblewrap 与验收命令。没有复制凭据。本次不是假服务测试，也不是竞品对照；任务保持原有固定提示词和检查。

| 任务 | 最终状态 | Agent 退出码 | 总秒数 | result 输入/输出 tokens | 验收 |
|---|---|---|---:|---:|---|
| [Rust 修复](rust-fix.jsonl) | completed | 0 | 14.8 | 45569/2057 | 1 项全部通过 |
| [精确编辑](edit-integrity.jsonl) | completed | 0 | 35.5 | 80086/7009 | 1 项全部通过 |
| [协作（完成竞态修复前）](before-goal-fix/team-collab.jsonl) | completed | 0 | 42.7 | 149604/10727 | 2 项全部通过 |
| [协作（最终修复后）](team-collab.jsonl) | completed | 0 | 62.2 | 173559/13488 | 2 项全部通过 |
| [批准边界](approval-gate.jsonl) | approval_required | 3 | 1.5 | 3480/210 | 1 项全部通过 |
| [中断后续接](resume-continue.jsonl) | completed | 124 → 0 | 104.2 | 116456/7398 | 3 项全部通过 |

批准边界的正确结果为 `approval_required`/3；恢复第一阶段的正确结果为 `timeout`/124，随后 `--resume` 返回 `completed`/0。修复后的评测脚本在这两类预期非零状态下仍验证对应契约，而不是把 Agent 非零直接混成错误或忽略真正失败。

恢复阶段一耗时 75.048 秒，阶段二 29.117 秒；表中耗时相加，usage 按最后 `result.usage` 的会话累计值报告，不重复累加第一阶段。该任务验收检查 `progress.txt` 的已完成步骤恰好一次及最终功能输出。阶段一记录单独保存在 [resume-continue.phase1.jsonl](resume-continue.phase1.jsonl)。

`rust-fix`、`edit-integrity`、批准与首轮协作验证使用本轮工具/取消/协议修复后的二进制，随后完成竞态又有补充修复。因此额外构建最终二进制后重跑协作，并运行恢复任务；首轮协作保存在 `before-goal-fix/`，没有丢弃其耗时或成绩。

可复跑（需要本机模型配置/凭据与可运行的 bubblewrap；输出目录应每次独立）：

```bash
cargo build --offline --manifest-path engine/Cargo.toml --bin teamagents
review/eval/run.sh --only rust-fix --timeout 180 --out /tmp/teamagents-live-rust-20260917
review/eval/run.sh --only edit-integrity --timeout 180 --out /tmp/teamagents-live-edit-20260917
review/eval/run.sh --only team-collab --timeout 240 --out /tmp/teamagents-live-final-team-20260917
review/eval/run.sh --only approval-gate --timeout 120 --out /tmp/teamagents-live-approval-20260917
review/eval/run.sh --only resume-continue --timeout 180 --out /tmp/teamagents-live-resume-20260917
```

所有这些评测脚本调用均返回 0，原始 JSONL 与 stderr 随本报告保存。恢复仍使用任务内原有 `timeout.txt` 的 75 秒，`--timeout 180` 是第二阶段上限；首次协作的输出目录为 `/tmp/teamagents-live-team-20260917`。

边界：这些任务规模较小、检查公开、运行次数有限，没有隐藏仓库级验收。不能用此表证明复杂工程成功率、五家供应商兼容性或比 Codex CLI / Claude Code / pi / Hermes 更优。离线回归、真终端与后续路线见 [总审查报告](../../../coding-agent-review-2026-09-17.md)。
