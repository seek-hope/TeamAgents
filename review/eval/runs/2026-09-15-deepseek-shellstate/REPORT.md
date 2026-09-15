# 全量任务集复跑：持久 Shell 之后（2026-09-15，DeepSeek）

本轮改了 shell 的包装方式（每条命令前置恢复 / 后置捕获状态），所以用真实模型把整套任务跑一遍
确认没有回归。命令：`review/eval/run.sh --timeout 600`；原始 JSONL 见同目录。

| 任务 | status | exit | 秒 | tokens(prompt/completion) | 验收 |
|---|---|---|---|---|---|
| approval-gate | approval_required | 3（期望 3） | 1.6 | 4136/209 | 全部通过 |
| edit-integrity | completed | 0 | 44.3 | 61177/7492 | 全部通过 |
| interrupted-recovery | timeout | 124（期望 124） | 45.1 | 10446/2235 | 全部通过 |
| long-output | completed | 0 | 27.3 | 43766/4786 | 全部通过 |
| rust-fix | completed | 0 | 16.6 | 52042/2144 | 全部通过 |
| team-collab | completed | 0 | 42.7 | 145536/8925 | 全部通过 |

6/6 符合预期（4 个 completed + 2 个刻意的安全边界），0 个失败的工具调用。
`long-output` 与 `rust-fix` 里模型实际用到了 shell（重定向、`cargo test`），说明包一层状态捕获
之后命令语义、退出码、`(exit N)` 标记都还正常。
