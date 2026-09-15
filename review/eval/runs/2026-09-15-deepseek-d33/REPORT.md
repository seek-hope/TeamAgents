# D-33 之后的真实复跑：team-collab（2026-09-15，DeepSeek）

- 目的：确认"成员继承 Leader 工具 + 自动 Leader↔成员通道 + 成员间只走共享空间"落地后组队链路仍通。
- 命令：`review/eval/run.sh --only team-collab --timeout 900`（deepseek-flash，默认单成员会话）
- 原始证据：同目录 `team-collab.jsonl`（含每次工具调用的 `tool` 行）

| status | exit | 秒 | tokens(prompt/completion) | 工具调用 | 失败调用 | 验收 |
|---|---|---|---|---|---|---|
| completed | 0 | 35.3 | 116651/7289 | 33 | 0 | 全部通过 |

动作分布：Leader `apply_topology_patch`(1，带 `base_revision`) → `assign_task`(2) → `wait_for_tasks`(1)
→ 自己 `ls/read_file/shell` 复核 → `signal_done`(1)；alpha-fixer 与 beta-fixer 各自
`ls/read_file/edit_file/shell/complete_task` 完成子任务。

本次 Leader 显式给了 `tool_bindings`，所以走的是文档路径而非继承默认值；继承与自动通道
由 `engine/tests/chat_e2e.rs::review_add_agent_inherits_leader_tools_and_gets_channels` 覆盖。
