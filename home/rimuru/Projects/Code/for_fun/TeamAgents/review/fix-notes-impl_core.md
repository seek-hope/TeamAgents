# fix-notes-impl_core（P0-4 + RT-06）

任务：task_834e30eeb005（接续 task_d03e6435981b）。文件范围：control.py / storage.py / runtime.py / permissions.py + 新增 tests。
基线（本机实测，2026-09-12）：`83 passed / 2 env-failed / 12 deselected`（impl_config 新增 7 例后）。

## 复现（修复前）

脚本：`review/tmp/repro_p0_4_rt06_impl_core.py` → 输出 `review/tmp/repro-p0_4-rt06-impl_core.txt`

```
== P0-4 ==  receipt ok=False, but committed entries: ['partial-write'];
            同一 action_id 重试仍拿失败回执（状态已脏，无法自愈）
== RT-06 == cancel_run 后: run 仍 WAITING_APPROVAL, approval 仍 PENDING;
            blockers 仍含 'active turns: ... WAITING_APPROVAL' + 'pending approvals: appr_...'
```

## 修复设计

### P0-4（control.py::submit）

- 动作尝试（dedup 读 → validate → reduce → persist_events → schedule → record_action）包在
  `_submit_once()` 的单事务里；异常在事务 `__exit__` 中 ROLLBACK，随后在**新事务**里写失败回执。
- 失败回执仍按 action_id 落库：同 id 重试拿回失败回执（幂等）；换新 id 在干净状态上重试。
- 校验错误（无写）依旧在同事务内写回执。

### RT-06（storage.py / control.py / runtime.py）

- `storage.expire_approval`：泛化 `APPROVED_ONCE→EXPIRED` 为 `(PENDING|APPROVED_ONCE)→EXPIRED`。
- 新增 `storage.expire_run_approvals(run_id) -> list[ApprovalRequest]`：把某 run 的 PENDING 批准全部置 EXPIRED（自身开事务）。
- `control.control.expire_run_approvals(run_id) -> list[EventDraft]`：过期 + 产出 `APPROVAL_DECIDED(status=EXPIRED)` 审计事件。
- `runtime._finalize`：run 到终止态（COMPLETED/FAILED/CANCELLED/OUTCOME_UNKNOWN）时 expire 该 run 的 PENDING 批准并入列事件。
- `runtime.reconcile`：外部回合不可核对 → OUTCOME_UNKNOWN 前 expire；（`state is not None` 且为终止态时同样 expire）。
- `runtime._request_stop` 超时（RUNNING→OUTCOME_UNKNOWN）同样 expire。
- `control._schedule`：新增前置收敛——`WAITING_APPROVAL` 且 `cancel_requested` 的停等回合无 executor 收尾，
  由 control 收敛为 CANCELLED（expire 批准、取消其未决任务、置成员 IDLE、ack 投递、发 RUN_CANCELLED/APPROVAL_DECIDED 事件）。
- `control._cancel_task`：目标成员回合停在 `WAITING_APPROVAL` 且属于该任务时，改用「请求取消该回合」而不是直接把任务标成 CANCELLED（否则回合永远停等）。
- 已 EXPIRED 批准的 `APPROVAL_DECISION`：`_validate` 已有 `approval is already EXPIRED` 干净失败回执，补测试固化。

## 状态

- [x] 复现（P0-4 + RT-06）
- [x] P0-4 代码
- [x] RT-06 代码
- [ ] 测试 + 全套件回归
- [ ] 报告（改动行号/命令输出/语义/遗留风险）

## 假设与遗留风险（初稿）

- RT-03（取消真中断）不在本任务：CUDA 侧「取消 RUNNING 回合」仍按既有语义走 runtime `_watch_cancellations`；
  本修复只处理**已停等**（无 executor）的 WAITING_APPROVAL 回合，不改变 RUNNING 回合的取消时机。
- 重启后遗留的 WAITING_APPROVAL 回合（RT-04/AD-7，reconcile 不覆盖 WAITING_*）不在本任务；
  本次只在「取消/终结」路径消除残留批准。若未来要做，可在 reconcile 中补 WAITING_* 的收敛。

## 结果（Leader 复跑，2026-09-12 12:2x）

- 新测试：`.venv/bin/python -m pytest tests/test_p0_4_reduce_rollback.py tests/test_rt06_approval_expiry.py -q` → **16 passed**。
- 全套件：`.venv/bin/python -m pytest tests/ -q` → **109 passed, 2 failed, 12 deselected**；两个失败与基线完全相同（缺 DEEPSEEK_API_KEY、无 DNS），无新失败。
- 代码审读：submit 重构为 `_submit_once` + 外层 except 新事务记失败回执（回滚在 tx.__exit__）；expire_approval 泛化为 (PENDING|APPROVED_ONCE)→EXPIRED；expire_run_approvals + control.expire_run_approvals 审计事件；_finalize/reconcile/_request_stop 终止路径接入；_schedule 对 cancel_requested 的 WAITING_APPROVAL 回合收敛；_cancel_task 对停等回合改走取消请求。
- 状态：实现与测试完成；本文件由 Leader 补齐结果段（原回合在补报告前撞上 step 上限，任务已不可由承接者关闭）。

## Leader 补记（2026-09-12 13:3x）：F-C3 半成品回归与两处修复（已落盘）

批次 2-β 的 F-C3 回合（task_183c013faee7）在 step 50 处死亡，留下半成品（`_offered` 记录 +
仅终态 ack）。该半成品使 3 个既有测试回归、并留下一个未被覆盖的投递缺口。Leader 定位并修复：

### 回归 1：停等回合被自己的旧输入"虚假唤醒"

- 现象：`tests/test_p3_deepagents_runner.py::test_deepagents_delegation_end_to_end`、
  `tests/test_p4_topology.py::test_mid_execution_supplement_reaches_running_leader`、
  `tests/test_t2_parallel.py::test_t2_parallel_with_barrier_and_supplement` 失败
  （117 passed / 5 failed）。
- 根因：终态才 ack 使「已注入但未 ack」的投递在回合停等（WAITING_TASK）期间保持 pending；
  `control._schedule` 停等唤醒条件 `user_input = any(kind == USER_MESSAGE for d in pending)`
  把该回合自己的旧输入当成新用户输入 → 停等回合被立刻续跑（Leader 在任务未完成前走到
  signal_done；supplement 也被跳过）。
- 修复（control.py `_schedule` 停等分支）：只有不在 `waiting.input_delivery_ids` 里的新投递
  才算唤醒输入：
      seen = set(waiting.input_delivery_ids or [])
      user_input = any(d["event_kind"] == EventKind.USER_MESSAGE
                       and d["delivery_id"] not in seen for d in pending)
- 复跑：3 个回归测试全绿。

### 缺口 2：RUNNING 中途 push 的投递永不 ack → 重复投递

- 现象（Leader 复现）：成员回合 RUNNING 期间用户发 supplement → `_schedule` active 分支
  append+`mid_turn_pushes` → 回合结束（终态）时 `_ack_injected` 只 ack `view.delivery_ids`
  （不含 push 的 id）→ 该投递保持 pending → 下一轮调度为它新建第二个回合，输入被重复处理。
- 修复（runtime.py `_drain_mid_turn`）：push 真正交给 runner 后，把 run 的输入集合并入
  `_offered`（push 也是注入），回合终结时一并 ack：
      if runner is not None:
          runner.deliver_mid_turn(run_id, items)
          if run is not None:
              self._offered[run_id] = (self._offered.get(run_id, set())
                                       | set(run.input_delivery_ids))
- 复现测试：`tests/test_fc3_mid_turn_ack.py`（Leader 落盘；单回合、两条投递均 applied、
  supplement 在下一模型调用可见）。

### 当前状态

- 全套件：**121 passed / 2 env-failed / 12 deselected**（两失败为既有环境项；
  121 = 原 120 + 本文件新增 `tests/test_fc3_mid_turn_ack.py` 1 例）。
- 说明：因回合已死、套件全红且修复点唯一并已实测，两处修复由 Leader 直接落盘；
  impl_core 收尾 F-C3 时请**复核语义**（可微调），并按下述任务补全原子性/精确 ack 测试与
  C-b 对照。control.py / runtime.py 仍归 impl_core。

## F-C3 + RT-05 收尾（task_43231fb39a31，impl_core）

### 复核 Leader 的两处修复

1. `control.py:_schedule` 停等唤醒（1034-1037）：`seen = set(waiting.input_delivery_ids or [])`，
   只有**不在**该回合输入里的新投递才按 `USER_MESSAGE` 唤醒。语义正确：停等回合不会被自己的旧
   输入虚假唤醒；新 supplement 仍是新投递 → 正常唤醒。无异议，未改。
2. `runtime.py:_drain_mid_turn`（250-264）：push 交给 runner 后把 `run.input_delivery_ids` 并入
   `_offered`（push 也是注入）。语义正确（否则 RUNNING 中途 push 的投递永不 ack → 下一轮重复
   投递）；无异议，保留。

### 我的微调（runtime.py，均为语义修正）

- `_ack_injected`（595-607）改为**只读** `_offered`（原来 `pop`）：ack 在终态事务内执行，若事务
  因任何原因回滚，注入台账必须保留——重试（`_execute` 异常路径或调度重跑）才能以同一证据收敛。
  原实现在 ack 抛错时先丢掉台账，导致重跑时"无证据可 ack"→ 投递永远 pending → 重复投递。
- `_finalize`：终态 ack 仍在事务内（548）；**提交成功后**才 `pop` 台账（554）。失败即回滚 →
  run 状态/成员状态/投递状态全部保持原值。
- `_execute` finally 兜底 `pop`（371）：双故障（终态重试也失败）时不泄漏台账；该 run 被调度重跑
  时会从视图（`pending_deliveries`）重建台账，语义不变。

### 新增测试 `tests/test_fc3_delivery_ack.py`（4 例）

| 测试 | 断言 |
| --- | --- |
| `test_ack_failure_rolls_back_terminal_state_then_converges_once` | ack 抛错 → run 仍 RUNNING、成员 BUSY、投递 pending（无「终态+未确认」窗口）；其间重排不新建回合；解除打桩后重跑收敛：COMPLETED、投递 applied、全程仅 1 个 run |
| `test_uninjected_deliveries_stay_pending_and_are_delivered_once` | 无 runner 的回合在建立视图前失败 → 两个批次投递均保持 pending（旧代码按 `MAX(batch_no)` 全部 applied）；恢复 runner 后恰好一个回合各投递一次（不丢不重） |
| `test_manual_terminal_unacked_input_is_injected_exactly_once` | 手工构造旧崩溃窗口（COMPLETED + pending）→ 重排后投递恰好注入一次、无第二次重复 run（旧窗口遗留态按 §7「至少一次」语义重投，不丢） |
| `test_ack_deliveries_exact_never_swallows_unlisted_pending` | 存储层：精确 ack 只改列出的 id，绝不按 batch 范围吞掉未列出的 pending；`last_applied_batch` 只推进到所列 id 的最大批号 |

### 修复前对照（前置快照 `.pre-fix-backup/src-tests-20260912-1203.tar.gz`）

- `tests/test_fc3_delivery_ack.py` + `tests/test_fc3_mid_turn_ack.py` 对旧代码：**4 failed / 1 passed**。
  失败的实质：`uninjected` 例为 `applied != pending`（正是 MAX 范围过度 ack 导致静默丢失）；
  另两例为 `Store` 无 `ack_deliveries_exact`（新 API 缺失）；Leader 的 mid-turn 例为重复 run。
- `review/tmp/probe_c2.py` C-b（手工构造旧窗口）在前后代码输出**相同**：
  老代码的写入顺序会制造该状态，DB 本身无法区分「已注入但未 ack」与「从未注入」；
  修复后该状态不再由 `_finalize` 产生（见下），手工构造的遗留态按至少一次语义重投。

```
# before（旧代码，快照）：C-b2 run=COMPLETED + delivery(1)=pending；
#   C-b3 下一轮调度新建 run 且 inputs=[1] → 同一输入再次排队
# after（新代码）：同样的手工构造仍会重投（不丢），但 live 路径无法产生该状态：
review/tmp/probe_fc3_atomic.py（输出 review/tmp/probe-fc3-after.txt）
finalize raised: ack write failed
run status after failure: RUNNING | agent: AgentStatus.BUSY
deliveries after failure: [(1, 'pending')]
runs after the next scheduling pass: 1
run status after retry: COMPLETED | deliveries: [(1, 'applied')]
total runs: 1 | pending deliveries: []
```

### 命令与结果

- `.venv/bin/python -m pytest tests/test_fc3_delivery_ack.py tests/test_fc3_mid_turn_ack.py tests/test_p0_4_reduce_rollback.py tests/test_rt06_approval_expiry.py -q` → **21 passed**。
- 全套件 `.venv/bin/python -m pytest tests/ -q` → **125 passed / 2 env-failed / 12 deselected**（基线
  121/2/12 + 本任务 4 个新例；两个失败与基线完全相同：缺 `DEEPSEEK_API_KEY`、无 DNS）。

### 遗留风险（F-C3/RT-05 范围）

- **窄口径的"交给 runner 即视为注入"**：push 在交给 runner 后若回合在下一次模型调用前终结，该
  投递会被 ack 而模型从未看到。要彻底关闭需要 runner 在真正注入时回传确认（`abefore_model` 的
  drain / checkpoint `applied_batch`，runners.py:688-690 只在重启 reconcile 用），超出本任务文件
  范围（runners.py 属防护区）；选择「宁可重复投递也不丢」的对齐点是注入台账（视图/push hand-off）。
- 旧崩溃窗口遗留库（终态+未确认）升级后仍会重投（至少一次语义，不丢但可能重复一次）；未加额外
  「已见」标记以保持最小改动（避免脚手架）。
- `_converge_cancelled_waiting_run` 仍用 `ack_run_deliveries(run)`（精确集合 `input_delivery_ids`，
  已无批量范围）；停等回合的输入在段首视图里出现过，取消时 ack 与其语义一致。

### 补：第 5 例（停等唤醒条件）与最终结果

- `tests/test_fc3_delivery_ack.py::test_parked_run_not_woken_by_its_own_input_but_by_new_supplement`：
  构造真实停等（leader 派任务给 b → `wait_for_tasks` → `("wait",)`），断言：
  - 停等回合的旧输入（已在 `input_delivery_ids` 里）不唤醒它（`seen` 条件生效），停等期间投递保持 pending；
  - 新 supplement（新投递）唤醒停等回合，续跑后两个输入都被注入并恰好 ack 一次。
  - 前置快照：该例失败（旧代码在停等终结时就把输入 ack 掉，且唤醒条件不看 `seen`）。
- 最终测试计数：`tests/test_fc3_delivery_ack.py` 5 例；对旧代码 4 failed / 1 passed（1 passed 为例 3：
  旧窗口遗留态重投语义在前后代码一致）。
- 最终全套件：**126 passed / 2 env-failed / 12 deselected**（基线 121 + 5 新例；两个失败与基线相同）。
- 证据文件：`review/tmp/probe-fc3-after.txt`、`review/tmp/prefix-check-parked.txt`、`review/tmp/probe-cb-before.txt`。

## Leader 补记 2（2026-09-12 14:0x）：派发活性修复——「忙时指派的任务永不启动」

用户观察：任务板 4 pending / 2 running，但所有成员 IDLE。Leader 复现并定位为**派发活性缺陷**：

- 机制：任务指派会给被指派人发 TASK_READY 投递（唤醒通知）。若到达时该成员正在跑一个回合，
  该投递被并入当前回合（`_schedule` active 分支）并在回合终结时 ack（F-C3 语义）。
  此后**没有任何事件会再次触发派发**——任务永远停在 PENDING、成员 IDLE
  （用户所见）。复现：Leader 连续指派 t1/t2 给忙成员 b：b 的单回合吞掉两条通知；
  t1 因回合结束未 complete 变 BLOCKED，t2 永为 PENDING（修复前实测）。
- 对照：程序里已有 `_announce_ready_tasks`（一次性 ping，`task_ready_announced:` meta 去重）
  与 `_next_ready_task`（就绪谓词），但调度循环只由「pending 投递」驱动 → 通知被吞即失联。
- 修复（control.py `_schedule`，紧邻原 `if not pending: continue`）：无投递、无 active/waiting
  回合的成员，若存在就绪 PENDING 任务（`_next_ready_task`）→ 直接建 QUEUED 回合派发
  （task_id 指向该任务、input_delivery_ids=[]、受 `_turn_budget_ok` 约束）。
  语义：每成员仍「一次一个回合」；active/waiting 时先让原回合收尾，下一轮再派。
  BLOCKED 任务不在此路径（保持「等待干预」语义）；依赖未满足的任务由 `_next_ready_task` 过滤。
- 防回归：tests/test_fc3_task_dispatch.py（1 例）：忙时指派 → 当前回合结束后第二个回合
  携带 t2 并 complete → t2 SUCCEEDED、2 个回合、result_refs 正确。
  临时禁用修复该用例失败（1 run/t2 PENDING），恢复后过。
- 全套件：**127 passed / 2 env-failed / 12 deselected**（121 基线 + impl_core 5 例 + 本例 +1）。
- 待办：restart 会话以加载该修复后，旧会话中滞留的 PENDING 任务会在启动/下一次调度时
  自动派发（无需重指派）。README/USER-GUIDE 的「任务卡住」排障段落后续补。

## Leader 补记 3（2026-09-12 14:3x）：终态回合的「自己的任务」收敛——RUNNING 任务不再滞留

用户观察的另一半：「2 running」但有成员 IDLE。Leader 构造出确定性场景并定位第二种滞留：

- 机制（runtime.py `_finalize`）：回合携带 run.task_id=t1 启动时把 t1 PENDING→RUNNING；
  若成员在回合内 `complete_task` 的是**另一个**任务 t2，则：
  - 原代码 `if COMPLETED and req is not None:` 分支处理 req（t2→SUCCEEDED）；
  - 原 `elif COMPLETED and run.task_id and req is None:` **不再求值**（if/elif 链），
    t1 的收敛逻辑被跳过 → t1 永久 RUNNING + 回合已终态 + 无执行者（用户所见的 stuck RUNNING）。
- 修复（runtime.py，最小改动）：把「未完成的自身任务 → BLOCKED」从 elif 链中**独立**出来：

  ```python
  if outcome.status is TurnStatus.COMPLETED and run.task_id and (
          req is None
          or (req["task_id"] and req["task_id"] != run.task_id)):
      ... compare_and_set_task(run.task_id -> BLOCKED) ...
  elif outcome.status is TurnStatus.FAILED and run.task_id: ...
  elif outcome.status is TurnStatus.CANCELLED and run.task_id: ...
  ```

  对原行为完全兼容：req 为 None 时照旧 BLOCKED；req 完成任务就是 run.task_id 时第一分支处理、
  第二分支条件为假不重复处理；req 完成别的任务（或 `req["task_id"]` 为空即 goal-done）时不再滞留。
  注意第一版补丁曾写成 `elif`（不可达，因为 `req is not None` 的第一分支会先吞掉）——
  测试当场失败并暴露，改为独立 `if` 后通过；这就是本修复的反证记录。
- 防回归：`tests/test_fc3_task_dispatch.py::test_turn_completing_another_task_blocks_its_own_task`
  （同一回合携带 t1、complete 掉 t2 → 断言 t2 SUCCEEDED 且 t1 BLOCKED 而非 RUNNING）。
- 全套件：**128 passed / 2 env-failed / 12 deselected**（127 + 本例）。

### 宿主侧只读/维护脚本（review/tmp/，供用户排查与本轮善后）

- `inspect_live.py`（只读）：列成员状态 / 任务（标 ready | dep-blocked）/ 最近回合 / 未确认投递 /
  待批准 + 卡顿诊断（含「任务 RUNNING 但回合已终态」提示）。
- `reping_stuck_tasks.py`（写入，默认 dry-run）：清除滞留 PENDING 任务的
  `task_ready_announced:` meta，让产品在后一次调度时重新发通知（零重启恢复路径）；
  幂等（无标记则跳过）；自测：dry-run/apply/二次 apply 0 清除，均通过。
- `check_turn_limits.py` / `raise_turn_limits.py`（见早前会话记录，不动产品代码）。
