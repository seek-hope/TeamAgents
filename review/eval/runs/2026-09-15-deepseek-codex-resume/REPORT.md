# Codex 成员被中断 → resume 自救：resume-codex-recovery（2026-09-15，DeepSeek + Codex）

把"成员级中断 + 任务自救"这条链在 **Codex 后端**上再验一遍（TeamSpec/config 见
`review/eval/tasks/resume-codex-recovery/`；codex-dev 走 `codex_profile = "deepseek"`，不用官方订阅）。

| 阶段 | status | exit | 说明 |
|---|---|---|---|
| 1（40s 超时） | timeout | 124 | Leader 把 `sh -c 'echo started >> run.txt; sleep 90; echo done >> run.txt'` 派给 codex-dev，等到会话超时 |
| 2（resume） | completed | 0 | 见下；`run.txt` 恰好 `started` + `done` 各一行 |

阶段 2 用时 58.7s，token 513294/11022，验收命令全部通过。

## 事件链（原始 JSONL）

1. resume 时两个回合都无法确证：`run_failed(codex-dev, OUTCOME_UNKNOWN)`、
   `run_failed(leader, OUTCOME_UNKNOWN)` → `task_blocked(task_c215…, reason=external turn outcome could not be confirmed)`。
2. Leader `cancel_task`（CANCELLED）→ 重派"收尾任务，不需要等待" → codex-dev `task_completed(SUCCEEDED)`。
3. Leader 自行复验 run.txt（13 字节、两行顺序正确）→ `signal_done` 先被拒（两个未知 run 的 detail）
   → `cancel_run` 两次（都 acknowledged）→ `signal_done` 通过 → `goal_done`。

结论：Codex 成员的中断同样落 BLOCKED，且恢复链与 Chat 成员一致；被中断命令的副作用没有重复。
