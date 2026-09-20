# Codex 冷启动恢复与断线收敛（2026-09-19）

本批继续实施方案 §9.2、§10.2、T17 的恢复要求，接续[投递授权修复](delivery-acl-2026-09-19.md)。
没有增加产品范围或依赖。恢复依据是持久线程与精确回合引用，以及原有任务完成事务；
不根据线程最后一条记录推断某个旧回合成功。

## 发现与复现

旧实现的五项定向检查先失败：

1. `CodexRunner::reconcile` 直接读取内存中的 server/thread；重建运行器后两者均为空，根本没有核对历史。
2. 线程有后续回合时，使用 `turns.last()` 会把后续成功记到此前失败的回合上。
3. 恢复接口只返回状态，遗漏结果摘要和 `complete_task` 申请；即使恢复出 COMPLETED，任务仍可能 BLOCKED。
4. RUNNING Codex 回合未保存外部 ID 时，runtime 将其当作内置成员重新排队，无法排除外部已经执行的窗口。
5. 本机 Codex **0.155.0** 生成的 `TurnSteerParams` 要求 `expectedTurnId`，原代码发送 `turnId`；
   旧假服务没有校验字段，因此原测试漏过了请求被拒的问题。

原始失败日志：

```text
/tmp/teamagents-codex-recovery-before-20260919.log
/tmp/teamagents-codex-matching-before-20260919.log
/tmp/teamagents-codex-steer-before-20260919.log
```

相邻路径还存在两处重放风险：未确认输入在 OUTCOME_UNKNOWN 归档后仍为 pending，
调度器会据此创建另一个回合；app-server 断线被标为普通 FAILED，也不能证明外部副作用未发生。
本批一并处理，并增加运行时回归。
随后真实服务复跑进一步发现：多条流式消息直接连接，历史消息以换行分隔，
两者重建的摘要不同；再次使用相同 `action_id` 提交时触发核心的载荷冲突保护。
新增 `cold_codex_recovery_reuses_the_committed_completion_request` 先在修复前稳定失败，
原始输出为 `/tmp/teamagents-codex-completion-before-20260919.log`。

协议依据由本机 CLI 现场生成，遵循 D-3，不将生成物入库：

```bash
codex --version
codex app-server generate-json-schema --out /tmp/teamagents-codex-schema-20260919
```

其中 `v2/TurnSteerParams.json` 要求 `expectedTurnId/input/threadId`，
`v2/ThreadReadParams.json` 提供 `threadId/includeTurns`。
在线官方文档抓取曾遇到本机 TLS 证书验证失败；本批没有把未成功抓取的网页作为协议证据。

## 修复后的行为

- 启动连接与创建/恢复成员线程分开。冷启动核对仅初始化 app-server 并调用
  `thread/read(includeTurns=true)`，不为读取历史而 `thread/resume`、`thread/start` 或 `turn/start`。
  下一次明确的新工作仍沿原线程执行 `thread/resume`，并注入当前成员说明。
- 读取结果必须匹配保存的 thread ID 和唯一的 turn ID。缺失、重复、损坏、未知状态、
  仍在执行、读取失败或缺少完成记录均进入 OUTCOME_UNKNOWN，保留具体原因。
- `AgentRunner::reconcile` 返回完整 `TurnOutcome`。COMPLETED 时通过恢复专用网关补交相同
  `run_id:turn_id:complete` 动作；已有完成申请时经会话归属校验读取并复用原记录，
  不重建或改写已提交的载荷。继续使用核心去重与任务结果事务；网关没有外部工具执行器。
  只提取匹配回合的 `agentMessage`，不发布私有 reasoning、工具输出、用户输入或其他回合。
  摘要与回复继续遵守既有 2,000/4,000 字符上限。FAILED 保留外部错误，interrupted 映射为 CANCELLED。
- 结算只确认后端持久记录证明已接受的投递，尚未接受的正常后续消息保留。
  对结果不明回合中仍 pending 的输入，在同一个结算事务中记录
  `dropped_reason="input outcome unknown"` 和 `run_id`，停止自动重放；原事件保留，
  不伪造消费确认，不推进这些输入的消费批次。SQLite 审计失败会回滚整个结算。
  Leader `cancel_run` 结清未知结果也不会复活这些旧输入；新明确工作仍可调度。
- 区分 JSON-RPC 明确拒绝与传输失败。`turn/start` 发送后连接断开/超时、
  成功响应缺少可用 ID，或回合中 app-server 退出，都不能证明任务失败：
  进入 OUTCOME_UNKNOWN，任务 BLOCKED，释放等待线程，保留成果且不自动再次执行。
  effort 兼容回退仅处理明确的 RPC 拒绝。普通新回合仍可重建已退出的 app-server。
- `turn/steer` 改为 `expectedTurnId`；假服务现在实际校验字段，成功与拒绝路径继续检查持久确认账本。

## 自动化证据

新增 **1 项 core、8 项 engine** 回归；另更新原有 app-server 退出、Chat 恢复接口、
steer 契约与真实模型检查。不是将旧测试重命名计为新增。

| 文件 / 测试 | 证明范围 |
|---|---|
| `core/tests/delivery_acl.rs::unknown_input_is_audited_without_acknowledgement_or_automatic_replay` | 已接受与不确定输入分开结算；审计失败回滚；旧缓冲不可重放；结清未知状态不复活旧输入；新工作可调度 |
| `engine/tests/codex_contract.rs::reconcile_matches_the_saved_turn_even_when_a_later_turn_completed` | 不能把后续成功归给旧失败回合 |
| `engine/tests/codex_recovery.rs::cold_codex_recovery_restores_the_matching_result_and_is_idempotent` | 生产 open_session 重开；任务与摘要恢复；只确认已接受输入；剩余消息保留；重复核对无重复事件；私有工具/推理不发布 |
| `cold_codex_recovery_preserves_failures_and_rejects_unverifiable_history` | 失败原因、取消、找不到对应回合、仍执行、空历史和未知状态 |
| `missing_codex_turn_id_cannot_blindly_requeue_accepted_work` | 未保存外部 ID 时不重排原回合、不创建替代回合 |
| `cold_codex_recovery_expires_stale_approvals_and_checks_thread_identity` | 错线程、重复 ID、缺完成项目、RPC 失败；WAITING_APPROVAL 恢复完成或未知后旧批准 EXPIRED |
| `cold_codex_recovery_reuses_the_committed_completion_request` | 流式拼接与历史消息分隔不同；复用已提交申请，不伪改动作载荷或重复任务结果 |
| `killed_codex_client_recovers_persisted_completion_without_reexecuting` | 子进程走生产 open_session/CodexRunner；实际 SIGKILL；新进程读持久外部结果；副作用一行、turn/start 一次 |
| `disconnected_codex_submission_never_schedules_automatic_reexecution` | 真实 runtime 调度；外部回复前/后断线及成功回复缺 ID；均仅一次副作用、一个工作回合、BLOCKED 任务 |

上表协议夹具不调用真实供应商。SIGKILL 测试杀的是持有生产会话的测试子进程，
外部 app-server 为持久化假服务；真实模型证据单独列在下一节。

复跑：

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test delivery_acl
cargo test --offline --locked --manifest-path engine/Cargo.toml \
  --test codex_recovery --test codex_contract --test codex_adapter
make check
make pty
```

最终 `make check` 全部通过：**core 79 / engine 289 / tui 104**；engine 另有 1 项 ignored，
默认未启用的 live 检查仍不计真实服务证据。格式、全目标严格 Clippy 与卫生检查通过。
`make pty` 三项通过：输入/粘贴/退出、点击、实际工作区审查。
中途整仓检查曾因旧 app-server 退出单测仍期待 FAILED 而失败；将其同步为已验证的
OUTCOME_UNKNOWN 语义后，最终全仓通过。原始失败日志保留，没有当作通过计数。

```text
/tmp/teamagents-codex-recovery-check-verified-20260919.log
/tmp/teamagents-codex-recovery-pty-20260919.log
/tmp/teamagents-codex-recovery-check-final-20260919.log  # 中途旧断言失败
```

## 真实服务与剩余边界

`engine/tests/live_codex.rs::live_codex_turn_and_cold_recovery_through_app_server`
已改为真实任务：原生执行工具追加并读回一行文件 → 外部回合完成，但本地未最终归档 →
关闭运行器/会话 → 生产 open_session 重开 → 匹配原回合、归档任务结果。
同时核对线程一致、任务 SUCCEEDED、回合 COMPLETED、汇报标记和文件仍恰好一行。
工作目录、XDG 状态和 CODEX_HOME 都使用隔离临时目录；仅从所选本机配置/凭据取认证，
不加载用户配置中的其他 MCP、hooks 或项目记录。

真实后端为 **Codex 0.155.0 + DeepSeek Flash，原生上下文 1,000,000**，
来源为用户确认的 D-36。首次在外层工具沙箱内运行时，Codex 的 bubblewrap mount-lock
报只读错误，随后停在批准，240 秒超时，检查时文件未生成。
通过执行权限流程在外层沙箱之外重跑，仍保留 Codex 自身的 workspace-write/on-request 设置：
首次重跑 **通过，7,712 ms**。
第二次真实复跑在 10.87 秒后因前述完成申请载荷冲突失败；复用原申请的修复完成后，
最终真实复验 **通过，12,879 ms**。每次都使用原生 1M，没有缩小窗口。
全部成功、失败日志和限定字段的结果摘要均保留在
[真实验收记录](eval/runs/2026-09-19-codex-recovery/REPORT.md)。

这些证据证明指定路径，仍不是五供应商全矩阵、真实模型下所有崩溃时间点、
全套批准/取消组合、或 Codex CLI/Claude Code/pi/Hermes 的同条件编码能力对照。
外部接受与本地记录之间仍没有通用 exactly-once；ID 缺失时不会猜测对应回合。
历史显示仍在执行的外部工作不会被此逻辑自动接管，需先核对实际进程/成果并结清未知结果。
T17 保持部分验收，长期目标保持进行中。
