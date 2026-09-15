# 计划工具（update_plan）的真实运行：plan-use（2026-09-15，DeepSeek）

- 任务：`review/eval/tasks/plan-use/`（两个独立小 bug；提示词要求先 `update_plan` 再按计划执行）
- 命令：`review/eval/run.sh --only plan-use --timeout 600`

| status | exit | 秒 | tokens(prompt/completion) | 验收 |
|---|---|---|---|---|
| completed | 0 | 27.3 | 80724/3149 | 全部通过 |

`update_plan` 三次调用（原始 JSONL 的 `tool` 行）：

1. 初始计划：`修 alpha (in_progress)` + `修 beta (pending)`
2. 修完 alpha 后：`alpha (done)` + `beta (in_progress)`
3. 修完 beta 后：两项都 `done`

落盘：`sessions/<id>/members/leader/plan.json` 两项均为 done；下一轮请求的系统提示里带
`<plan>` 块（回归 `chat::tests::plan_round_trips_into_the_prompt_block`）。
