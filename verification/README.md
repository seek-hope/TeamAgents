# TeamAgents 形式化验证（R2 控制面）

目标：把重构方案里**已确认的协议性质**变成机器可查的规格，而不是只靠样本测试。当前阶段验证的是
**控制面协议模型**（`core/src/v2/control.rs` + `engine/src/v2/driver.rs` 的抽象），不是 Rust 代码本身——
"规格↔代码"的对应关系在下面的映射表里逐条给出，代码级验证（真实引擎上的不变量随机测试、
必要时的 Lean/Kani）是后续阶段。

## 运行

```bash
make verify-model        # 小配置穷举（秒级；13 不变量 + 4 性质）
make verify-model-wide   # 宽配置穷举（2 实例 / 2 操作；约 11 分钟 / 275M 状态）
```

首次运行会把固定版本（TLC v1.7.1，SHA-256 见 Makefile）的 `tla2tools.jar` 下载到
`$TLA_TOOLS_DIR`（默认 `~/.local/share/teamagents-verify`）并校验；工具链不进仓库、不进 `make check`。
需要 Java（本机 OpenJDK 27）。

## 规格与配置

| 文件 | 内容 |
|---|---|
| `tla/V2Control.tla` | 控制面抽象模型：实例相位机、请求/尝试、决策与操作、批准、派发线性化点、取消/超时、epoch 重置、目标预算预留与结算、崩溃/恢复 |
| `tla/MC.cfg` | 小配置（1 实例 / 1 操作 / 2 请求槽 / 1 尝试槽 / 1 次 epoch 重置 / 1 次未知用量） |
| `tla/MC_wide.cfg` | 宽配置（2 实例 / 2 操作且其一需批准 / 3 请求槽 / 2 尝试槽） |

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

## 边界（诚实说明）

- 已验证的是**模型**性质：TLC 穷举的是抽象状态机，不是 Rust 实现。除非做精化证明（后续阶段的可选工作），
  不能据此声称"Rust 代码已被证明"。
- 尚未建模：等待/计时器（A22/A23）、制品与 GC（A30）、任务与委派（A02/A16）、压缩（A20）、多实例共享预算的
  跨实例结算（A18 的 worker 归属）、daemon 协议（A28）、审批有效期与 RT-06 的过期语义。
- 状态空间前沿：宽配置 275M 状态 / 11 分钟；继续加实例或操作数需要对称性/约束或改为随机模拟
  （`-simulate`）作为补充。

## 后续阶段（本目标的剩余工作）

1. 扩充规格覆盖上表"尚未建模"的协议面，并为每条新增性质补映射与验收编号。
2. 代码级不变量测试：在真实 `core::v2::Control` 上用随机命令序列（proptest）断言同一组不变量，
   使"规格↔代码"从纸面对应变成可执行对应。
3. 纯函数层的形式化：内核里的算术/集合性质（预算求和、分页、等待条件求值、路径越界）用 Lean 4 或
   Rust 侧有界验证（Kani/loom 视适用性评估）。
4. 验证报告：把"已证明/未证明/假设"分列，随 `docs/ACCEPTANCE.md` 一起维护。
