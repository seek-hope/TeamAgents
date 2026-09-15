# 成员级中断 → 任务 BLOCKED → Leader 自救：resume-task-recovery（2026-09-15，DeepSeek）

目标：验证"成员回合被中断"这条最难受的恢复路径**不需要人介入**，团队自己能把活干完。
两阶段任务（`review/eval/tasks/resume-task-recovery/`，TeamSpec 见同目录 `team.yaml`）。

| 阶段 | status | exit | 说明 |
|---|---|---|---|
| 1（40s 超时） | timeout | 124 | Leader 把 `sh -c 'echo started >> run.txt; sleep 90; echo done >> run.txt'` 派给 dev；dev 跑起来后被会话超时打断 |
| 2（resume） | completed | 0 | 见下面的事件链；`run.txt` 恰好 `started` 一行 + `done` 一行 |

阶段 2 用时 64.6s（含两个回合），token 135921/12329，三项验收命令全部通过。

## 全模型驱动的事件链（原始 JSONL 的 event/tool 行）

1. `task_blocked {"task_id":"task_1820…","assignee":"dev","reason":"external turn outcome could not be confirmed"}`
   —— 被中断的 dev 回合在 resume 时无法确证，任务按规则落 BLOCKED。
2. `cancel_task(task_1820…)` → `{"status":"CANCELLED"}` —— Leader 自己结清卡住的任务。
3. `assign_task(dev, "不需要等待、立即…")` → 新任务 `task_2593…` → dev `echo done >> run.txt` →
   `complete_task` → `SUCCEEDED`。
4. Leader 自己 `cat run.txt` 复核 → `signal_done` 先被拒（**还有一个未知结果的 run 没结清**，
   拒绝回执带着 `detail.blockers`）→ `cancel_run run_6c74…` / `run_7d76…`（两个都 acknowledged）→
   `signal_done` 通过 → `goal_done`。
5. `run.txt` 内容：`started` 与 `done` 各一行（`grep -c '^started$'` = 1）——被中断命令的副作用
   **没有重复**。

## 顺带修掉的一处措辞缺陷

同一次运行里 Leader 先按阻塞信息的写法调了 `cancel_run "dev:run_6c74…"`（把"成员:run"整串当成 id）
被拒两次，才改成裸 `run_…` 成功。阻塞信息已改为 `run_6c74… of dev`（run id 独立可复制）。
