# 运行时、执行与权限安全域 · 审查报告（review/findings-runtime.md）

审查员：reviewer_runtime ｜ 基准：`TeamAgents-Implementation-Plan.zh-CN.md` §2.2/§4/§6.4/§7/§9.3/§10.1/§11/§12/§16-17 + `docs/DECISIONS.md`
协议：`review/PROTOCOL.md`（只读；仅在 `review/**` 写入）

## 一、范围与方法

精读的源码（全量）：

| 文件 | 说明 |
|---|---|
| `src/teamagents/runtime.py`（612 行） | 会话运行时、调度、回合终结、恢复 |
| `src/teamagents/execution.py`（242 行） | bwrap 隔离、受保护文件 backend |
| `src/teamagents/agents.py`（281 行） | `ToolGateway`、`TurnOutcome`、`FakeMember` |
| `src/teamagents/permissions.py`（155 行） | 权限策略与批准闸门 |
| `src/teamagents/tools.py`（220 行） | web 搜索/抓取、SSRF 防护、MCP 绑定 |
| `src/teamagents/providers.py`（86 行） | 模型 profile → 模型实例、effort 映射 |

为判定"唯一入口"与调度语义，另读了上下游：`runners.py`（DeepAgentsRunner + middleware）、`control.py`（`_validate`/`_reduce`/`_schedule`/`_completion_blockers`）、`storage.py`（approvals/runs/deliveries/completion_requests/quoted DDL）、`session.py`、`config.py`、`tui/app.py`（批准与全自动开关的调用点），以及第三方 `deepagents 0.7.13` 的 `backends/composite.py`、`backends/filesystem.py`、`middleware/filesystem.py`、`middleware/subagents.py`、`graph.py`（判定内置文件工具与私有子代理是否经过本仓网关）。

测试（只读理解语义，不评审测试本身）：`tests/test_p1_guards.py`、`test_p3_deepagents_runner.py`、`test_p3_tools_and_session.py`、`test_p3_web_tools.py`、`conftest.py`、`scripted_model.py`；另核对 `docs/P0-findings.md` 的隔离探针结论。

新增最小复现脚本（`review/tmp/`，均写入临时目录，不改仓库源码）：

- `review/tmp/exp_escape.py` — 文件工具经 `/memory/`、`/skills/` 路由逃出授权工作目录
- `review/tmp/exp_subagent.py` — 私有子代理绕过权限网关
- `review/tmp/exp_mode_and_delivery.py` — 运行期权限模式不生效；投递被 ack 但未注入

## 二、结论

1. **权限网关不是唯一入口**：Deep Agents 自动添加的 `general-purpose` 私有子代理不装配 `TeamAgentMiddleware`，其 `shell`/MCP 调用完全不过批准闸门（RT-02，已复现）；文件工具还能经 `/memory/`、`/skills/` 路由写出授权工作目录（RT-01，已复现），配合默认加载的 `~/.config/teamagents/AGENTS.md` 可反写用户配置把 `permissions.mode` 改成 `full_auto`——T16「全自动只能由用户开启」被绕开。
2. **T16 在运行期整体失效**：`set_permission_mode` 只写数据库与事件，从不更新活着的 `ApprovalGate`（RT-07，已复现）；`ApprovalGate.set_mode` 在生产代码里没有任何调用点，`PermissionPolicy.write_paths` 是死字段。
3. **取消语义谎报**：`DeepAgentsRunner.request_interrupt` 只改内存状态、不中断 graph/任务，运行时却据此把回合标成 `CANCELLED`（RT-03）；被取消宣告的回合实际继续执行并产生副作用，违反 §9.3「等待执行停止的确认」。
4. **恢复窗口**：重启后 `WAITING_APPROVAL` 的回合失去 `_paused_kind`，恢复时用新消息而不是 `Command(resume=...)`（RT-04）；被取消的 RUN 不会清掉 PENDING 批准，`_schedule` 又对 `WAITING_APPROVAL` 的 `cancel_requested` 直接 `continue`，导致 `signal_done` 永久被 blocker 卡住（RT-06）。
5. **投递存在静默丢失**：成员运行中到达的消息在回合终结时一并 ack，但若该回合没有再发生模型调用，内容从未注入且不再重投（RT-05，已复现），正违反 §7「不能在仅放入内存队列后就推进消费位置」。

## 三、发现清单

| ID | 严重度 | 标题 |
|---|---|---|
| RT-01 | P0 | 文件工具经 `/memory/`、`/skills/` 路由逃出授权工作目录，可反写用户配置开启 full_auto |
| RT-02 | P0 | 默认 `general-purpose` 私有子代理绕过 `TeamAgentMiddleware`（批准与步数上限） |
| RT-03 | P1 | `cancel` 不停 DeepAgents 回合，却把回合标成 `CANCELLED` |
| RT-04 | P1 | 重启后 `WAITING_APPROVAL` 回合无法按批准语义恢复（`_paused_kind` 仅内存） |
| RT-05 | P1 | 回合终结时 ack 尚未注入的投递 → 消息静默丢失 |
| RT-06 | P1 | 取消/终结不回滚 PENDING 批准，且 `cancel` 对 `WAITING_APPROVAL` 静默无效 → `signal_done` 死锁 |
| RT-07 | P1 | 运行期权限模式不生效（`SET_PERMISSION_MODE` 不同步闸门，T16） |
| RT-08 | P1 | `web_fetch` 跟随重定向不重新校验 → SSRF 防护可绕过 |
| RT-09 | P1 | `shell` 与文件工具对所有成员无条件注册，无视 `tool_bindings` |
| RT-10 | P2 | 预授权范围不可配置：`write_paths` 死字段、`pre_authorized` 硬编码 |
| RT-11 | P2 | 输出体积无真正上限：`run_isolated` 全量缓冲、`web_fetch` 全量读入内存 |
| RT-12 | P2 | 一个回合内多次 `complete_task` 仅最后一个生效（upsert 覆盖） |
| RT-13 | P2 | `mcp_`/`web_` 前缀无条件放行（旁路批准的名字启发式） |
| RT-14 | P3 | `_loop` 每轮泄漏 `wake.wait()` 任务；`close()` 不回收取消任务 |
| RT-15 | P3 | `_switch_effort_to_max` 就地改写共享 catalog；effort 映射结果被丢弃 |
| RT-16 | P3 | 死代码与命名瑕疵：`GuardedFilesystemBackend` 未使用、`_save_artifact` 秒级命名冲突等 |

---

### RT-01 · P0 · 文件工具经 `/memory/`、`/skills/` 路由逃出授权工作目录，可反写用户配置开启 full_auto

**证据**

- `src/teamagents/runners.py:386-402`：`_build_backend()` 把成员 workdir 作为默认路由（`IsolatedShellBackend(self.workdir, ...)`），但额外挂了三个**宿主真实目录**路由，且都用**未加保护的** `FilesystemBackend`：
  ```python
  routes = {"/artifacts/": FilesystemBackend(root_dir=self.artifacts_dir, virtual_mode=True)}
  for i, skills_dir in enumerate(self.skills_dirs):
      if skills_dir.is_dir():
          routes[f"/skills/{i}/"] = FilesystemBackend(root_dir=skills_dir, virtual_mode=True)
  for i, memory_file in enumerate(self.memory_files):
      if memory_file.is_file():
          routes[f"/memory/{i}/"] = FilesystemBackend(root_dir=memory_file.parent, virtual_mode=True)
  ```
- `src/teamagents/session.py:88-96`：`memory_files` 默认包含 `cwd/AGENTS.md` 与 `~/.config/teamagents/AGENTS.md`，`skills_dirs` 包含 `cwd/.teamagents/skills` 与用户级 skills 路径 → 这些**父目录**成为可写根。
- `src/teamagents/execution.py:167-198`：本仓写了 `GuardedFilesystemBackend`（符号链接/穿越防护），**全仓无任何引用**（`grep -rn GuardedFilesystemBackend src/` 仅命中定义处）——真正生效的是 deepagents 的 `FilesystemBackend`。
- 权限侧不设防：`src/teamagents/permissions.py:74-80` 的 `_bound_tool()` 把 `write_file`/`edit_file`/`delete` 等文件工具列入白名单，`Decision(allow=True)`，因此这些越界写在**预授权范围内**被执行，不产生批准请求。

**复现**（`review/tmp/exp_escape.py`，写入临时目录）：

```
tool names exposed to the model: [..., 'write_file', ...]
outside-workdir config.toml written: True "mode = 'full_auto'\n"
skill file overwritten: True
approvals raised: []
```

即：成员用 `write_file("/memory/0/config.toml", "mode = 'full_auto'")` 直接改写了 AGENTS.md **同级目录**里的真实文件；用 `write_file("/skills/0/reporting/SKILL.md", ...)` 覆写了用户的 skill 文件。

由此形成一条把「全自动只能由用户开启」变成模型可持久化开关的链路：`session.py:93-95` 在 `~/.config/teamagents/AGENTS.md` 存在时把它加入 `memory_files`，于是 memory 路由的根就是 **`~/.config/teamagents/`**（该条件在用户按 README/`examples/config.toml` 配置用户级指令文件时成立），成员即可写其中的 `config.toml`：`permissions.mode = "full_auto"` → 下次启动 `permission_mode_from_config()`（`config.py:85-91`，只读用户配置文件）返回 full_auto。注意这条链不需要 `cwd/AGENTS.md`（项目目录通常在 workdir 内，属预授权范围），只有**用户级**路径真正越界。同理，配置里的用户级 skills 路径也会成为可写根。

**影响**：T23（路径穿越/越权写）与 T16（全自动只能用户开启）同时被绕；成员可改写影响自身后续行为的指令文件（AGENTS.md/SKILL.md），形成持久化的提示注入通道；`/artifacts/` 路由（会话制品目录）同样可被任意成员写，而 §9.1 只把它当"引用目标"。

**建议**：memory/skills 路由改为只读 backend（写操作返回权限错误），或至少接入 `GuardedFilesystemBackend`/显式只读挂载；`/artifacts/` 写入应经批准或限定在会话目录内；同时把文件工具的路径判断纳入 `permissions.py`（而不是仅靠 backend 的 virtual_mode）。

---

### RT-02 · P0 · 默认 `general-purpose` 私有子代理绕过 `TeamAgentMiddleware`（批准与步数上限）

**证据**

- `src/teamagents/runners.py:442-452`：`create_deep_agent(...)` 调用**没有**传 `general_purpose_subagent=GeneralPurposeSubagentProfile(enabled=False)` 之类的开关，Deep Agents 因此自动注入默认私有子代理（`deepagents/graph.py:790-857`）。
- `deepagents/graph.py:834-838`：该子代理的 `tools` 是**调用方传入的同一份 tools 列表**——即 `runners.py:437-438` 的 `[*build_team_tools(self), self._shell_tool(), *bound_tools, *extra_tools]`，包含 `shell` 与全部 MCP/web 工具。
- 同一处 `gp_middleware = [FilesystemMiddleware(...), create_summarization_middleware(...), PatchToolCallsMiddleware(), (+Skills)]`——**不含 `TeamAgentMiddleware`**。而权限判定与步数上限都在 middleware 里：`runners.py:273-329`（`awrap_tool_call` 调 `self.approvals.check`）、`runners.py:266-271`（`awrap_model_call` 计 `model_steps`）。
- 因此：父图里 `shell(network=True)` 会命中 `permissions.py:55-59` 的"需批准"，而在子代理里完全不过闸门；`config.py`/`session.py` 也没有任何地方关闭该子代理。

**复现**（`review/tmp/exp_subagent.py`）：模型先调 `task(subagent_type="general-purpose", ...)`，用户批准该 `task` 调用（批准 scope 确实只写着 `{"tool": "task", "args": {...}}`），子代理随后调用 `shell(command=..., network=True)`：

```
approval scope: {"tool": "task", "args": {"subagent_type": "general-purpose", "description": "report the network interfaces"}, "reason": "task is not pre-authorized"}
approvals raised: []          # 子代理的 shell(network=True) 没有产生任何批准
approval rows in DB: 1        # 只有 task 那一条
run status: [COMPLETED]
shell -> {"ok": true, "exit_code": 0, ...}
```

对照组（父图直接调 `shell(network=True)`）在同一套代码里会 park 成 `WAITING_APPROVAL`（`tests/test_p3_deepagents_runner.py:113-151` 断言了该行为）。注意 `shell` 的 `network` 形参是模型可控的（`runners.py:408-409`），且 `run_isolated(network=True)` 会**去掉** `--unshare-net`（`execution.py:90-91`）。

**影响**：T15/T23 的批准边界被绕；用户看到的批准卡只描述 `task`，实际放行的是子代理内**任意** shell/MCP 行为（含联网、读宿主可挂载路径）；D-2 声称的"统一网关是唯一入口"不成立；`max_model_steps_per_turn` 对子代理内部循环同样不计（`_guarded_executor` 不是这条路径的计数点，见 `runtime.py:407-418` 只服务 ToolGateway 路径），§6.4「个体私有子任务也必须计入所属成员的资源使用」未满足。

**建议**：显式关闭默认私有子代理（`general_purpose_subagent=GeneralPurposeSubagentProfile(enabled=False)`），或在子代理 middleware 中装配同一个 `TeamAgentMiddleware`/`awrap_tool_call`；若确要保留子代理，则将其工具限制为无副作用的只读白名单，并把步数计数放到共享的 runner 计数器上。

---

### RT-03 · P1 · `cancel` 不停 DeepAgents 回合，却把回合标成 `CANCELLED`

**证据**

- `src/teamagents/runners.py:575-579`：
  ```python
  async def request_interrupt(self, run_id: str) -> TurnStatus:
      self._states[run_id] = TurnStatus.CANCELLED
      return TurnStatus.CANCELLED
  ```
  没有任何 `graph.ainterrupt`/任务取消/`asyncio` 取消，也没有向 graph 发信号；正在执行的 `_stream_graph` 完全不知情。
- `src/teamagents/runtime.py:395-405`：回合正常返回后，运行时**用这个返回值**覆盖结果：
  ```python
  confirmed = await asyncio.wait_for(runner.request_interrupt(run.run_id), timeout=...)
  outcome = TurnOutcome(status=confirmed, note="cancelled by request")
  ```
  `FakeMember` 里的实现同样只是"置一个标志、让脚本下次检查"（`agents.py:267-270`）。
- 更早的路径 `runtime.py:314-339`（`_watch_cancellations`/`_request_stop`）也把"立刻返回 CANCELLED"当成"已确认停止"。

**影响**：TUI/用户发出取消后，界面显示 `CANCELLED`（还有 `run_cancelled` 事件），但回合在后台继续跑完模型与工具（写文件、发消息、联网），取消并没有收敛副作用；若进程在此后重启，`reconcile()` 还会用 `query_state()` 的 `CANCELLED` 覆盖数据库（`runtime.py:137-146`），把"仍在运行"记成"已取消"。§9.3 要求「取消可立即提出请求，但要等待执行停止的确认」，本实现既没有停止手段，也没有真实确认。

**建议**：DeepAgentsRunner 的 `request_interrupt` 应真正中断当前 graph 段（例如取消承载 `astream` 的 task、或在模型边界通过 middleware 抛出中断），并在无法确认停止时返回 `OUTCOME_UNKNOWN`（`runtime.py:399-403` 已有该分支，只是永远走不到）。

---

### RT-04 · P1 · 重启后 `WAITING_APPROVAL` 回合无法按批准语义恢复（`_paused_kind` 仅内存）

**证据**

- `src/teamagents/runners.py:359`：`self._paused_kind: dict[str, str] = {}`——纯内存。
- `src/teamagents/runtime.py:137-159`：`reconcile()` 只遍历 `TurnStatus.RUNNING`：`for run in self.store.runs_for_session(self.session_id, [TurnStatus.RUNNING])`。`WAITING_APPROVAL`/`WAITING_TASK` 的回合不在其中。
- `src/teamagents/runners.py:481-496`：恢复分支依赖 `self._paused_kind.get(run.run_id)`；拿不到就落进 `else`：
  ```python
  text = render_view(view, wake, str(self.workdir))
  graph_input = {"messages": [HumanMessage(content=text)]}
  ```
  而批准后的正确恢复是 `Command(resume={"allow": ..., "decisions": ...})`（481-487 行）。
- 同一函数 `reconcile()`（`runners.py:598-615`）只在 `_graph is not None` 时才读 checkpoint 恢复 `_paused_kind`，而重启后的第一次 `start_or_resume` 之前 `_graph` 必然是 `None`（`_ensure_graph` 懒构建，`runners.py:427-433`）。

**影响**：跨进程恢复（T8）时，用户对一个重启前就存在的 pending approval 做"批准"：回合被唤醒成 `RUNNING`，但 LangGraph 线程收到的是**一条新 HumanMessage**而不是 resume 值——被中断的工具调用不会按批准执行，回合语义与用户意图脱节（对照 `runtime.py:173-182` 只把决定转发给"持有活回合"的外部 backend，DeepAgents 路径依赖 `_paused_kind`）。**待验证**：需要带持久 checkpointer 的跨进程复现脚本（本轮未构造，判断基于上述代码路径）。

**建议**：把 `paused_kind` 落库（`turn_runs` 加一列，或在 `reconcile()` 中通过 `graph.aget_state()` 对 `WAITING_APPROVAL`/`WAITING_TASK` 的 run 一并探测 `state.next` 与 interrupt 值——`runners.py:607-615` 已有这段逻辑，只是没有被 `WAITING_*` 的 run 触发）。

---

### RT-05 · P1 · 回合终结时 ack 尚未注入的投递 → 消息静默丢失

**证据**

- `src/teamagents/runtime.py:530`：`self.store.ack_run_deliveries(run)` 在 `_finalize` 里无条件执行（不论该投递是否真的被模型看到）。
- `src/teamagents/storage.py:457-470`：`ack_run_deliveries` 取 `run.input_delivery_ids` 的 `MAX(batch_no)`，然后 `ack_deliveries()`（439-455 行）把该成员 **batch_no ≤ MAX 的全部 pending 投递**置为 `applied` 并推进 `last_applied_batch`。
- `src/teamagents/control.py:974-987`：运行中到达的投递只是 `append_run_inputs` + 追加到**内存** `self.mid_turn_pushes`；`runtime.py:241-250` 的 `_drain_mid_turn()` 把它交给 runner；DeepAgents 侧进入 `Inbox.items`（`runners.py:377-378, 584-586`），只有在下一次 `abefore_model` 才注入（`runners.py:253-264`）。
- `src/teamagents/runners.py:475-476`：**下一次** `start_or_resume` 的第一件事是 `inbox.items.clear()`——上一次没来得及注入的内容就此消失。
- 唤醒侧同样不再补投：`control.py:968-970` 只对 `pending_deliveries` 创建唤醒，而这些投递已经是 `applied`。

**复现**（`review/tmp/exp_mode_and_delivery.py`，C2）：成员 b 在 Leader 运行中发消息给它，Leader 的回合在其最后一次模型调用之后结束：

```
C2 leader runs: 2 | statuses: [COMPLETED, COMPLETED]
C2 pending deliveries for leader after settle: []      # 已 ack，不会重投
C2 leader observed inbox items: []                     # 内容从未进入视图
C2 message delivered_to: ['{"text": "MIDTURN-LOST", "from": "b", "target": "leader"}']
```

逐条投递状态（补充探针，直接查 `deliveries` 表）：

```
delivery: {'delivery_id': 1, 'status': 'applied', 'batch_no': 1, 'kind': 'user_message'}
delivery: {'delivery_id': 3, 'status': 'applied', 'batch_no': 2, 'kind': 'task_started'}
delivery: {'delivery_id': 4, 'status': 'applied', 'batch_no': 3, 'kind': 'message'}   # ← 从未注入，已 applied
delivery: {'delivery_id': 5, 'status': 'applied', 'batch_no': 4, 'kind': 'task_blocked'}
last_applied_batch: 4
leader observed: []
```

`message`（batch 3）已 `applied`，而 `last_applied_batch` 被推到 4——第 4 条投递甚至不在该 run 的 `input_delivery_ids` 里，只因 `MAX(batch_no)` 的写法被一并推进，说明"消费位置"的推进与"确实应用"完全脱钩。

即使把最后一个脚本步骤换成读取 inbox（同一 run 内 `_drain_mid_turn` 的推送会被读到），只要该回合不再发生模型调用，交付就已经被记成 applied——正是 §7 明令禁止的"仅放入内存队列就推进消费位置"。

**影响**：T3/T14 的"消息不丢"承诺不成立；被丢失的消息对发送方显示已投递、对接收方永不可见，且没有任何事件或标记（`applied` 与"确实看到"在库里无法区分）。§7 要求"投递采用至少一次传输，配合稳定消息 ID 和检查点确认"，实现只有批次号，没有"确实应用"的证据。

**建议**：`abefore_model` 注入后才推进批次（DeepAgents 侧已有 `applied_batch`，`runners.py:36-42`，但只在 `reconcile()` 里用于 ack，见 604-606 行）；回合终结时只 ack 真正注入过的批次，其余保持 `pending` 以便下一次唤醒。

---

### RT-06 · P1 · 取消/终结不回滚 PENDING 批准，且 `cancel` 对 `WAITING_APPROVAL` 静默无效 → `signal_done` 死锁

**证据**

- `src/teamagents/runtime.py:513-527`：`WAITING_APPROVAL` 只补发 `approval_requested` 事件并置 `AgentStatus.WAITING`，对 `FAILED`/`CANCELLED`/`OUTCOME_UNKNOWN` 的回合**不 expire**残留的 `PENDING` 批准；`expire_approval` 只被 `consume_once` 调用（`permissions.py:140-141`）。
- `src/teamagents/control.py:988-990`：调度时对 `WAITING_APPROVAL` 的回合直接跳过，**在检查 `waiting.cancel_requested` 之前**：
  ```python
  if waiting.status is TurnStatus.WAITING_APPROVAL:
      continue
  ```
  于是 `cancel_run`（`control.py:512-521`）对该回合只写 `cancel_requested=1` 并发一条 `run_cancelled` 事件，状态机里它永远停在 `WAITING_APPROVAL`（`runtime.py:314-326` 的 `_watch_cancellations` 只处理 `self._inflight` 里的 run，而等待中的 run 不在其中）。
- `src/teamagents/control.py:892-917`：`_completion_blockers` 把 `pending_approvals` 与 `WAITING_APPROVAL` 都算作 blocker → `signal_done` 持续返回 `goal not yet complete`。

**影响**：T22「有可恢复状态」与 §6.4 的完成判定被卡死；被取消的回合留下永不消解的批准项，用户与 Leader 都只能靠"手动拒绝"这个未文档化的动作脱困（仅当用户注意到批准面板里那条陈旧请求）。同一模式也影响 `OUTCOME_UNKNOWN` 回合里残留的批准。

**建议**：回合进入终止态（尤其 `CANCELLED`/`FAILED`/`OUTCOME_UNKNOWN`）时把该 run 的 `PENDING` 批准置为 `EXPIRED`；`_schedule` 对 `WAITING_APPROVAL` 的 `cancel_requested` 先行处理（取消或按 §8 转入 `DRAINING` 边界）。

---

### RT-07 · P1 · 运行期权限模式不生效（`SET_PERMISSION_MODE` 不同步闸门，T16）

**证据**

- `src/teamagents/control.py:543-550`：`SET_PERMISSION_MODE` 只做 `self.store.set_permission_mode(...)` + 一条事件，**没有触碰 `ApprovalGate`**。
- `src/teamagents/permissions.py:92-94`：`ApprovalGate.set_mode()` 是全仓唯一会更新 `policy.mode`/递增 `policy_revision` 的地方——`grep -rn "set_mode(" src/` **零调用点**（只有 `tests/test_p6_tui.py` 断言数据库值）。
- `src/teamagents/session.py:128-130`：`policy = PermissionPolicy(mode=PermissionMode(store.get_session(...)["permissions_mode"]))`——模式只在**进程启动时**读取一次。
- `src/teamagents/tui/app.py:492-500`：`Ctrl+F` 走的就是 `SET_PERMISSION_MODE`。

**复现**（`review/tmp/exp_mode_and_delivery.py`，C1）：

```
C1 before: gate.mode = approved_scope | policy_revision = 1
C1 after : gate.mode = approved_scope | db mode = full_auto | policy_revision = 1
C1 back  : gate.mode = approved_scope | db mode = approved_scope
```

**影响**：T16 的"全自动只需用户开启、无需逐次批准"在**当前进程内**不成立——用户按 `Ctrl+F` 后界面与数据库都说 `full_auto`，实际仍逐次弹批准（反向亦然：无法在运行中收紧）。另外 `policy_revision` 永远停在 1，`permissions.py:113/137-138` 依赖它区分"批准是否在策略变更前发出的"，这个保护在运行期是空转的。

**建议**：`Control` 处理 `SET_PERMISSION_MODE` 时把模式同步给运行时持有的 `ApprovalGate`（或让 gate 每次 `check()` 读 `store.get_session(...)["permissions_mode"]`）；同时用 `policy_revision` 让模式变更使旧的一次性批准失效。

---

### RT-08 · P1 · `web_fetch` 跟随重定向不重新校验 → SSRF 防护可绕过

**证据**

- `src/teamagents/tools.py:182-185`：
  ```python
  guard_url(url, allow_private=binding.env.get("allow_private") == "1")
  async with httpx.AsyncClient(timeout=30, follow_redirects=True) as client:
      response = await client.get(url, headers={"User-Agent": "TeamAgents/0.1"})
  ```
  `guard_url`（157-174 行）只解析**传入的 URL**，`follow_redirects=True` 之后的每一跳都不再经过它，最终 `str(response.url)` 还会被原样写进结果。
- 同一处还有两个次级问题：(a) `binding.env.get("allow_private") == "1"` 一旦置位就**整段跳过**校验（`tools.py:162-163`），即该防护可以被用户配置整体关闭；(b) DNS 解析（`socket.getaddrinfo`）与 httpx 实际连接是**两次独立解析**，存在经典 rebinding TOCTOU（**待验证**：需要可控 DNS/本地 server 环境，沙箱无网络）。

**影响**：`web_fetch` 可被页面内容（或 MCP/搜索返回的链接）引导到 `http://169.254.169.254/...`、`http://127.0.0.1:port/...` 等内网目标并把响应正文交给模型——正是 §12.1/T19 要防的越权取数；由于 `entries.url`/`content` 会进入会话上下文，还可作为内网探测的结果回传通道。

**建议**：改为手动重定向循环（`follow_redirects=False`），每一跳都跑 `guard_url` 并限制跳数；把"允许内网"降级为需要批准的操作而不是静默开关；连接层用解析后的 IP 直连（或在同一协程内复用解析结果）以消除 rebinding 窗口。

---

### RT-09 · P1 · `shell` 与文件工具对所有成员无条件注册，无视 `tool_bindings`

**证据**

- `src/teamagents/runners.py:437-438`：
  ```python
  tools = [*build_team_tools(self), self._shell_tool(),
           *(self._bound_tools or []), *self.extra_tools]
  ```
  `self._shell_tool()` 与外层 `FilesystemMiddleware`（由 `create_deep_agent` 自动装配）都不看 `self.agent.tool_bindings`；而 `build_bound_tools`（`tools.py:56-65`）对 `files`/`shell` 只是 `continue`，从不参与注册。
- 测试已经把这一行为固化为期望：`tests/test_p3_deepagents_runner.py:82` 断言只绑 `files` 的成员（`tests/conftest.py:21-27`）也暴露 `shell`。
- 权限侧同样默认放行：`src/teamagents/permissions.py:44` `pre_authorized = {"files", "shell"}`。

**影响**：§12.1「工具按成员筛选」、§5.2「权限不超过用户配置」、T10「越权配置被拒绝」在工具面不成立——Leader 只需在 `add_agent` 的 `tool_bindings` 里不写 `shell`（或用户在 TeamSpec 里刻意不给），成员依然能执行任意命令；配置/文档上的最小权限声明与实际能力不符（审计与信任边界失真）。

**建议**：按 `agent.tool_bindings` 条件注册 `shell`（`"shell" in bindings` 才加 `_shell_tool()`），文件工具同理（Deep Agents 支持 `tools=` 白名单/`FilesystemMiddleware(tools=[...])`）；缺绑定时的调用应被网关拒绝而不是"不存在"。

---

### RT-10 · P2 · 预授权范围不可配置：`write_paths` 死字段、`pre_authorized` 硬编码

**证据**

- `src/teamagents/permissions.py:41,48`：`write_paths` 被存储为 `self.write_paths`，但 `evaluate()`（50-72 行）**从不读取它**；`grep -rn write_paths src/` 仅命中这两行。
- `src/teamagents/session.py:128-130` 构造 `PermissionPolicy` 时只传 `mode`，`pre_authorized` 恒为默认 `{"files","shell"}`、`require_approval` 恒为空集、`write_paths` 恒为空。
- §12.2 要求"文件路径、工具、MCP 服务和联网能力有明确范围……首次进入项目时展示可修改的范围"——当前没有任何用户可见的"范围"表达（`docs/USER-GUIDE.md` 也未描述如何配置预授权）。

**影响**：A 类（与方案不符）＋安全姿态含混：MCP/web 工具靠"绑定即授权"（`permissions.py:65-68`、`runners.py:292-295`），文件工具靠名字白名单，shell 靠 workdir 挂载——三者没有统一、可读、可审计的授权来源，"用户明确授权的范围"这一要求无法落实，也无法在 TUI 展示（`tui/panels.py:277` 只显示 mode 字符串）。

**建议**：要么实现 `write_paths`/`require_approval` 的配置与判定（并在 TUI 展示/编辑），要么删除这些字段并在文档中明确"预授权=绑定本身"，避免留下"已支持范围配置"的错误印象。

---

### RT-11 · P2 · 输出体积无真正上限：`run_isolated` 全量缓冲、`web_fetch` 全量读入内存

**证据**

- `src/teamagents/execution.py:119-134`：`out_bytes, _ = proc.communicate(timeout=timeout)` 先把**全部** stdout/stderr 收进内存，之后才判断 `truncated = len(out_bytes) > max_output_bytes` 并写制品；`artifact` 的落盘发生在全量读取**之后**。
- `src/teamagents/tools.py:189-190`：`raw = response.content[:max_bytes]`——`response.content` 已经把整个响应体读进内存，`max_bytes` 只截断切片；且 `max_bytes` 是模型可控形参（179 行）。
- 两者都在"诚实上报截断"上做得不错（`execution.py:129-134` 明确标注），但内存侧没有任何保护。

**影响**：一个 `yes`/`cat /dev/zero` 式命令或数 GB 响应即可耗尽宿主内存（T23 的健壮性面；§12.1 要求"长任务输出以制品保存并分页读取，避免整段塞入上下文"——内存目标的意图未被满足）。此外 `_save_artifact`（152-159 行）用 `time.strftime('%Y%m%d-%H%M%S')`+pid 命名，同秒内两次执行会互相覆盖（P3）。

**建议**：`run_isolated` 改为边读边限流（读满 `max_output_bytes` 后落盘并继续丢弃/落盘），`web_fetch` 用 `client.stream()` + 迭代读取；`max_output_bytes`/`max_bytes` 设服务端上限。

---

### RT-12 · P2 · 一个回合内多次 `complete_task` 仅最后一个生效（upsert 覆盖）

**证据**

- `src/teamagents/storage.py:773-780`：`record_completion_request` 用 `ON CONFLICT(run_id) DO UPDATE SET task_id=excluded.task_id, ...`——每个 run 只保留**一条**完成申请。
- `src/teamagents/control.py:379-390`：`COMPLETE_TASK` 每次都直接 upsert（动作 id 由 `run_id:tool_call_id` 派生，同一回合的不同工具调用是不同 action_id，都会被接受，见 `agents.py:96`、`runners.py:144-146`）。
- `src/teamagents/runtime.py:449-476`：`_finalize` 只读这一条 `req`，`if req["task_id"]:` 分支只完成**最后提交**的那个任务。

**影响**：Leader 在同一回合里依次 `complete_task`（或成员一次汇报多个子任务）时，先提交的任务永远停在 `PENDING/RUNNING`，随后被 `control.py:1024-1045` 判为不可运行或永久未完成 → `signal_done` 被 `unfinished tasks` blocker 卡住（§6.2 的完成合约）。这是"看似成功、实际未完成"的一类静默不一致。

**建议**：`completion_requests` 改为按 `(run_id, task_id)` 记录多条，`_finalize` 遍历全部；或让 `COMPLETE_TASK` 对同一 run 的第二个 task_id 明确报错。

---

### RT-13 · P2 · `mcp_`/`web_` 前缀无条件放行（旁路批准的名字启发式）

**证据**

- `src/teamagents/permissions.py:65-68`：
  ```python
  if tool_name in self.pre_authorized or self._bound_tool(tool_name):
      return Decision(allow=True)
  if tool_name.startswith(("mcp_", "web_")):
      return Decision(allow=True)
  ```
  任何名字以 `mcp_`/`web_` 开头的工具都**跳过**批准，而不问它是否真的来自"用户配置并绑定的服务"。真正的绑定检查在别处（`runners.py:292-295` 的 `bound_tool_names()`），所以这条分支在 DeepAgents 路径上基本不可达；但它是"名字即授权"的判据，一旦成员获得名为 `mcp_*`/`web_*` 的 `extra_tools`，或 MCP 服务以该前缀命名（`tools.py:214` 的 `tool_name_prefix=True` 用服务名作前缀），就变成无批准通道。ToolGateway 路径（`agents.py:114-134`）则**先**跑该判定再调用 executor，是这条规则真正生效的地方。

**影响**：批准边界依赖命名约定，属于可被配置/服务命名意外触发的旁路；与"绑定即授权、未绑定必须批准"的表述不一致。

**建议**：删除该前缀分支，改为"在 runtime 注入的已绑定工具集合内才放行"（把 `bound_tool_names` 作为白名单传入 policy），未知工具一律请求批准。

---

### RT-14 · P3 · `_loop` 每轮泄漏 `wake.wait()` 任务；`close()` 不回收取消任务

**证据**

- `src/teamagents/runtime.py:257-261`：每轮 `asyncio.wait([... , asyncio.create_task(self._wake.wait())], ...)` 都新建一个任务，`first_completed` 之后**从不 cancel**；`_wake.clear()` 也不会让它结束，长期运行会持续堆积任务对象。
- `src/teamagents/runtime.py:122-131`：`close()` 只取消 `self._inflight`，`self._cancel_tasks`（`_watch_cancellations` 创建，314-326 行）不取消也不等待，`cleanup` 之后仍可能挂着对已关闭 store 的调用。

**影响**：长时间会话的内存/任务句柄泄漏；关闭阶段可能出现"已关闭数据库仍被访问"的日志噪声或异常。属可维护性/健壮性问题。

**建议**：把 `wake_task` 提为实例字段并复用/在每轮结束时 cancel；`close()` 里一并取消 `_cancel_tasks`（或统一用一个任务集合）。

---

### RT-15 · P3 · `_switch_effort_to_max` 就地改写共享 catalog；effort 映射结果被丢弃

**证据**

- `src/teamagents/runners.py:546-547`：`self.catalog.models[self.agent.model_profile] = profile.model_copy(update=...)`——`self.catalog` 是 `session.open_session` 里构造的**同一个** `UserConfig` 对象，被所有成员 runner 共享（`session.py:170-174`）与 `SessionRuntime.catalog` 共享，因此一个成员的 effort 回退会改掉同进程其他成员的同名 profile。
- `src/teamagents/providers.py:64`：`normalized, _note = normalize_effort(profile)`——D-8 要求"自动回退 max 并记住结果"，这里的 `note`（"reasoning_effort xhigh -> max (deepseek)"）被直接丢弃，没有任何事件/日志告诉用户发生了校准。

**影响**：跨成员配置污染（T7「更换配置不改变角色与 ACL」的精神面）；用户看不到 D-8 承诺的校准记录，排查"为什么这个成员用了 max"没有线索。

**建议**：回退只改该 runner 私有的 profile 副本（或回退标记挂在 runner 上）；`build_chat_model` 返回/透出 note，由 runner 发一条事件或 INFO 日志。

---

### RT-16 · P3 · 死代码与命名瑕疵

**证据**

- `src/teamagents/execution.py:167-198`：`GuardedFilesystemBackend`（符号链接/穿越防护）**无任何引用**（`grep -rn GuardedFilesystemBackend src/` 只有定义与 205 行的继承声明，而 `IsolatedShellBackend` 只被当 backend 用，其 `_resolve_path` 由 deepagents 调用）。真正提供"路径检查必须覆盖符号链接与穿越"（§12.2）的是 deepagents 的 `FilesystemBackend`（`virtual_mode=True`，`backends/filesystem.py:203-215`），而**额外路由**用的正是它——见 RT-01。
- `src/teamagents/execution.py:152-159`：`_save_artifact` 名字精确到秒，同秒两次执行覆盖同一文件。
- `src/teamagents/agents.py:95`：`ToolGateway.call` 里的 `self._seq += 1` 从未被读取；`_execute_tool` 用 `kind=ActionKind.COMPLETE_TASK` 只是为了让 `Receipt.kind` 有值（110-113 行），语义误导。
- `src/teamagents/runtime.py:456`：`__import__("json").loads(...)` 内联导入；同文件 158、278 行存在 `_ = run` / `_ = leaders_busy` 之类的占位语句（`leaders_busy` 统计后从未使用，266-311 行）。
- `src/teamagents/permissions.py:7-8` 的模块 docstring 说"路径检查（符号链接/穿越）与 bubblewrap 范围落在 P3；本模块已拥有决策流"——P3 已完成，注释未更新（`GuardedFilesystemBackend` 甚至没被接上）。

**影响**：可维护性；其中"死掉的文件路径防护"会误导后续维护者以为越界写已被防住（RT-01 的实际证据）。

**建议**：删除或真正接入 `GuardedFilesystemBackend`（优先接入）；清理占位语句与误导性注释；`_save_artifact` 加随机后缀或微秒。

## 四、与方案/文档的偏差（A 类）

| 编号 | 方案要求 | 实现现状 | 结论 |
|---|---|---|---|
| RT-01/RT-02/RT-09 | §12.2「默认模式采用统一工具检查」；D-2「ToolGateway 是所有成员工具调用的唯一入口」 | 文件路由、默认私有子代理、无条件注册的 `shell` 都不经过网关/批准 | 与基准不符，非 DECISIONS 记录的偏离 |
| RT-07 | §12.2/T16「`full_auto` 由用户在可信 TUI/启动参数中开启」 | 运行期切到 `full_auto` 不改变实际判定；反向也无法收紧 | 与基准不符 |
| RT-10 | §12.2「预授权范围明确、可在首次进入项目时查看与修改」 | `write_paths` 死字段、`pre_authorized` 硬编码、无 TUI 展示 | 与基准不符（属未完成而非简化） |
| RT-03 | §9.3「取消要等待执行停止的确认」 | 立即自报 CANCELLED，无停止手段 | 与基准不符 |
| RT-05 | §7「不能在仅放入内存队列后就推进消费位置」 | `ack_run_deliveries` 按批次推进，含未注入的投递 | 与基准不符 |
| RT-15 | D-8「自动回退 max 并**记住结果**」 | `normalize_effort` 的 note 被丢弃、无事件 | 与已确认决策的落地不完整 |

## 五、未验证/存疑项

1. **RT-04 的跨进程复现**：判断依据是 `reconcile()` 只处理 RUNNING + `_paused_kind` 内存化（`runtime.py:137-159`、`runners.py:359,481-496`）。已确认代码路径，但未构造带持久 checkpointer 的两进程脚本；标注「待验证」。
2. **RT-08 的 DNS rebinding**：`guard_url` 与 httpx 两次解析之间的 TOCTOU 是代码结构推断（`tools.py:164-174,183`），沙箱无网络与可控 DNS，未实测；**重定向绕过**已由代码路径确认（`follow_redirects=True` + 单次校验），但同样未做端到端复现（可用 `httpx.MockTransport` 构造 302→内网地址来演示，本轮未做）。
3. **`execute` 内置工具**：本仓 backend（`CompositeBackend` 包 `IsolatedShellBackend`）是否会额外暴露 deepagents 的 `execute` 工具未逐一验证；探针中模型可见工具列表**不含** `execute`（`review/tmp/exp_subagent.py` 输出），故不列为发现，但若 backend 组合变化需复查。
4. **MCP stdio 生命周期**（`tools.py:205-220`）：注释声称"每次调用一个短生命周期会话（不泄漏进程）"，本轮未验证 `MultiServerMCPClient.get_tools()` 是否为每次工具调用重新 spawn 子进程、取消/超时是否会留下孤儿进程（T13「无遗留工具进程」）。标注「待验证」。
5. **`register_external` 的哈希口径**（`permissions.py:143-155`）用 `operation_hash(scope.get("kind"), scope.get("request"))` 与 `check()` 的 `operation_hash(tool_name,args)` 不同源，可能与 Codex 侧批准/会话缓存的匹配逻辑错位——属适配器域主责，此处仅记录，未验证。
6. **`uniq_active_run` 唯一索引**（`storage.py:93-94`）与 `_start_ready_runs` 同轮为同一成员创建两个任务的窗口：`control._schedule` 的 `_active_runs` 检查看起来已挡住（`control.py:971-973`），未构造竞态复现。

## 六、只读合规与交叉核对

### 只读合规自检（未修改任何被审文件）

审查期间只在 `review/**` 下写入（本报告 + `review/tmp/` 三个脚本）。被审文件 mtime 均早于本会话开始（`runtime.py`/`agents.py` 01:24、`execution.py` 23:18、`permissions.py` 23:49、`tools.py` 23:48、`providers.py` 10:39，而 `review/` 目录创建于 10:52），未对 `src/**`、`tests/**`、`docs/**` 做任何写操作；复现脚本全部在临时目录（`tempfile.mkdtemp`）内制造数据。

### 与其他审查员报告的交叉核对（PROTOCOL 第 7 条）

- `review/findings-core.md`（control/storage/models）：其 F-C1（Limits 默认值）、F-C2（`_reduce` 异常吞掉后半提交）与本域无重叠；F-C2 会放大本报告 RT-12、RT-05 的后果。
- `review/findings-verify.md`：V-3（`bump_context_epoch` 死代码）与 `runners.py:382-384` 的检查点线程键直接相关——本域确认 epoch 恒为 1，故"移除后同名重建"会复用同一 thread；V-4（T23 无自动化证据）与 RT-01/RT-10 一致：路径防护缺的正是自动化用例。
- `review/findings-surface.md`：其「项目配置可改写 base_url / 注入 MCP 命令」与本域 RT-01 是两条独立越权链（前者靠项目文件，后者靠运行时文件工具写用户级配置），建议汇总时并列。
- 未发现与他域重复的条目。

## 七、自检（实际运行过的命令与结果）

| 命令 | 结果 |
|---|---|
| `grep -rn "set_mode(\|write_paths\|policy_revision" src/ tests/` | 确认 `ApprovalGate.set_mode` 在生产代码零调用、`write_paths` 只被赋值 |
| `grep -rn GuardedFilesystemBackend src/` | 仅命中定义处 → 死代码（RT-16） |
| `grep -rn "completion_request\|completion_requests" src/teamagents/*.py` | 确认 `record_completion_request`/`completion_request` 为唯一读写点（RT-12） |
| `.venv/bin/python review/tmp/exp_escape.py` | `outside-workdir config.toml written: True`；`skill file overwritten: True`；`approvals raised: []` → RT-01 |
| `.venv/bin/python review/tmp/exp_subagent.py` | 批准卡只描述 `task`；子代理 `shell`（`network=True`）执行成功且**无新批准** → RT-02 |
| `.venv/bin/python review/tmp/exp_mode_and_delivery.py` | `gate.mode` 恒为 `approved_scope` 而 DB 为 `full_auto` → RT-07；`MIDTURN-LOST` 已投递、已 ack、从未被观察 → RT-05 |
| `.venv/bin/python -m pytest tests/test_p1_guards.py tests/test_p3_deepagents_runner.py -q` | `13 passed in 2.74s`（本域相关的确定性用例在本环境通过；我的发现均落在这些用例未覆盖的路径上） |
| 只读阅读 `deepagents 0.7.13`：`graph.py:790-900`、`middleware/subagents.py:577-720`、`backends/composite.py`、`backends/filesystem.py:203-215`、`middleware/filesystem.py:1366-1380,2857-2900` | 判定内置文件工具/子代理的装配方式（RT-01/RT-02 的依据） |
| `cat /proc/net/dev`（宿主） | 沙箱仅 `lo`，故 `shell(network=True)` 的网络可达性未做端到端证明，只证明"闸门未被调用" |
| `docs/P0-findings.md` 第 4 节（bwrap 探针：`$HOME` 不可见、`/etc` 只读、网络不可达、`/proc/1/root` 不可见） | 与本次代码阅读一致：`execution.py:68-93` 的挂载/命名空间参数与探针描述相符（`--ro-bind /usr /etc /opt`、`--tmpfs /tmp`、`--unshare-pid/ipc/uts`、`--die-with-parent`、默认 `--unshare-net`、`--ro-*`/`--bind`） |

未跑全量 `pytest`（本域结论不依赖通过率；`RT-*` 均由代码路径 + 上述探针支撑）。未修改任何 `src/**`、`tests/**`、`docs/**`、`examples/**` 文件。
