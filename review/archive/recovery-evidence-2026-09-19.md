# 旧排队回合的执行证据与历史清理

延续方案 §9 与 D-32/D-35，修复“旧 QUEUED 的检查点丢失后被当成新工作”和历史清理删除恢复依据。
保持现有数据库格式、成员身份、权限规则和后端核对路径；本批仅使用本地模型协议夹具。

## 已复现的问题

实际 `serve` 进程先通过 bubblewrap Shell 向文件追加一行，再等待下一次模型响应。
SIGKILL 后删除该回合检查点，把数据库记录改为旧版可能留下的 QUEUED；数据库仍保留
`run_started`。修复前，重开会发出新的模型请求，存在重复执行已发生副作用的风险。

另三项反例确认：清理会删掉未结清回合的开始事件；损坏的开始载荷未被拒绝；
删除投递后删除事件失败，会留下部分清理结果。

原始复现日志：

- `/tmp/teamagents-recovery-evidence-before-20260919.log`
- `/tmp/teamagents-recovery-evidence-retention-before-20260919.log`

## 实现

`Store::run_started_ids` 查询当前会话的开始事件，不依赖界面前 1000 条事件。
恢复排队回合时共同使用数据库开始事件、成员检查点和外部回合 ID；存在任一执行依据，
先整批恢复到核对路径，再由原后端确认结果。Chat 缺失/损坏检查点或存在未确认副作用时，
明确报告 `OUTCOME_UNKNOWN` 及原因，不补造检查点或自动重新请求模型。
Codex 缺少外部回合 ID 也保持原来的结果不明处理。

恢复和清理共用开始载荷校验：回合 ID、成员 ID、RUNNING 开始状态和 actor 必须有效，
用于恢复的目标或拟清理的事件还需匹配本会话实际回合与成员。损坏记录原样保留并报错，
JSON 诊断不输出私有载荷内容。

`prune_history` 从读取候选开始使用同一个 SQLite savepoint。投递与事件一起删除，
后续批次失败也整体回滚；事件按 256 个序号分块，避免大历史超过绑定参数上限。
只有 COMPLETED、FAILED、CANCELLED 回合的开始证据可按年龄清理；
OUTCOME_UNKNOWN 仍可能继续核对，必须和活动回合一样保留。未消费投递的原保护保持。
只有实际删除后、没有外层事务时才尝试 VACUUM。

## 行为检查

新增四项 core 回归：

| 检查 | 证明范围 |
|---|---|
| `run_start_evidence_is_not_limited_to_display_history_or_other_sessions` | 开始事件位于 1000 条显示记录之后仍可查到；其他会话的有效/损坏记录不混入当前结果 |
| `history_retention_keeps_start_evidence_until_a_run_is_settled` | 八种回合状态；仅三个已结清状态可删；dry-run 不写入且计数一致 |
| `history_retention_failure_does_not_partially_delete_delivery_evidence` | 超过 600 条历史，第三批删除失败时投递和前两批事件全部回滚；修复后计数一致，其他会话保留 |
| `history_retention_rejects_malformed_start_evidence_without_deleting_it` | 十三类 JSON、状态、成员、actor、缺失/外会话回合错误；恢复查询和清理均拒绝，私有标记不泄露，修复后可继续 |

新增一项 engine 回归：
`queued_chat_with_a_lost_checkpoint_uses_start_evidence_instead_of_replaying`。
通过真实进程、SIGKILL 和文件追加，检查普通重开、旧取消标记、显式清理与
`open_session` 自动清理四条路径：同一个回合进入结果不明，没有新增模型请求，文件只有一行。

扩展 `missing_codex_turn_id_cannot_blindly_requeue_accepted_work`：
原 RUNNING 和旧 QUEUED 均在缺少外部 ID 时停止核对，不启动 app-server 请求。
继续保留“从未开始的新排队意图正常执行”与脚本后端稳定动作 ID 重放的原回归。

## 验证

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test stored_integrity
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery --test chat_e2e --test codex_recovery --test session_boot
make fmt
make check
make pty
bash review/eval/check-runner.sh
```

最终 `make check` 的格式、全目标严格 Clippy、回归及仓库卫生检查通过：
**core 152 / engine 369 / TUI 105，共 626 项**；engine 仍有三项显式 ignored，
未启用的真实服务入口不计作供应商验收。
专项为 `stored_integrity` 19 项，`chat_e2e` 59 项、`codex_recovery` 7 项、
`recovery` 29 项、`session_boot` 3 项，全部通过。三项 PTY 与 runner 30 项契约通过，
runner 实际执行 bubblewrap 和 Git 评分检查，没有调用真实模型。
八份相关文档的 128 项本地链接有效，`git diff --check` 通过；累计修改未提交或发布。

最终验证日志：

- `/tmp/teamagents-recovery-evidence-core-current-20260919.log`
- `/tmp/teamagents-recovery-evidence-engine-current-20260919.log`
- `/tmp/teamagents-recovery-evidence-check-20260919.log`
- `/tmp/teamagents-recovery-evidence-pty-20260919.log`
- `/tmp/teamagents-recovery-evidence-runner-20260919.log`

过程记录保留：早期扩大默认脚本后端恢复判断导致既有 T8 失败，该额外改动已撤回；
早期保留检查曾错误使用包含 OUTCOME_UNKNOWN 的 `is_terminal` 判断，已改回显式三个已结清状态。
早期通过数不作为最终源码的验证证据。

## 保留边界

- 开始事件只能证明回合曾启动，不能还原丢失的模型回复或确认外部副作用。
  数据库开始记录、检查点和外部 ID 全部丢失时，仍无法证明它与从未开始的意图可区分。
- 本批没有新增迁移或损坏数据库自动修复，没有为任意外部工具提供 exactly-once 保证。
- 开始事件查询扫描当前会话历史；本批检查了超过界面上限的记录，没有给出超大历史的性能承诺。
- 不新增真实供应商、陌生仓库、真实模型 TUI 或发行制品成绩。
  T1–T24 的十八项路径证据与六项部分覆盖保持，整体成熟度目标未完成。
