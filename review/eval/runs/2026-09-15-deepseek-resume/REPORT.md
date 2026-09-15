# 中断后 `--resume` 继续：resume-continue（2026-09-15，DeepSeek）

两阶段任务（runner 新增 `resume.md` 支持）：阶段 1 故意超时中断，阶段 2 `exec --resume <session>`
继续同一会话。原始证据：`resume-continue.phase1.jsonl`（阶段 1）与 `resume-continue.jsonl`（阶段 2）。

| 阶段 | status | exit | 说明 |
|---|---|---|---|
| 1（75s 超时） | timeout | 124 | 修好 alpha、跑检查、`>>` 追加一次 `alpha done`、更新计划，然后按指示 `sleep 90` 被超时打断 |
| 2（resume） | completed | 0 | 修好 beta、三项检查通过、`progress.txt` 里 `alpha done` **恰好一行**、`goal_done` |

阶段 2 用时 47.7s，token 129512/6688（同目录 JSONL 为本文件的这套结果）。

## 评测本身的两个稳定性教训（已写回任务定义）

1. 阶段 1 的超时（`timeout.txt`）必须**宽于"做完第一步"所需时间**，否则模型还没动手就被打断，
   阶段 2 拿到的是一个空工作区（实测 30s 时会偶发）。现改为 75s。
2. 阶段 2 的提示词现在明确写出"如果发现结果不明的回合挡住 signal_done，用 cancel_run 结清"：
   这是用户指南里的正常操作，写进提示词后任务稳定为绿（不写的话，模型能不能自己发现取决于运气）。
   "模型自己发现"的那条路径另有证据：`resume3` 那次运行是模型自己从拒绝回执里读到 run id 并结清的。

## 这一轮真正修掉的两个缺陷

1. **未知结果的回合无法结清**：阶段 1 被中断的回合在 resume 时落 `OUTCOME_UNKNOWN`，而
   `signal_done` 的完成检查把它当阻塞项，且**没有任何动作能清掉它** → 会话再也无法完成目标。
   现在 `cancel_run` 可以作用于 OUTCOME_UNKNOWN 的回合：把它置为 CANCELLED 并记
   `acknowledged_outcome_unknown: true`（人工确认"副作用我接受了、不再重试"）；阻塞信息也会
   直接给出"用 cancel_run 结清哪个 run"。
2. **失败回执丢掉了细节**：阻塞项（含 run id 与提示）放在回执的 `result` 里，而工具结果只把
   `error` 交给模型，模型因此只能瞎猜 run id（实测猜了 5 个全错）。现在失败回执会把
   `result` 作为 `detail` 一起回传。

修复后的实际链路（阶段 2 `tool` 行）：
`signal_done False {"error":"goal not yet complete","detail":{"blockers":["outcome-unknown operations: leader:run_fadd431073c44ae0 (acknowledge each with cancel_run …)"]}}`
→ `cancel_run True {"run_id":"run_fadd431073c44ae0"}`（status=acknowledged）
→ `signal_done True` → `goal_done`。
