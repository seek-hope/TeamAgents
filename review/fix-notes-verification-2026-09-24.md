# 修复台账：形式化验证发现的两处问题（2026-09-24）

本轮起因：用户提出用形式化方法校验当前设计，随后确认"全部修复"。TLA+/TLC 规格穷举先给出反例，
再用代码探针确认，最后落码修复并补回归测试。规格与性质映射见
[verification/README.md](../verification/README.md)；决策记录见 `docs/DECISIONS.md` D-44。

复跑命令：`make verify-model-all`（四个模块穷举）、`make check`（格式/静态检查/全测试）。

## 1. V-W1：wait 的 tool_call 在两条非 drain 出口未被回答

- **性质**：`ResolvedWaitIsAnswered`（修复前为预期反例配置 `MC_wait_contract.cfg` 的
  `ResolvedWaitAnswersItsCall`）。
- **规格反例**（可复跑，修复前）：`Error: Invariant ResolvedWaitAnswersItsCall is violated.`，
  轨迹 `ArmWait(PENDING)` → `Supersede` → `CANCELLED` 且 `answers = 0`。
- **代码探针**（修复前）：`cargo test --offline --manifest-path core/Cargo.toml --lib
  wait_call_answer_gap_outside_the_drain_path -- --nocapture` 输出
  `answers_for_wait_1=0`（注册即满足）与 `answers_for_wait_2=0`（被取代）。
- **根因**：只有 `wake_satisfied_at`（drain 路径）会给等待的 tool_call 追加 tool 响应；
  `import_response` 的"注册即满足"分支与 `submit_input`/`close_epoch_execution` 的取消分支
  直接把等待置为 SATISFIED/CANCELLED，不回答。严格线协议端点（OpenAI 风格 Responses、Anthropic）
  拒绝 assistant `tool_calls` 无对应 tool 响应的请求——这条约束在 `core/src/kernel/mod.rs` 与
  `wake_satisfied_at` 的注释里都写着，是 R22 修复时补的 drain 版本。
- **修复**：`core/src/v2/control.rs` 新增 `answer_closed_waits`（回答落在等待自己的 call id 上；无 call
  时退化为 note）与 `wait_reason`（drain 与注册即满足共用同一段理由文本）；在
  `import_response` 的注册即满足分支、`submit_input` 的取代分支、`close_epoch_execution` 的封存
  分支调用。去重键仍是 wait id（`append_context` 的 envelope 去重），重放不会追加第二条。
- **回归**：`core::v2::control::tests::wait_call_answered_outside_the_drain_path`
  （两条路径各断言回答存在、只追加一次、取代回答含 `superseded`）；
  `the_wake_answers_the_wait_tool_call`（drain 路径）保持通过。

## 2. V-G1：目标进入终态后仍接受记在它名下的新工作

- **性质**：`NoStaleActiveGoal`、`RegisteredWorkNeedsAnActiveGoal`、`RequestsResolveToActiveGoals`
  （修复前为预期反例配置 `MC_task_contract.cfg`）。
- **规格反例**（可复跑，修复前）：`RegisteredWorkNeedsAnActiveGoal is violated`（目标尚未创建就委派
  任务）与 `ClosedGoalTakesNoNewOperation is violated`（终态目标仍开新操作）。
- **根因**：`budget_goal` 直接返回实例的 `active_goal_id`（或其最旧开放任务的目标）而不看目标状态；
  `delegate_task` 只校验 `goal_id` 存在；`complete_goal`/`block_goal` 不摘除实例指针，于是目标结清后
  新回合继续记账到已结清的目标上（预算闸门仍有效，不会超发，但"已结清"与"仍有新工作在名下"不自洽）。
- **修复**：`core/src/v2/control.rs`
  - `goal_is_active` + `budget_goal` 只把 ACTIVE 目标作为记账目标（实例指针与最旧开放任务两条路径），
    解析不到就按"无目标"运行（与无目标会话同一模式）；
  - `detach_goal` 在 `complete_goal`/`block_goal` 结清时摘除所有指向该目标的实例指针，响应与事件带
    `detached` 计数；已终态目标的迟到 finish 也走同一摘除（幂等修复）；
  - `delegate_task` 要求目标 ACTIVE，否则报错并提示先 `create_goal`。
- **回归**：`core::v2::control::tests::a_settled_goal_takes_no_new_work`
  （摘除指针、委派被拒（显式与隐式两种身份路径）、新请求不记账且不占用预留、记录冻结、
  新建并挂载目标后恢复记账与委派）。
- **语义边界（有意保留）**：新工作的线性化点是**请求**而不是操作——请求在目标 ACTIVE 时准入，之后目标
  结清，其操作与用量仍会落到该目标（诚实记账）。`complete_goal` 只检查开放操作、不检查任务，因此目标可在
  自己名下任务仍开放时结清；这类任务的后续请求没有记账目标。收紧需要先给 driver 一个"拒绝完成"的
  已提交结果，未列入本次范围；边界写进 `verification/tla/V2Task.tla` 头注与 README。

## 3. V-P1：终止实例后残留执行指针（代码级不变量测试发现）

- **发现方式**：`core/tests/v2_invariants.rs`（规格↔代码的可执行对应）在随机游走里报出
  `OneActiveRequest: instance i1 is MODEL_PENDING with 0 pending requests`。
- **根因**：`set_lifecycle` 的 TERMINATED 分支调用 `close_epoch_execution` 取消了在途请求，但没有像
  `reset_instance`/`fail_request` 那样把执行指针归零，实例停在 `phase = MODEL_PENDING`、
  `active_request_id` 指向一个已 `CANCELLED` 的请求。
- **修复**：`core/src/v2/control.rs` 终止分支补上同一归一化（`phase = 'READY'`、`active_request_id = NULL`）。
- **回归**：`core::v2::control::tests::terminating_an_instance_normalizes_its_execution_pointer`。
- **同时新增**：`core/tests/v2_invariants.rs`（穷举长度 ≤ 2 的命令序列 + 60 条固定种子游走，每步重查
  13 组不变量；带覆盖率断言与"检查器灵敏度"反向验证）。

## 4. V-P2：压缩请求可以被当成回合导入（代码级不变量测试发现）

- **发现方式**：`core/tests/v2_invariants.rs` 的随机游走（覆盖驱动后）走到 `record_attempt` →
  `import_response` 作用在**压缩请求**上并成功。
- **根因**：`import_response` 只校验请求存在且为 `PENDING`，不校验 `kind = 'turn'`，于是压缩请求也能
  走回合导入路径（追加 assistant 条目、开操作、按回合收尾）。压缩请求本该由 `compress_context` 用一段
  总结提交（§7/A20）；driver 不会这么调，但控制面没有拒绝。
- **修复**：`core/src/v2/control.rs` 的 `import_response` 在读请求时一并取 `kind`，非 `turn` 直接拒绝并
  提示改用 `compress_context`。
- **回归**：`core::v2::control::tests::import_response_refuses_a_compression_request`。

## 附带修正

- `verification/tla/V2Wait.tla`：取代语义按代码修正为"整批置 CANCELLED 并回答"，并补
  `ResolvedWaitIsAnswered`；`AnswerImpliesConditions` 拆成 `SatisfiedHoldsConditions`（唤醒才有条件
  前提）与 `AnswerImpliesResolved`。
- `verification/tla/V2Task.tla`：新增实例目标指针与请求准入（`Request`/`ImportOp`）建模，
  委派加 ACTIVE 守卫；"终态不可改写"与"不在结清目标上登记新工作"改用监测变量
  （`rewritten`/`lateTask`/`lateRequest`），因为 TLC 只接受 `<>[]A`/`[]<>A` 形式的含动作时序公式。
- `make verify-model-contract` 与其两个预期反例配置随修复删除：两条契约已并入主配置的不变量。
