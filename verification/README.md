# TeamAgents 形式化验证（R2 控制面）

目标：把重构方案里**已确认的协议性质**变成机器可查的规格，而不是只靠样本测试。当前阶段验证的是
**控制面协议模型**（`core/src/v2/control.rs` + `engine/src/v2/driver.rs` 的抽象），不是 Rust 代码本身——
"规格↔代码"的对应关系在下面的映射表里逐条给出，代码级验证（真实引擎上的不变量随机测试、
必要时的 Lean/Kani）是后续阶段。

## 运行

```bash
make verify-model           # 控制面小配置（秒级；13 不变量 + 4 性质）
make verify-model-all       # 控制面 + 制品/GC + 等待/唤醒三个模块的小配置
make verify-model-wide      # 控制面宽配置（2 实例 / 2 操作；约 11 分钟 / 275M 状态）
```

首次运行会把固定版本（TLC v1.7.1，SHA-256 见 Makefile）的 `tla2tools.jar` 下载到
`$TLA_TOOLS_DIR`（默认 `~/.local/share/teamagents-verify`）并校验；工具链不进仓库、不进 `make check`。
需要 Java（本机 OpenJDK 27）。

## 规格与配置

| 文件 | 内容 |
|---|---|
| `tla/V2Control.tla` | 控制面抽象模型：实例相位机、请求/尝试、决策与操作、批准、派发线性化点、取消/超时、epoch 重置、目标预算预留与结算、崩溃/恢复 |
| `tla/MC.cfg` | 小配置（1 实例 / 1 操作 / 2 请求槽 / 1 尝试槽 / 1 次 epoch 重置 / 1 次未知用量） |
| `tla/MC_wide.cfg` | 控制面宽配置（2 实例 / 2 操作且其一需批准 / 3 请求槽 / 2 尝试槽） |
| `tla/V2Artifact.tla` + `tla/MC_artifact.cfg` | 制品与 GC：写字节 → STAGING 行 → 引用与 LIVE 同事务 → GC 认领 → 删除/放弃 |
| `tla/V2Wait.tla` + `tla/MC_wait.cfg` | 等待/唤醒/计时器/取代：注册即求值 → 停放 drain 扫描 → 满足即答同事务 → 取消/取代/重挂 |
| `tla/V2Task.tla` + `tla/MC_task.cfg` | 任务生命周期与目标结清：委派（前置任务必须先存在、目标必须 ACTIVE）→ 启动 → 结清/取消 → 系统停放 → 终止级联；目标创建/请求准入/开放操作/结清与摘除 |
| `tla/V2Compress.tla` + `tla/MC_compress.cfg` | 上下文压缩（A20）：开门/提交/失败/被 epoch 关闭取消；总结追加在尾部、覆盖只增不减、原文永不删除 |
| `tla/V2Daemon.tla` + `tla/MC_daemon.cfg` | 会话 daemon 协议（A28）：稳定命令 id 的去重与回放、checkpoint 的快照+水位原子对、events(since) 无缺口、慢客户端不阻塞写者 |
| `tla/V2Checks.tla` + `tla/MC_checks.cfg` | 必需检查（A16/§8）：只对"自称成功"的候选做校验、失败进有界修复轮、轮次耗尽或校验路径不可用（陈旧观察/派发被拒）落 BLOCKED，绝不升级候选 |

环境（工具结果、批准时机、崩溃时点）在模型里是**非确定性**的；这正是要穷举的部分。

## 已验证的性质与代码映射

| 性质（规格） | 含义 | 代码锚点 | 验收场景 |
|---|---|---|---|
| `TypeOK` | 相位/生命周期/操作状态/效果计数取值合法 | `models.rs` 枚举、`OpStatuses` | §4.1 |
| `NoEffectBeforeApproval` | 需要批准的操作，未批准前效果为 0 | `dispatch_operation` 的批准闸；`approve`/`deny` | A25/A12 |
| `RecordBeforeEffect` | 效果发生前必有持久化派发记录 | `dispatch_operation` 先落 `DISPATCH_COMMITTED` 再执行 | A08/A11 |
| `EffectAtMostOnce` | 同一操作的外部效果至多一次（恢复不重放） | 恢复路径只置 `OUTCOME_UNKNOWN` | A08/A10/A13 |
| `ReservationsAdmitted` | 活跃预留总量不超上限（准入闸门的推论） | `reserve_budget` 的 `known+reserved+est ≤ max` | A18/§8 |
| `AdmissionGate`（时序） | 每次进入 `MODEL_PENDING` 都通过准入闸门 | 同上 | A18/§8 |
| `ReservationReleased` | 请求关闭（完成/失败/取消）必释放预留 | `release_reservation` 的调用点 | §8 |
| `OneActiveRequest` | 单实例同时只有一个活跃请求 | `begin_request` 的 phase/revision 守卫 | §3/§6.1 |
| `SelectionIsComplete` | 只有被原子选中的完整尝试存在 | `record_attempt` 的 `selected_attempt_id IS NULL` 更新 | A19 |
| `NoTurnWithoutWork` | 最后一条是模型自己的话时不开新回合 | 收尾条目 + `step_ready` 闲置判据（R22 修复） | §5.4 |
| `StaleExecutorRejected` | 推进中的执行者必然持有当前 revision | `begin_request` 的 `revision == expected` | §6.1 |
| `NoEffectOnTerminated` | 已终止实例不产生效果 | `set_lifecycle` TERMINATED 与派发守卫 | §6.4 |
| `PreparedIsNotTerminal` | `PREPARED` 操作尚未产生任何效果 | 操作状态机 | §6.1 |
| `CancelledBeforeStartHasNoEffect` | "开始前取消"= 效果未发生（可已有派发记录） | `cancel_operation` 对 `DISPATCH_COMMITTED` 的处理 | A13 |
| `TerminalOpStable`（时序） | 操作终态不可改写 | `complete_operation` 的 "already terminal" 拒绝 | A13 |
| `TerminalGoalStatusStable`（时序） | 目标终态不可改写 | `complete_goal`/`block_goal` 的 `already_closed` 分支 | §8 |
| `NoReceiptAcrossEpochs`（时序） | 回执不跨 epoch 落地 | `reset_instance` 关闭旧 epoch + 取消在途操作 | A24 |

### 制品与 GC（A30）

| 性质（规格） | 含义 | 代码锚点 |
|---|---|---|
| `NoReferenceToUnpersisted` | 有引用的制品其字节必已持久化 | `store_response_artifact`（tmp→fsync→rename 后 `artifact_stage`） |
| `LiveIsPersisted` | LIVE 制品必有字节 | `artifact_publish` 与引用同事务 |
| `GcClaimsOnlyUnreferencedLive` | 被认领（DELETING）的制品无任何引用 | `artifact_gc_claim` 的候选条件 |
| `CollectorSkipsIncomplete` | 磁盘上没有半成品残留（STAGING/ABANDONED 不被删除器动） | GC 只认领 LIVE；`artifact_abandon` 只标记 |
| `ReferencesOnlyLive`（时序） | 引用只能附着到 LIVE 制品，或在同一步随 LIVE 翻转附着，且字节已在 | `publish_one` + 引用同事务 |
| `BytesOnlyDeletedWhileDeleting`（时序） | 文件消失只发生在 DELETING | GC 删除顺序 |
| `ClaimOnlyFromLive`（时序） | GC 只从 LIVE 认领 | 同上 |
| `LiveFlipCarriesReference`（时序） | STAGING→LIVE 必伴随首次引用（不会有"活着但无人引用"的窗口被回收） | `publish_list` 同一命令内提交 |

### 等待 / 唤醒 / 计时器（A22/A23、RT-06）

| 性质（规格） | 含义 | 代码锚点 |
|---|---|---|
| `TypeOK` | 相位/等待状态/回答计数取值合法 | `waits.status`、`instances.phase` |
| `WakeAnswerAtMostOnce` | 唤醒回答每个 wait 至多追加一次 | `wake_satisfied_at` 的 `PENDING → SATISFIED` 守卫 + `append_context` 去重 |
| `AnswerImpliesConditions` | 没有虚假唤醒：回答只在条件真的成立时追加 | `evaluate_wait` 先判 `satisfied` 再追加 |
| `AnswerImpliesSatisfied` | 回答永远与 `SATISFIED` 同一步出现 | 同上（同一事务） |
| `WakeAnswersItsCall` | 回答落在等待自己的 tool_call 上（R22 配对修复） | `wait_call_id` + `Observation::ToolResult` |
| `WaitingHasPendingWait` | 停放实例必有属于自己的 PENDING 等待 | `import_response` 仅在未满足时置 `WAITING` |
| `PendingImpliesParked` | PENDING 等待的属主一定处于 `WAITING`（drain 恒可用） | `submit_input`/`close_epoch_execution` 取消等待后才置 `READY` |
| `UnusedSlotHasNoAnswer` | 未使用的等待槽没有回答 | 等待行按决策创建 |
| `NoStrandedPending`（时序） | 条件成立且未终结的 PENDING 等待终会被关闭（drain 满足或被取代取消），不会永久悬挂 | 停放 drain（弱公平：driver 轮询）+ 取代路径 |

### 任务、委派与目标结清（A02/A09/A16）

| 性质（规格） | 含义 | 代码锚点 |
|---|---|---|
| `TypeOK` | 任务/目标/操作/生命周期取值合法 | `tasks.status`、`goals.status`、操作状态集 |
| `SystemOnlyParksTasks` | 系统自己不结清也不取消任务，唯一的任务写是"停放为 BLOCKED" | `complete_task`/`cancel_task` 拒绝 System；`park_tasks_for_unknown` |
| `OnlyPartiesWriteTasks` | 只有承接者、委派者、用户或系统（停放）能改任务 | 四个动作的身份守卫；委派者即 requester |
| `SettledIsFinal` | 终态任务不可改写（`SUCCEEDED/FAILED/CANCELLED` 一步到位且不再变） | `complete_task`/`cancel_task` 的终态分支 |
| `ReturnPathOnlyWhileOpen` | 窄返回能力只在任务未结清时存在；`SUCCEEDED/FAILED` 与取消都会撤销，`BLOCKED` 保留 | `revoke_grant_tree` 的调用点；`terminal = SUCCEEDED\|FAILED` |
| `DependenciesPointBackwards` | 依赖边只指向更早创建的任务 ⇒ 依赖图**按构造无环** | `delegate_task` 要求 `dependency` 已存在 |
| `NoSelfDependency` | 任务不依赖自己 | `delegate_task` 的自依赖检查 |
| `NoOpenTaskOnDeadAssignee` | 已终止实例名下没有未结清任务 | `set_lifecycle` TERMINATED 的级联取消 |
| `NoStaleActiveGoal` | 没有任何实例指向已结清的目标（结清即摘除指针 + 记账只认 ACTIVE） | `detach_goal`、`budget_goal` 的 `goal_is_active` 过滤（V-G1 修复） |
| `RegisteredWorkNeedsAnActiveGoal` | 委派只落在 ACTIVE 目标上（监测变量 `lateTask`） | `delegate_task` 的 `goal_is_active` 守卫（V-G1 修复） |
| `RequestsResolveToActiveGoals` | 请求解析到的目标必定 ACTIVE（监测变量 `lateRequest`） | `budget_goal` 的 `goal_is_active` 过滤（V-G1 修复） |

任务模块只断言安全性：任务能否推进取决于环境（成员的回合），方案不要求系统替用户结清，故不写活性。

## 建模过程中的三项发现

1. **预算性质必须写成"准入闸门 + 预留上限"**，不能写成"实际用量绝不超限"：模型里 `known` 由供应商标注的用量结算、
   不经闸门，`BeginRequest` 才是闸门。这与方案 §8 的说法一致（"供应商计费不完整时不给'绝不超额'的虚假保证"）；
   朴素写法会被 TLC 立刻反证。
2. **结算后的目标仍会被继续记账**：TLC 先反证了"结算后目标记录不再变化"，核对代码确认
   `reserve_budget`/`settle_usage` 都没有目标状态检查，而 `begin_request` 用 `active_goal_id` 解析目标——
   即同一会话里目标完成后的新回合仍会记到那个已 `SUCCEEDED` 的目标上。预算闸门仍有效（不会超发），
   但"目标状态"与"后续用量"不再一致。**这是一条需要产品决策的边界**（是否要求新回合另建目标），因此模型
   如实保留该行为，只断言"终态状态不可改写"。
3. **`CANCELLED_BEFORE_START` 的语义**是"效果未发生"而不是"未派发"：代码对一个已派发但未启动的操作取消时
   正是这个状态，因此不变量必须约束 `effect = 0`。

### 发现 V-W1（已修复，2026-09-24）

**等被解决后必须回答它自己的 `wait` tool_call——修复前有两条路径不回答。**

修复：新增 `answer_closed_waits`，把答案推广到两条非 drain 出口——注册即满足（`import_response` 内、
`wait_reason` 与 drain 同格式）与被取代/关闭 epoch（`submit_input`、`close_epoch_execution`）。
去重键仍是 wait id，重放不会追加第二条。规格侧由不变量 `ResolvedWaitIsAnswered` 守着；
回归测试 `wait_call_answered_outside_the_drain_path`（注册即满足、被取代两条路径断言回答存在且
只追加一次，取代回答含 `superseded`）。

以下为修复前记录的反例与探针证据。

严格线协议端点（OpenAI 风格 Responses、Anthropic）拒绝"assistant `tool_calls` 没有对应 tool 响应"的请求，
仓库自己的注释也这么写（`core/src/kernel/mod.rs`、`wake_satisfied_at` 的说明）。代码只在 **drain 路径**
（`wake_satisfied_at` 扫 `PENDING` 并追加回答）上配对，另外两条路径没有回答：

1. **注册即满足**（`import_response` 里注册时 `evaluate_wait` 直接判为 satisfied 的那条，正是 A23 用来
   "不丢唤醒"的分支）：wait 直接落 `SATISFIED`、实例不 park，之后没有任何地方为它追加 tool_result；
2. **被取代**（`submit_input` 把该实例的 `PENDING` 等待整批置 `CANCELLED`，`close_epoch_execution` 同理）：
   回答也不追加，实例带着一条未回答的 `wait` 调用进入下一回合。

证据：

- 规格反例（可复跑）：`make verify-model-contract` → `Error: Invariant ResolvedWaitAnswersItsCall is violated`，
  轨迹为 `ArmWait(PENDING)` → `Supersede` → `CANCELLED` 且 `answers = 0`；
  注册即满足那条由 `ArmWait` 的 satisfied 分支同样触发（`answers` 保持 0）。
- 代码探针（`cargo test --offline --manifest-path core/Cargo.toml --lib wait_call_answer_gap_outside_the_drain_path -- --nocapture`）：

  ```text
  PROBE A: satisfied=true phase="READY" answers_for_wait_1=0   # 注册即满足，无回答
  PROBE B: phase_after_import="WAITING" wait_state=CANCELLED phase_after_input="READY" answers_for_wait_2=0  # 取代，无回答
  ```

影响与修复方向（待用户确认后落码）：严格端点下这两条路径的下一次请求会被拒；宽松端点（本机评测用的
DeepSeek chat-completions）容忍，所以真实评测没暴露。修复即把"回答"从 drain 路径推广到这两条路径
（注册即满足时追加同一格式的答案；取代/关闭 epoch 时给被取消的等待追加上下文回答）。

### 发现 V-G1（已修复，2026-09-24）

**目标进入终态后不再接受新工作。** 修复前，委派任务、开新操作、继续记账三条路径都不看目标状态：

- `delegate_task` 只校验承接者与（实例委派时的）委派者活跃、`goal_id` 存在，没有"目标必须 ACTIVE"；
- `import_response` 开操作时 `goal_id` 来自 `request_goal(active_goal_id)`，同样不看目标状态；
- `reserve_budget`/`settle_usage` 也不看目标状态（这条即上面"发现 2"）。

规格反例（可复跑，两条契约都在 `MC_task_contract.cfg` 里，TLC 先报第一条）：

```text
Error: Invariant RegisteredWorkNeedsAnActiveGoal is violated.     # 目标还没建就委派了任务
Error: Invariant ClosedGoalTakesNoNewOperation is violated.       # 终态目标仍开新操作
```

修复（用户确认"全部修复"后落码）：

- `budget_goal` 只把 **ACTIVE** 目标作为记账目标（实例的 `active_goal_id` 与其最旧开放任务的目标两条路径）；
  解析不到就按"无目标"运行（与无目标会话同一模式）；
- `complete_goal`/`block_goal` 结清时摘除所有指向该目标的实例指针（`detach_goal`，返回 `detached` 计数）；
- `delegate_task` 要求目标 ACTIVE，否则明确报错并提示先 `create_goal`；
- 规格侧由 `NoStaleActiveGoal`、`RegisteredWorkNeedsAnActiveGoal`、`RequestsResolveToActiveGoals` 守着；
  回归测试 `a_settled_goal_takes_no_new_work`（摘除指针、委派被拒、新请求不记账、新目标恢复记账与委派、
  已结清记录不再变化）。

建模结论（写进 `V2Task.tla` 头注）：**"新工作"的线性化点是请求（`begin_request`），不是操作**。请求在目标
ACTIVE 时被准入，之后目标结清，它仍会开操作并把用量结算到那个目标——这是诚实记账，不是新工作；因此
性质写成请求级（`RequestsResolveToActiveGoals`）而不是操作级。

已知边界（未强制，见 `V2Task.tla` 头注）：`complete_goal` 只检查开放操作、不检查任务，所以目标可以在
自己名下的任务仍开放时结清；那些任务继续运行，其后续请求没有记账目标。要收紧（结清前必须结清任务）
需要先给 driver 一个"拒绝完成"的已提交结果，属于后续工作。

### 建模过程中另外两条（性质表述本身的修正）

4. **同事务翻转必须写进性质**：制品首次引用是在 `STAGING → LIVE` 的同一步里附上的，因此
   "引用只能附着到 LIVE 制品"这种朴素写法会被 TLC 立刻反证——正确表述要允许 `row' = LIVE`。
5. **引用计数不能用无界整数**：`refs++` 会让状态空间发散（实测 1.8 亿状态仍未收敛）；改成
   **有限持有者集合**（`Owners` 常量）后同一配置只有 64 个可达状态。这条对后续模块同样适用。

### 上下文压缩（A20）

| 性质（规格） | 含义 | 代码锚点 |
|---|---|---|
| `TailAppend` | 条目占据槽位前缀：新条目只追加在尾部，不插入中间 | `append_entry` 的 `MAX(idx)+1` |
| `NoEntryIsEverLost` | 原文永不删除（覆盖只是视图事实，监测变量 `lost` 保持空） | `compress_context` 只写 `compressed_by`，从不 DELETE |
| `CoveragePointsForward` | 总结永远比它覆盖的条目新 | 先追加总结（尾部）再标记覆盖 |
| `CoverageNeverLifted` | 覆盖只增不减、不会被改指到另一个总结（监测变量 `uncovered` 保持空） | 覆盖语句带 `compressed_by IS NULL` 守卫 |
| `NewestSummaryIsVisible` | 最新总结自身不会被覆盖（更早的总结可以被更晚的总结覆盖） | 提交顺序 |
| `CoveredStaysCoveredByItsSummary` | 被覆盖的条目一定指向一个更晚的真实总结 | 同上 |
| `ClosedCompressionReleasesReservation` | 压缩请求关闭（完成 / 失败 / 被 epoch 关闭取消）都释放预留 | `compress_context`/`fail_compression`/`close_epoch_execution` 里的 `release_reservation` |

压缩请求的**准入**（生命周期、目标截止时间、预算闸门）与 turn 请求同一条代码路径，由 `V2Control`
的 `AdmissionGate` 覆盖，此处不再重复建模。

### 会话 daemon 协议（A28）

| 性质（规格） | 含义 | 代码锚点 |
|---|---|---|
| `LogMonotone` | 事件日志只增不减：版本不重用、不回滚（监测变量 `shrank` 保持 FALSE） | `events.sequence` 自增；`read_events(since)` |
| `AppliedAtMostOnce` | 一个 command id 至多生效一次 | `submit_inner` 的 `commands` 表去重 |
| `ReceiptsAreStable` | 已存回执不再改写；同 payload 的重放返回**存的**那份回执（监测变量 `drift` 保持空） | `submit_inner` 命中已有 command_id 时原样返回 `result_json` |
| `ReceiptNamesARealVersion` | 回执指向的版本真实存在 | 同上 |
| `AppliedCommandsUsedTheWireVersion` | 只有握手通过的协议版本能提交命令 | `PROTOCOL_VERSION` 检查 |
| `SnapshotNeverLeadsCursor` | 快照声明的版本绝不领先于客户端持有的水位——这正是"快照+水位同一读事务"买到的东西 | `daemon.rs` 的 `checkpoint`（`unchecked_transaction` 里同时读快照与 `MAX(sequence)`） |
| `ViewMatchesCursor` / `CursorNeverBeyondLog` | 断连重连后视图与游标一致、没有缺口 | `events` 返回 `sequence > since` 的全部事件 |
| `NoResyncInThisVersion` | 这一版事件永不回收，`resync_required` 恒为 false（`pruned` 从不置真） | 头注："Events are never reclaimed in this first version" |

`SnapshotNeverLeadsCursor` 是**非空性质**：把 `checkpoint` 拆成"先写快照、再写水位"两步（即不是同一读事务），
TLC 立刻反证（实测 `Error: Invariant SnapshotNeverLeadsCursor is violated`）。慢客户端不阻塞写者的部分是
结构性的：`RuntimeEvent` 不依赖任何客户端游标，因此这里不写活性性质。

### 必需检查（A16/§8）

| 性质（规格） | 含义 | 代码锚点 |
|---|---|---|
| `SuccessRequiresAllChecksPassed` | 只有**每个必需检查都真的通过**的轮次才能把目标收成 SUCCEEDED（性质写在**观察到的结果**上，不是写在"结论"变量上） | `step_completion_checks`：`failures.is_empty()` 才 `complete_goal` |
| `NoUpgradeOfTheCandidate` | 运行时不升级模型候选：承认未交付的候选永不会 SUCCEEDED（监测变量 `nonSuccessSuccess`） | 只有候选自称 success 才进校验；`complete_goal` 用**存储的**候选结清 |
| `ChecksOnlyVerifyAClaimedSuccess` | 不跑检查的候选直接按自身结果结清（监测变量 `lateRound`） | `step_completion_checks` 只在 `outcome == "success"` 时进入 |
| `RoundsAreMonotone` / `RoundsAreBounded` | 轮次只增、且不超过预算（监测变量 `rewound`） | `rounds >= max_rounds` 分支 |
| `BlockedAfterTheBudgetOrStale` | 自称成功的候选落 BLOCKED，只可能是"预算耗尽"或"校验路径不可用（陈旧观察）" | `infra`（`dispatch_refused`/`spawn`）与 `stale_inputs` 分类；`block_goal` |
| `NoUnverifiedSuccess` | 没有未经校验的成功（监测变量 `upgrades`） | 同上 |

非空证据：把 `Accept` 放宽成"有一个 pass 就接受（哪怕同时有 fail）"，TLC 立刻反证
`SuccessRequiresAllChecksPassed is violated`；把这条性质写成"SUCCEEDED ⇒ 记录的结论是 pass"则是**空性质**
（动作自己就能写 pass），所以最终断言绑在观察到的结果上。

## 规格↔代码的可执行对应（`core/tests/v2_invariants.rs`）

规格检查的是抽象状态机。`core/tests/v2_invariants.rs`（随 `make check` 自动运行）把**同一组不变量**在真实
`core::v2::Control` 上重算一遍：

```bash
cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants
```

- **穷举**：长度 ≤ 2 的命令序列，每条从全新数据库开始（36 种命令 ⇒ 1,332 条序列），含被拒绝的组合；
- **随机游走**：60 条固定种子的 24 步游走，每步只在"当前可用"的命令里挑，并优先挑本次游走用得最少的
  命令种类（覆盖驱动，否则会反复做同一件安全的事而走不到深层链路）；种子固定 ⇒ 轨迹可复现；
- **每步之后重查**：`TypeOK`、`SettledIsFinal`、`ReturnPathOnlyWhileOpen`、`DependenciesPointBackwards`、
  `NoOpenTaskOnDeadAssignee`、`NoStaleActiveGoal`、`ReservationReleased`、`OneActiveRequest`（只对 turn
  请求计数：压缩请求并发且不动相位）、`SelectionIsComplete`、`ResolvedWaitIsAnswered`、
  `NoEffectBeforeApproval`、`LiveIsPersisted`、`TailAppend`、`NoEntryIsEverLost`、
  `CoveragePointsForward`、`CoverageNeverLifted`、`NewestSummaryIsVisible`、`ApprovalDecisionIsFinal`
  （决定落下后不再改写，PENDING → 过期合法）、`PendingApprovalOnlyForPreparedOperation`（RT-06：
  操作终结后不得留下待批）、`NoEffectAfterDenial`、`ReceiptsAreStable`（同一 command id 的存量回执
  不再改写）、`ReplayedCommandIsInert`（重放步骤必须不动命令/事件/上下文三张表）、
  `LogMonotone`（事件日志只增不减）、上下文 epoch 一致性；
- **覆盖率断言**：游走必须真的走到"等被解决 / 目标结清 / 任务结清 / 操作终态 / epoch 重置 / 实例终止 /
  制品 LIVE / 压缩提交 / 批准已决定 / 命令重放（同 id 返回存量回执、异 payload 被拒）"，否则测试失败
  （防止"空转通过"）；
- **反向验证**（`the_invariant_checker_detects_broken_states`）：人为破坏状态（未知状态值、终态被改写、
  悬挂目标指针）时检查器必须报出来，否则"全部通过"没有意义。

这条可执行对应已经抓到两处代码问题（V-P1 与 V-P2，见下），并覆盖多个"必须被拒绝"的反例探针
（委派到已结清目标、非承接者结清任务在代码里都必须被拒绝）。

边界：这是**有界穷举 + 采样**，不是证明；它检查"实现状态是否满足不变量"，不检查活性，也不覆盖并发交错
（`Control::submit` 在单个连接上串行，交错属于 driver 层）。

### 发现 V-P2（代码级不变量测试发现，已修复）

随机游走走出了"把压缩请求当回合导入"的路径：`import_response` 只检查请求是否 `PENDING`，不检查
`kind`，于是一个压缩请求可以被当成回合导进上下文——追加 assistant 条目、开操作、按回合收尾，而压缩
请求本来只该由 `compress_context` 用一段总结提交（§7/A20）。驱动不会这么做，但控制面没有拒绝。
修复：`import_response` 拒绝 `kind != 'turn'` 的请求并提示用 `compress_context`；回归
`import_response_refuses_a_compression_request`。

### 发现 V-P1（代码级不变量测试发现，已修复）

终止实例时 `close_epoch_execution` 取消了在途请求，但 `set_lifecycle` 的 TERMINATED 分支没有像
`reset_instance`/`fail_request` 那样把执行指针归零：实例停在 `phase = MODEL_PENDING`，
`active_request_id` 指向一个已 `CANCELLED` 的请求，于是"phase 为 `MODEL_PENDING` ⇒ 存在 PENDING 请求"
在已终止实例上不再成立。修复：终止分支补上与 reset/fail 相同的归一化（phase → READY、指针清空），
回归测试 `terminating_an_instance_normalizes_its_execution_pointer`。

## 纯函数层的有界穷举（`core/tests/kernel_properties.rs`）

内核里不碰数据库的那部分（线协议视图、输出裁剪、分页、响应分类）用有界穷举/枚举直接检查，同样随
`make check` 运行：

```bash
cargo test --offline --manifest-path core/Cargo.toml --test kernel_properties
```

| 检查 | 性质 |
|---|---|
| `wire_view_is_a_paired_permutation` | `prepare_request` 的输出：系统提示在最前、其余是输入条目的**置换**（不丢不重）、有回答的调用后面**紧跟**它的回答（R22 配对）、assistant 之间保持原序。穷举长度 ≤ 3 的全部 258 种条目组合 + 两个长用例，并断言"真的搬动过 ≥ 10 次"（否则这条性质是空转） |
| `tool_output_cap_keeps_head_and_tail_within_bounds` | 不超上限不改写；超上限后长度有界（≤ 上限 + 64 的截断标记）、保留首尾、明确标记截断 |
| `paging_reconstructs_the_original_without_gaps` | `page_output` 逐页取回能**无缝重建原文**（长度 0..12 × limit 1..5 全枚举）；坐标自洽（`next_offset` = 已消费长度、`eof` 时无下一页坐标）；非法参数与越界明确报错，不静默截断 |
| `response_classification_is_exhaustive` | `interpret_response`：单独 finish → 完成候选；单独 wait → 等待；与别的调用混用 → 忽略并记协议注释、其余照常成为意图；空响应 → 普通回复 |
| `args_hash_is_deterministic` | 同样参数永远得到同样的 `args_hash`（收据、去重与重放都依赖它） |

两处边界（如实记录，不是缺陷）：

- 裁剪对"刚超过上限一点点"的输入会**变长**（首尾 + 截断标记，最多 +64 字符）；真正的收缩发生在远超上限时。
- `pair_tool_results` 只把回答**上移**到调用之后，不会下移：回答先于调用的顺序在真实日志里不可能出现
  （运行时先追加调用），所以那条路径只用"置换"性质覆盖。

## 边界（诚实说明）

- 已验证的是**模型**性质：TLC 穷举的是抽象状态机，不是 Rust 实现。除非做精化证明（后续阶段的可选工作），
  不能据此声称"Rust 代码已被证明"。
- 已建模：控制面状态机、制品与 GC（A30）、等待/唤醒/计时器/取代（A22/A23、RT-06 的去重语义）、
  任务/委派/目标结清（A02/A09）、上下文压缩（A20）、daemon 协议的命令去重与快照水位（A28）、
  必需检查轮次与修复/阻断（A16）。
- 尚未建模：审批 `expires_at` 的到时判断（决定终态与"操作终结后不得留下待批"已在代码级不变量测试里
  覆盖，但未单独建 TLA 模块）；`execute_check_ops` 的检查命令执行细节（派发/超时/重连）只按"轮次与结论"
  抽象。多实例共享预算的跨实例结算（A18 的 worker 归属）已在 `V2Task` 里按 `budget_goal` 的解析规则
  建模（含"只看最旧开放任务"的取序细节）。
- 弱公平假设：`V2Wait` 的活性依赖"停放 drain 弱公平"，即 driver 的轮询循环在 `WAITING` 下持续尝试
  （`engine/src/v2/driver.rs`）；这是实现事实，不是被证明的结论。
- 状态空间前沿（`MC_task`）：1 任务 / 2 实例 / 2 目标 = 5.7M 状态 / 约 20 秒；把任务加到 2 个会发散
  （实测 43M 状态、4 分钟未收敛），需要对称性或更强的抽象。
- 代码级对应（`core/tests/v2_invariants.rs`）是采样 + 有界穷举，不是证明；它给不出"所有执行都满足"，
  只给"这些执行都满足"+ 检查器灵敏度（反向验证）。
- 状态空间前沿：宽配置 275M 状态 / 11 分钟；继续加实例或操作数需要对称性/约束或改为随机模拟
  （`-simulate`）作为补充。

## 结论与台账

- [REPORT.md](REPORT.md)：形式化验证的结论（能声称什么 / 不能声称什么）、证据清单、A01–A36 逐项台账、
  未证明清单与可推翻条件。

## 后续阶段（本目标的剩余工作）

1. 扩充规格覆盖上表"尚未建模"的协议面，并为每条新增性质补映射与验收编号。
2. 代码级不变量测试：在真实 `core::v2::Control` 上用随机命令序列（proptest）断言同一组不变量，
   使"规格↔代码"从纸面对应变成可执行对应。
3. 纯函数层的形式化：内核里的算术/集合性质（预算求和、分页、等待条件求值、路径越界）用 Lean 4 或
   Rust 侧有界验证（Kani/loom 视适用性评估）。
4. 验证报告：把"已证明/未证明/假设"分列，随 `docs/ACCEPTANCE.md` 一起维护。
