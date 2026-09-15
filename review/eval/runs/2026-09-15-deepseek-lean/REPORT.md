# 精简系统提示后的全套复跑（2026-09-15，DeepSeek）

改动：系统提示里的工具清单从"每个工具一行说明"改为**只列名字**（每个工具的描述本来就在请求的
function schemas 里，等于每轮重复发送了一遍）。实测固定开销：系统提示 3227 → 471 字符
（`chat::tests::prompt_overhead_stays_lean` 把它钉住），工具 schemas 9784 字符不变。

命令：`review/eval/run.sh --timeout 600`（13 个任务；原始 JSONL 见同目录）。

| 任务 | status | exit | 秒 | prompt | completion | 验收 |
|---|---|---|---|---|---|---|
| approval-gate | approval_required | 3 | 1.9 | 3478 | 143 | 全部通过 |
| edit-integrity | completed | 0 | 14.1 | 26062 | 1967 | 全部通过 |
| interrupted-recovery | timeout | 124 | 45.1 | 3456 | 576 | 全部通过 |
| long-output | completed | 0 | 22.8 | 51827 | 3189 | 全部通过 |
| plan-use | completed | 0 | 23.8 | 67413 | 3276 | 全部通过 |
| resume-codex-recovery | completed | 0 | 37.0 | 660207 | 5474 | 全部通过 |
| resume-continue | completed | 0 | 20.6 | 81981 | 7706 | 全部通过 |
| resume-task-recovery | completed | 0 | 41.1 | 93305 | 8138 | 全部通过 |
| rust-fix | completed | 0 | 23.6 | 61791 | 3331 | 全部通过 |
| team-codex-gate | approval_required | 3 | 18.3 | 105701 | 2622 | 全部通过 |
| team-codex-interrupt | timeout | 124 | 45.1 | 203658 | 1670 | 全部通过 |
| team-codex | completed | 0 | 28.9 | 432641 | 3475 | 全部通过 |
| team-collab | completed | 0 | 38.3 | 113917 | 7824 | 全部通过 |

合计 prompt 1,905,437 / completion 49,391（13 个任务）。

可比对照：`plan-use` 的 prompt token 从 77,540 降到 67,413（−13%）；`team-collab` 从 116,651 到
113,917（−2.3%）。绝对值不大是因为历史本身占大头；固定开销（每轮都付）降幅是系统提示那一项
的 85%。行为面无回归：13 个任务的验收全过，含"必须读懂工具契约"的 team-collab 与 team-codex。
