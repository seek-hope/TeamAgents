# TeamAgents 代码审查报告 · 核心状态与持久化域（core）

> 审查任务：`task_d207400d534b`（原 `task_528b1b39c8c1` 中断后重新派发）。审查者：`reviewer_core`。
> 审查基准：`TeamAgents-Implementation-Plan.zh-CN.md`（§2.2、§4、§5.1、§5.3、§6.4、§7、§8、§9.2、§16–17；T1–T6/T8/T10–T14/T21/T22/T24）、`docs/DECISIONS.md`（D-1、D-10）。
> 审查对象（全量精读）：`src/teamagents/control.py`（1175 行）、`src/teamagents/storage.py`（1009 行）、`src/teamagents/models.py`。
> 交叉阅读（仅用于判定语义，不作审查对象）：`runtime.py`（reconcile/_finalize/ack）、`runners.py`（thread_id、中间件注入、checkpoint 确认）、`views.py`（event_push/observer scope/AgentView）、`tests/conftest.py`、`tests/test_p1_guards.py`、`test_p2_recovery.py`、`test_p2_cancel_pause.py`、`test_p4_topology.py`。
> 只读声明：未修改 `src/**`、`tests/**`、`docs/**`、`examples/**`；审查产物仅写入 `review/**`。
>
> 本仓库**不是 git 仓库**，只读性以 sha256/mtime 证明（见 §六）。**审查期间存在外部改动**：`storage.py`/`models.py` 在 11:11 被外部修改（对齐 D-10：`Limits` 默认值重做、`load_team_spec` 容忍旧 session 的已删 limits 键），本报告以改动后的当前版本为准。草稿期的 F-C1（limits 默认值与 D-10 不一致）**已被外部修复，不再列为发现**；发现 ID 已重排。

---

## 一、范围与方法

**精读**：`control.py` 全文（事务入口 `submit`、`_validate`、`_reduce`、拓扑补丁与边界、`_sync_members`、调度、预算/限流、事件与投递落库）；`storage.py` 全文（schema/唯一约束、WAL 与事务边界、动作去重回执、事件序列、投递批次与确认、patch/run/task 读写、`load_team_spec` 容错）；`models.py` 全文（校验器、`Limits`、`ObserverSpec`、运行时对象、序列化）。

**方法**：通读 → 对照方案与 DECISIONS → 对可疑点写最小复现探针（`review/tmp/probe_core.py`、`review/tmp/probe_c2.py`，输出 `review/tmp/probe-core-output.txt`、`review/tmp/probe-core-C.txt`）→ 跑确定性测试。所有结论均给出 `file:line` + 可重跑证据；推测项在 §五 标注「待验证」。

**关键证据一句话索引**

| 证据 | 内容 |
|---|---|
| probe A1–A5 | 仅权限变化的成员进入 DRAINING 后永不恢复，消息只入 pending、不再产生运行 |
| probe B1–B4 | `_reduce` 写后抛错：半应用写入已提交 + 失败回执落库（动作不可重试） |
| probe C-a1–C-a7 | 投递永不确认时，同一投递被反复注入并重跑，共创建 **1000** 个 run（全部 COMPLETED、全部 `input_delivery_ids==[1]`），仅因 `max_turns_per_goal` 耗尽而停止 |
| probe C-b1–C-b4 | 终态 run + 未确认投递（崩溃构造）后，下一次调度立即重排同一投递，成员将再次看到同一条 `user_message` |
| probe D1–D5 | `reconcile` 恢复出的 COMPLETED 运行不收敛：任务仍 RUNNING、成员仍 BUSY、无事件、完成请求未应用 |

**环境限制**：沙箱无网络（DNS 不可用）、无密钥；每次 shell 调用的 `/tmp` 不共享，故探针输出落盘到 `review/tmp/`。

---

## 二、结论

1. **单事务契约在主路径成立、在异常路径被破坏。** `Control.submit` 做到"去重先于校验"（control.py:78-81）、事件/投递/回执同事务（control.py:102-107），`_Tx` 可重入 + `BEGIN IMMEDIATE`（storage.py:944-972）。但 `_reduce` 抛异常时，**异常前已执行的写入会连同失败回执一起提交**，且动作永久不可重试（F-C2，P0）—— 注释里"transaction rolls back and nothing half-applies"与事实相反（control.py:94-95）。
2. **崩溃恢复存在一条未闭合的投递确认窗口。** 运行终态先落库、投递确认后落库、两者之间无事务保护（runtime.py:523/530），`reconcile` 只处理 RUNNING 运行（runtime.py:139-146），因此"终态运行 + 未确认投递"这一状态**没有任何路径会被修复**；`_schedule` 只看 pending 投递、不比对已消费批次（control.py:968-1022），于是同一投递被重新注入。实测在"确认始终失败"的构造下逐轮重跑，直到把该 goal 的轮次上限（1000）耗尽（F-C3，P1）。
3. **`runtime.reconcile` 会静默丢弃已完成的工作。** 它把 checkpoint 里的终态直接写进 `turn_runs` 就 `continue`（runtime.py:144-146），绕过 `_finalize`，于是任务卡在 RUNNING、成员卡在 BUSY、没有 `RUN_COMPLETED` 事件、完成请求不被应用，`signal_done` 永远报"有未完成任务"（F-C4，P1）。
4. **DRAINING 是单向状态。** 仅"权限变化"（通道/ACL/观察者）的受影响成员会被置为 DRAINING（control.py:633-659、803-811），而清除 DRAINING 只发生在成员 `AgentSpec` 变化时（control.py:831-841），`_schedule` 从此永久跳过该成员（control.py:964-967）：收不到消息、也不执行任务（F-C1，P1）。同 id 成员"移除后重加"因 `REMOVED` 不复位而同样被永久跳过（F-C7，P2）。
5. **`context_epoch` 机制只有读、没有写。** `bump_context_epoch`（storage.py:712-719）在 `src/` 中零调用者，而线程键正是 `session_id:agent_id:epoch`（runners.py:382-384、control.py:1020/1151-1153）：同一会话内"删掉再用同名（同 id）重建"的成员会继承被删成员的线程上下文，违背 T24（F-C5，P1）。
6. **校验完整性与死代码。** observer 的 `event_types`/`capabilities` 接受任意字符串（未知枚举不报错，违背方案 156 行），`capabilities` 无任何执行点（F-C6，P2）；`task_cancel_requested` 只写不读、`reconcile` 的 QUEUED 空循环、`_emit_limit_reached` 借 `PAUSE_SESSION` 为载体却不会暂停会话（F-C8，P3）。
7. **经检查未发现问题、值得保留的设计**：动作回执去重先于校验；投递批次号与 `applied_batch` 的检查点确认设计（runners.py:37-55/607）；依赖失败→BLOCKED 不静默卡死（control.py:1024-1045）；任务就绪一次性播报（control.py:1047-1066）；超限后停止派发且只报一次 `LIMIT_REACHED`（control.py:1006-1008/1093-1105，符合方案 224 行）；SQLite 侧 WAL + `busy_timeout=5000` + `foreign_keys=ON` + 手动事务 + 进程内串行化（storage.py:229-234/944-1002）。

---

## 三、发现清单

| ID | 严重度 | 标题 | 主要证据 |
|---|---|---|---|
| F-C1 | **P1** | 仅权限变化时成员卡在 DRAINING 且被永久跳过（成员失效、消息不再投递） | control.py:633-659/803-811/831-841/964-967；probe A1–A5 |
| F-C2 | **P0** | `_reduce` 异常路径提交半应用写入 + 失败回执（回执与真实状态不一致、动作不可重试） | control.py:78-101；probe B1–B4 |
| F-C3 | **P1** | 运行终态先于投递确认落库：崩溃后同一投递重复注入（最坏逐轮重跑至耗尽轮次上限） | runtime.py:523/530/559/139-146；runners.py:253-264/607；control.py:968-1022；probe C-a/C-b |
| F-C4 | **P1** | `reconcile` 绕过 `_finalize` 直接写终态：重启后已完成的工作被静默丢弃 | runtime.py:144-146 对比 446-531；probe D1–D5 |
| F-C5 | **P1** | 同 id 成员重加不换 `context_epoch`：继承被删成员的线程上下文（违背 T24） | storage.py:712-719（无调用者）/689-694；control.py:1020/1151-1153；runners.py:382-384 |
| F-C6 | P2 | observer 配置校验缺口：`event_types`/`capabilities` 任意字符串，未知枚举不报错 | models.py:203-211/256-289；方案 156 行 |
| F-C7 | P2 | 同 id 成员重加不重置 `REMOVED`：与 F-C1 同样的调度永久跳过（代码路径推断） | control.py:708-722/831-844/964-967 |
| F-C8 | P3 | 死字段/死代码：`task_cancel_requested` 只写不读、`reconcile` QUEUED 空循环、`PAUSE_SESSION` 载体 | storage.py:765-769；runtime.py:157-158；control.py:1093-1105 |

### F-C1（P1）仅权限变化时成员卡在 DRAINING 且被永久跳过

**证据**

- control.py:633-659：拓扑补丁的受影响成员若"仍有在途运行"（`_agent_has_live_run`）则 `set_agent_status(..., DRAINING)` 并把补丁置为 `WAITING_BOUNDARY`。
- control.py:803-811：`_affected_agents` 把"`AgentSpec` 没变但权限变了"的成员也计入（`_permissions_changed`，control.py:814-829 比较 sends/delegates/spaces/observers）。
- control.py:831-841：`_sync_members` **只在 `old_a is None or old_a != a` 时**才 bump config revision 并 DRAINING→IDLE。
- control.py:1068-1080：边界应用（`_apply_boundary_patches`）只调用 `_sync_members(new_spec, current)` —— 对"仅权限变化"的成员仍然不满足上面的条件。
- control.py:964-967：`_schedule` 对 `REMOVED`/`DRAINING` 成员直接 `continue`。
- probe A（`review/tmp/probe-core-output.txt`）：
  - A1 `status=WAITING_BOUNDARY`，`affected_agents=['c']`；A2 `c` 变为 `DRAINING`；
  - A3 边界应用后 `revision=2`，但 `c` 仍 `DRAINING`、`config_revision=1`（未 bump）；
  - A4 给 `c` 发消息：回执 ok、`pending deliveries=1`，但 `new runs for c = 0`，`c` 仍 DRAINING。

**影响**：成员进入"永久冻结"——此后发给它的消息只堆积在 `deliveries(pending)`，永远不会产生运行；没有任何错误、事件或告警暴露这一状态（成员在 `relevant_topology` 里仍以 DRAINING 出现）。会话中只要有一次涉及该成员的权限类补丁且它恰好在途，该成员即报废，只能靠再改它的 `AgentSpec` 触发 bump 才可能恢复。

**建议**：把"解除 DRAINING"绑定到补丁的 `affected_agents` 而不是"AgentSpec 是否变化"——在 `_apply_boundary_patches`/`_apply_patch_action` 应用成功后，对 `patch.affected_agents` 中所有 `DRAINING` 成员执行 DRAINING→IDLE（并 bump config revision）；同时补一条回归测试："权限补丁 + 受影响成员在途 → 边界应用后成员可再次收到消息并产生运行"。

### F-C2（P0）`_reduce` 异常路径提交半应用写入 + 失败回执

**证据**

- control.py:78 进入事务 → 83-92 校验并调用 `_reduce`（`_reduce` 内部含 `add_shared_entry`、`insert_task`、`insert_patch`、`create_delivery`、`save_team_spec` 等写操作）→ 93-101 捕获任何异常后**在同一个事务内**写失败回执并 `return`。
- control.py:94-95 的注释声称 "the transaction rolls back and nothing half-applies"——只有 `raise` 才会回滚，而这里是 `return`；`with` 正常退出即提交。
- control.py:79-81：动作去重先于一切，失败回执落库后同一 `action_id` 重试只会拿回该失败回执（不可重试）。
- probe B（`review/tmp/probe-core-output.txt`）：B1 回执 `ok=False`、`"RuntimeError: simulated failure after write"`；B2 共享空间 `main` 中已提交条目 `['partial-write']`；B3 失败回执已持久化 → 无法重试。

**影响**：数据状态与回执/审计不一致：调用方（模型、TUI、Leader）看到"动作失败"，但部分业务变更已经生效且不可重试，只能人工修状态。这直接破坏 T8/§9.2 的原子性与可恢复性前提（"动作提交后回执丢失仍只产生一次团队变更"）。

**建议**：异常分支改为 `raise`（让事务回滚），或捕获后显式 rollback 到保存点；如果确实需要"把畸形输入变成可读拒绝"，就把 `_reduce` 中的写收敛到 `_persist_events` 之后，先做纯校验（无副作用的 dry-run）再落库。

**可达性说明（见 §五.1）**：本次以注入异常证明后果；真实触发需要 `_reduce` 在完成某些写之后抛错（例如某个 malformed op 通过了 `_validate` 却在后续步骤触发 `KeyError/TypeError`）。按"承诺的原子性在异常路径不成立"这一事实判 P0。

### F-C3（P1）运行终态先于投递确认落库：崩溃后重复注入

**证据**

- runtime.py:523 `set_run_status(run.run_id, outcome.status)`、524-529 成员状态、530 `ack_run_deliveries(run)`：三条独立语句（`set_run_status`/`set_agent_status` 为自带事务的写，`ack_run_deliveries` 自开事务），**不在一个事务里**。
- runtime.py:559：`_finalize` 结尾调用 `self.control.schedule()` —— 每结束一轮就重排一次。
- runtime.py:139-146：`reconcile` 只遍历 `RUNNING` 运行；终态运行的未确认投递不会被补确认。`runners.DeepAgentsRunner.reconcile`（runners.py:592-620）里基于 checkpoint `applied_batch` 的确认（runners.py:607）同样只在 RUNNING 运行时可达。
- runners.py:253-264：`TeamAgentMiddleware.abefore_model` 把 drain 出来的 inbox 项直接注入模型输入，**不检查 `state["applied_batch"]`**（`applied_batch` 只用于 runner 自己的确认路径）。
- control.py:968-1022：`_schedule` 只依据 `pending_deliveries` 创建/续接运行，不比对"该投递批次是否已被某次运行消费"。
- probe C（`review/tmp/probe-core-C.txt`）：
  - C-b1–C-b4：构造"终态运行 + 未确认投递"（即崩溃瞬间）后，下一次调度直接新建运行 `input_delivery_ids=[1]`，且该运行将注入的内容就是原来那条 `user_message 'hello'`；
  - C-a1–C-a7：让确认始终失败（模拟崩溃/确认链故障）后，同一投递被反复注入并重跑：`total runs created: 1000`、`status histogram: {'COMPLETED': 1000}`、`runs whose input_delivery_ids == [1]: 1000`、投递(1) 仍 `pending`、`settle()` 直到轮次上限把派发卡死才返回 True（无遗留 QUEUED/RUNNING）。

**影响**：至少一次投递的"重试"退化为"重复注入"，违反方案 247 行与 T21。真实崩溃后通常重复注入一次（一次多余的模型调用 + 该轮副作用被重复执行，可能重复发消息/重复执行工具）；若确认链持续失败或进程反复崩溃，则每轮重排、直到耗尽该 goal 的 `max_turns_per_goal`（实测 1000 轮）。

**建议**：① 把 `set_run_status` + `set_agent_status` + `ack_run_deliveries` 合并进一个事务（或先写"确认意图"再提交终态）；② `_schedule` 对"`batch_no` ≤ 该成员已确认批次"的投递不再生成新运行，作为独立于确认时序的兜底；③ `reconcile` 对终态运行用 checkpoint 的 `applied_batch` 补做确认；④ `abefore_model` 注入时跳过 `applied_batch` 覆盖的项。

### F-C4（P1）`reconcile` 绕过 `_finalize` 直接写终态：重启后已完成的工作被静默丢弃

**证据**

- runtime.py:144-146：checkpoint 有终态时执行 `self.store.set_run_status(run.run_id, state)` 然后 `continue`；对比正常路径 runtime.py:446-531（`_finalize`：应用完成请求、写 `RUN_*` 事件、设成员状态、ack 投递、清理步数、重排）。
- probe D（`review/tmp/probe-core-output.txt`）：D1 `run=COMPLETED` 而 `task=RUNNING`、`agent=BUSY`；D2 `events=[]`；D3 完成请求仍未应用；D4 `signal_done` 的 blockers 仍为 `['unfinished tasks: t1:RUNNING']`。

**影响**：重启后发现"其实已经完成"的成员回合被丢弃：任务永远 RUNNING、成员永远 BUSY、Leader 看不到完成事件、完成请求（`result_refs/summary`）丢失，`signal_done` 永远被阻塞，只能人工取消/重派——正是 §9.2 要求"恢复后不丢结果"的反例。

**建议**：`reconcile` 发现终态时走与 `_finalize` 相同的收敛路径（最小集：应用完成请求 → 写 `RUN_COMPLETED/RUN_FAILED` 事件 → 成员状态落 IDLE/WAITING → ack 投递 → `schedule()`），或把 `_finalize` 抽成"状态无关的收敛函数"由两处共用。

### F-C5（P1）同 id 成员重加不换 `context_epoch`：继承被删成员的线程上下文

**证据**

- storage.py:712-719：`bump_context_epoch` 存在；`grep -rn "bump_context_epoch" src tests` 在 `src/` 中只命中定义本身（其余命中是 `.pyc`），**无任何调用者**。
- storage.py:689-694 读取 `context_epoch`；control.py:1020 在建运行时就地取 `context_ref=self._context_ref(agent.id)`；control.py:1151-1153 `_context_ref = f"{session_id}:{agent_id}:{epoch}"`。
- runners.py:382-384：`_thread_id = f"{self.session_id}:{self.agent.id}:{epoch}"`（epoch 从 `run.context_ref` 里解析）。
- control.py:708-722：`remove_agent` 把成员移出 TeamSpec；`add_agent` 只拒绝"当前已存在的 id"，因此同一会话内"先删后加同 id"是合法序列。

**影响**：`context_epoch` 恒为 1，线程键恒定 → 重建的同 id 成员复用被删成员的 DeepAgents 线程（对话历史、工具痕迹、私有子代理状态），违背 T24「同名新成员不继承旧身份」与方案 134 行"线程键由 `session_id + agent_id + context_epoch` 确定"。

**建议**：在"移除成员"或"检测到同 id 重新加入且该 id 在会话历史中存在"时调用 `bump_context_epoch`（例如在 `_sync_members` 里对 `old_a is not None and old_a.removed_at`/`AgentStatus.REMOVED` 的成员处理，或直接在 `_apply_operations` 处理 `remove_agent`/`add_agent` 时标记），并补回归测试："删除→以同 id 重加后 `run.context_ref` 与删除前不同"。

### F-C6（P2）observer 配置校验缺口

**证据**：models.py:203-211（`event_types: list[str]`、`capabilities: list[str]` 均为任意字符串，`payload_scope`/`wake_policy` 才用 `Literal`）；models.py:256-289（`_validate_refs` 校验 leader/重复 id/成员上限/通道与观察者成员引用/空间 ACL，**不校验 `event_types` 是否为已知 `EventKind`、`capabilities` 是否为已知能力**）；对照方案 156 行"未知字段、未知引用和不支持的枚举值均返回可理解的错误"；`grep -rn capabilities src/teamagents/*.py` 显示除 `models.py:211` 的观察者字段外只有 `models.py:501` 与 `views.py:202`（`AgentView.capabilities = sorted(set(tool_bindings))`），即观察者的 `capabilities` 既无校验也无执行点。

**影响**：拼错的 `event_types`（例如 `"task_complete"`）让观察者**静默永不匹配**（`views.event_push` 的 `observer_matches` 只做集合判断），排障困难；`capabilities` 是死字段，使用者会误以为它能限制观察者能力上限。

**建议**：用 `EventKind` 与已知能力集合做枚举/`Literal` 校验（或在 `_validate_refs` 中显式报错）；`capabilities` 要么实现强制点，要么从 schema 移除。

### F-C7（P2）同 id 成员重加不重置 `REMOVED`（代码路径推断）

**证据**：control.py:842-844（`_sync_members` 对被移除的旧成员置 `REMOVED`）；control.py:833-841（同函数对"新增/变更"成员只 `ensure_agent` + 条件 bump，从不把 `REMOVED` 复位为 IDLE）；control.py:964-967（`_schedule` 跳过 `REMOVED`）。`update_agent` 也不触碰状态。

**影响**：与 F-C1 同类但触发路径不同：重加的同 id 成员虽然回到 TeamSpec（`_validate_refs` 不阻止），调度却永远跳过它，给它的消息只堆积 pending。

**说明**：本条为代码路径推断（未单独写探针，理由见 §五.4）；F-C1 的探针已证明"DRAINING → 永久跳过"的同一调度分支。

**建议**：`_sync_members` 在 `ensure_agent` 后，若当前状态为 `REMOVED` 则复位为 `IDLE` 并 bump config revision（新身份）；同时把 F-C1 的 affected 清理逻辑一并处理。

### F-C8（P3）死字段 / 死代码（清理项，不影响当前语义）

- `task_cancel_requested`：只写不读。control.py:883 写入；storage.py:765-769 的 getter 在 `src/` 中无调用者；取消语义实际由 **run 级** flag 承担（`run_cancel_requested` 在 runtime.py:319/395 读取）。若将来有代码只看任务级 flag，会漏掉取消。
- runtime.py:157-158：`for run in self.store.runs_for_session(..., [QUEUED]): _ = run` 是空循环（死代码），提示 reconcile 对 QUEUED 运行并未做任何处理。
- control.py:1093-1105：`_emit_limit_reached` 为了调用 `_persist_events` 而构造 `ActionKind.PAUSE_SESSION` 的 TeamAction，但该动作不经过 `_reduce`（control.py:552-553 才是真正置 PAUSED 的分支），因此**不会暂停会话**；同理 control.py:1042/1063 用 `ActionKind.CANCEL_TASK` 作为 blocked/ready 事件的载体。与方案 224 行（达到上限时"停止派发新工作、保存状态、报告 LIMIT_REACHED"）不冲突——派发确实被 `_turn_budget_ok` 卡住（control.py:1006-1008），但借用的 `kind` 容易让读者误判语义，建议改用中性的"系统事件"载体。

---

## 四、与方案/文档的偏差

1. **F-C5 ↔ 方案 134 行 / §5.1 / T24**：方案要求 `context_epoch` 参与线程键并可变更（"删除后用同名创建的成员使用新 ID，不能自动继承被删除成员的上下文"）；实现只读不写，身份复用真实存在。
2. **F-C3 ↔ 方案 247 行 / T21**："投递采用至少一次传输，配合稳定消息 ID 和检查点确认避免上下文重复注入"；实现中确认与终态不同事务、且终态运行不可补确认，崩溃后退化为重复注入。
3. **F-C1/F-C7 ↔ 方案 §8（T11–T13）**：补丁"到达边界后应用并恢复成员"的意图只对 `AgentSpec` 变化的成员成立；权限型受影响成员没有恢复路径（进入 DRAINING 即永久冻结）。
4. **F-C6 ↔ 方案 156 行**：要求"未知字段、未知引用和不支持的枚举值均返回可理解的错误"；observer 的 `event_types`/`capabilities` 未做枚举校验。
5. **已确认的偏离（非缺陷，记录在案）**：D-1（`Control.submit` 单入口管线）、D-10（limits 重做）。审查期间 `storage.py`/`models.py`/`runners.py`/`execution.py` 在 11:11 的外部改动即 D-10 落地；`load_team_spec` 对旧 session 的已删 limits 键做丢弃式容忍属正向兼容处理。
6. **未发现与方案冲突的 SQLite 配置**：WAL、`busy_timeout=5000`、`foreign_keys=ON`、`isolation_level=None` 手动事务、进程内 `_LockedConn` 串行化、可重入 `BEGIN IMMEDIATE`（storage.py:229-234/944-1002）与 §9.2 的"显式事务 + 唯一约束 + 检查预期旧状态"方向一致；动作/事件/投递的唯一约束与 `update_run_status_where` 的条件更新在本次审查中未发现绕过。

---

## 五、未验证 / 存疑项

1. **F-C2 的"真实可达异常路径"未穷举**（探针用注入异常）。P0 判定基于"代码承诺的原子性在异常路径不成立 + 后果已证"；若维护方认为 `_reduce` 不可能在写后抛错，可降为 P1 并补一条"写后抛错"的防御性测试。
2. **F-C3 的 1000 轮重跑是"确认每次都失败"的构造**；真实 `kill -9` 崩溃（进程终止后重启）预期只重复注入一次。构造证明了机制与上限（受 `max_turns_per_goal` 约束），未统计真实崩溃落在该窗口的概率。
3. **probe C-a6 内部不一致**：1000 个 run 但事件里 `run_started=500`/`run_completed=499`。未定位原因（可能与事件 emit 的批次/去重路径或我被 patch 的 ack 有关），不影响 C-a2/C-a4 的结论；如需可另开一条跟踪。
4. **F-C5/F-C7 未跑端到端探针**：F-C5 需要真实 DeepAgents checkpointer（依赖与磁盘状态）才能展示"继承历史"；F-C7 需要构造"移除→重加"脚本。两者均以代码路径 + grep（`bump_context_epoch` 零调用者）为证。
5. **未覆盖区域**：approval 唤醒路径（`_wake_approval_run`）、codex runner 的 reconcile 语义、跨进程并发写（会话文件锁与多进程场景）、`OUTCOME_UNKNOWN` 之后的收敛、`mid_turn_pushes` 在崩溃时的丢失、长事务对 WAL 的影响（本次未见长事务，`submit` 内无 I/O 等待）。
6. **未做真实崩溃注入**：以直接构造 store 状态（终态 run + pending 投递）替代 `kill -9` 时序注入；建议后续用子进程 + 定时 kill 做一次端到端验证。

---

## 六、自检

**只读性（sha256，审查结束时）**

```
d357d9bf44d37a7d3a0be9a91583c3cee0302d56efc6d64bff9c2e7a2a41edf5  src/teamagents/control.py
03e6543698ab3e1301919bf9993b32b77eadca0498575d3773dd688581754f0b  src/teamagents/storage.py
39ca9d95620f0c8865d700c3c4094bf651d98c7496471f94fa5da569f93cc9df  src/teamagents/models.py
7b4c2486e94e087b0baf370a9735202cd9f81f12cf8208689c37c59e9f667f99  src/teamagents/runtime.py
0f910c914d240e747e2586c4283447c5955f87854414ff9c98c13c688c5070c7  src/teamagents/runners.py
a7ef668bb70e36db46f5c08630efbdda55434aca74f75d6df9575e68eeb13b21  src/teamagents/views.py
```

- `control.py` mtime `01:01:22`（审查期间未变）；`storage.py`/`models.py` mtime `11:11`（外部修改，见抬头说明）。
- 本次审查未修改任何源文件/测试/文档；写入仅限 `review/findings-core.md`、`review/tmp/*`。运行 Python 会在 `src/teamagents/__pycache__/` 生成字节码（源文件内容未变，上表可证）。

**确定性测试（无网络、无密钥、`-p no:cacheprovider`）**

```bash
# 域内
.venv/bin/python -m pytest tests/test_p1_guards.py tests/test_p2_recovery.py tests/test_p4_topology.py -q
# -> 16 passed

# 全量（沙箱默认无网络/无密钥）
.venv/bin/python -m pytest tests/ -q
# -> 2 failed, 76 passed, 12 deselected
#    两个失败均为已知环境性失败：
#    - test_config_cli.py::test_xhigh_maps_to_max_for_models_without_xhigh（缺 DEEPSEEK_API_KEY）
#    - test_p3_web_tools.py::test_ssrf_guard_blocks_internal_targets（DNS 不可用）
```

**结论**：现有验收测试全绿的同时，F-C1/F-C3/F-C4/F-C5 依然存在 —— 说明"崩溃窗口 / 边界恢复 / 身份复用"三条路径缺少回归覆盖；建议为每条发现补一个确定性测试（F-C1/F-C7 可用 `FakeMember` 脚本；F-C3/F-C4 可用"构造终态 run + 未确认投递"和"checkpoint 终态"的假 runner，本报告的两个探针可直接改造成测试）。

**复现命令**

```bash
.venv/bin/python review/tmp/probe_c2.py            # A/B/D + C（紧凑输出，落盘 review/tmp/probe-core-C.txt）
.venv/bin/python review/tmp/probe_core.py          # 原始探针（输出 review/tmp/probe-core-output.txt）
```
