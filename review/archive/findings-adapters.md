# 代码审查报告 · 适配器、工作目录与会话域（adapters）

审查员：reviewer_adapters ｜ 任务：task_3aa9cdae0c52 ｜ 日期：2026-09-12

## 一、范围与方法

**精读对象（全量）**

| 文件 | 行数 | 覆盖 |
|---|---|---|
| `src/teamagents/codex.py` | 459 | 全文 |
| `src/teamagents/runners.py` | 620 | 全文 |
| `src/teamagents/workspace.py` | 135 | 全文 |
| `src/teamagents/session.py` | 193 | 全文 |
| `src/teamagents/sessions.py` | 210 | 全文 |

**交叉阅读（为判定调用契约/语义）**：`src/teamagents/runtime.py`（`reconcile` / `_execute_inner` / `_finalize` / `_watch_cancellations`）、`control.py`（`_schedule` / `_completion_blockers` / `_wake_approval_run` / `_cancel_task`）、`permissions.py`（`ApprovalGate`、`register_external`）、`storage.py`（`ack_deliveries`/`set_codex_thread`/`pending_approvals`）、`cli.py`（doctor 的 Codex schema 校验）、`tui/app.py`、`tui/panels.py`。

**参照测试（理解语义，不评测试本身）**：`tests/test_p5_codex_adapter.py`、`tests/test_p5_workspace.py`、`tests/test_p6_sessions_ui.py`、`tests/test_p2_recovery.py`、`tests/conftest.py`、`tests/fake_codex_app_server.py`、`tests/scripted_model.py`。

**基准**：方案 §5.1、§9.1–9.3、§10.1–10.2、§12.1–12.3、§13、§16（P5/P6 完成条件）、§17（T8/T13/T17/T18/T22/T24）；`docs/DECISIONS.md` D-4（Codex 默认配置）、D-8（effort 回退）、D-9（会话管理）。

**命令与探针**（全部只读，源码/测试未改动；脚本写在 `review/tmp/`，输出存 `review/tmp/out/`）

> 修订说明：审查期间 `src/teamagents/runners.py` 于 11:11 被外部改动（`max_model_steps=50` 字面量 → `limits.max_model_steps_per_turn`，+3 行）。本报告对 `runners.py` 的引用行号以**该修订后版本**（623 行）为准；`codex.py` / `workspace.py` / `session.py` / `sessions.py` 在此期间未变。

```bash
.venv/bin/python -m pytest tests/test_p5_codex_adapter.py -q      # 5 passed in 2.22s
.venv/bin/python -m pytest tests/test_p5_codex_adapter.py tests/test_p5_workspace.py \
    tests/test_p6_sessions_ui.py tests/test_p2_recovery.py -q     # 20 passed in 10.37s（11:11 修订后复跑）
.venv/bin/python review/tmp/repro_lock_leak.py                    # AD-2
.venv/bin/python review/tmp/repro_worktree_reopen.py              # AD-1
.venv/bin/python review/tmp/repro_worktree_open_session.py        # AD-1
.venv/bin/python review/tmp/repro_session_id_collision_a.py       # AD-3
.venv/bin/python review/tmp/repro_codex_process_death.py          # AD-4
.venv/bin/python review/tmp/repro_codex_midturn_dup.py            # AD-5
.venv/bin/python review/tmp/repro_codex_progress_dup.py           # AD-6
.venv/bin/python review/tmp/repro_orphan_approval.py              # AD-7
.venv/bin/python review/tmp/repro_list_sessions_robust.py         # 反证：无缺陷
```

## 二、结论（摘要）

1. **`git_worktree` 成员的会话无法第二次打开**：`prepare()` 每次都在同一路径用新分支 `git worktree add`，`open_session` 必调它 —— 恢复/重启/TUI 切回直接 `WorkspaceError`（AD-1，P1）。这条使 §16 P5「worktree 成果保留」与 T24「会话复用」在真实工作目录策略下不可达。
2. **`open_session` 中途失败会泄漏会话锁 fd**，同进程内后续一切会话操作被误报为「另一个进程正在运行」（AD-2，P1）；与 AD-1 叠加后，worktree 会话一次失败即永久不可用。
3. **归档后 session id 可被复用**，`list_sessions` 返回同 id 两条记录，TUI 会话面板按 id 作行键（Textual `DuplicateKey`）、删除/切换出现歧义目标（AD-3，P1）。
4. **Codex 后端进程中途死亡时回合不会进入结果不明**：`start_or_resume` 挂起至 900s 的 `turn_active_timeout_s` 才 FAILED，期间成员槽位与任务卡住（AD-4，P1）；**重启后停在 `WAITING_APPROVAL` 的 Codex 回合既不核对也不可恢复，用户补批批准会让该回合整段重放并再次请求批准**（AD-7，P1，已实测）。
5. 其余为语义/一致性缺陷：Codex 中途消息重复注入（AD-5，P2）、Codex 进度与最终摘要文本重复（AD-6，P2）、`sandbox` 未按 D-4 逐回合显式传入（AD-8，P2）、隔离工作目录成果在删除会话时被静默丢弃（AD-9，P2）、`reconcile` 用「线程最后一个回合」推测当前 run（AD-10，P2）。**未发现可直接绕过权限/ACL 的 P0 级问题**：Codex 侧无团队工具注入、成员 id 有字符白名单、git 调用无 shell 注入面。

## 三、发现清单

> 严重度：P0 安全边界/数据损坏/谎报成功；P1 关键语义错误、竞态、恢复窗口、异常吞没；P2 边界/错误处理、文档与实现不符；P3 可维护性。

### AD-1（P1）`git_worktree` 成员的会话无法第二次打开：`prepare()` 在固定路径重复 `worktree add`

- 证据：`src/teamagents/workspace.py:71-79`
  ```python
  branch = f"teamagents/{agent.id}-{int(time.time())}"
  ...
  result = _git(project_cwd, "worktree", "add", "-b", branch, str(path), base or "HEAD", timeout=120)
  if result.returncode != 0:
      raise WorkspaceError(f"git worktree add failed: {result.stderr.strip()}")
  ```
  `src/teamagents/session.py:74-85` 每次 `open_session` 都调用 `prepare(agent, cwd, member_dir)`，`member_dir = paths["base"]/"members"/agent.id` 是**稳定路径**，`Worktree` 已存在也不复用。
- 复现（`review/tmp/out/repro_worktree_reopen.txt`）：
  ```
  1st prepare -> git_worktree /tmp/.../members/b/work teamagents/b-1789181760
  2nd prepare RAISED WorkspaceError: git worktree add failed: Preparing worktree (new branch 'teamagents/b-1789181760')
  fatal: a branch named 'teamagents/b-1789181760' already exists
  3rd prepare (1s later) RAISED WorkspaceError: fatal: '/tmp/.../members/b/work' already exists
  ```
  端到端（`repro_worktree_open_session.txt`）：首次 `open_session` 成功，第二次（同样的 session_id/cwd/spec）直接抛 `WorkspaceError`（分支名冲突；1 秒后换成路径冲突，两条都不通）。
- 影响：T24「同一会话继续使用原成员上下文」、T18「worktree 均通过」、§9.2「进程恢复」在 `git_worktree` 策略下全部失效；每次重启都新建残留分支（`git branch` 里可见 `+ teamagents/b-...`），加速 git ref 目录膨胀。
- 建议：`prepare()` 先检测 `member_dir/"work"/".git"` 已存在（或 `git worktree list` 已登记该路径）→ 直接复用 `Workspace(path=..., policy=GIT_WORKTREE, branch=<git -C work rev-parse --abbrev-ref HEAD>, base_commit=<merge-base>)`；分支名改为稳定（如 `teamagents/{agent.id}`）或检测存在后加后缀；删除 worktree 后 `git worktree prune`。同时把 worktree 目录从 `list_sessions` 的体积统计口径里排除或标注。

### AD-2（P1）`open_session` 中途失败泄漏会话锁 fd → 同进程后续会话操作被误报 `SessionInUse`

- 证据：`src/teamagents/session.py:112-174`
  ```python
  114      lock_handle = acquire_session_lock(paths)
  ...
  132      stack = contextlib.AsyncExitStack()
  133      stack.callback(lambda: os.close(lock_handle))     # 这里才登记释放
  ...
  170      runners = {agent.id: make_runner(agent) for agent in spec.agents}   # 任何异常都逃逸
  ```
  116–170 之间的任何异常（AD-1 的 `WorkspaceError`、`AsyncSqliteSaver`/模型构建/`build_bound_tools` 失败）都不会执行 `os.close(lock_handle)`，也不会 `stack.aclose()`。
- 复现（`review/tmp/out/repro_lock_leak.txt`）：
  ```
  after clean close: locked = False lock fds: []
  2nd open RAISED WorkspaceError: git worktree add failed: ...
  after FAILED open: locked = True lock fds: ['6']
  retry RAISED SessionInUse: session is already running in another process (lock: .../session.lock)
  ```
  `/proc/self/fd/6 -> .../sessions/s1/session.lock` 证明 fd 泄漏。
- 影响：失败仅是初始化错误，却把会话标成「运行中」：TUI 的归档/删除/切换（`tui/app.py:377-423`）全部被 `SessionInUse` 拒绝并显示误导性原因；此外该 fd 会一直占用到进程退出（P1 级联：单次失败即需要重启 TUI）。
- 建议：`lock_handle` 拿到后立即 `stack.callback(...)`（把 `stack` 建在 acquire 之前），或 `try/except BaseException: os.close(lock_handle); raise`。另外 `make_runner` 循环应整体在 `try` 内，失败时同时释放已建的 runner（Codex 子进程此刻尚未启动，风险低）。

### AD-3（P1）归档后 session id 可复用 → `list_sessions` 重复 id、TUI 行键冲突、删除/切换目标歧义

- 证据：`src/teamagents/sessions.py:127-137`
  ```python
  root = sessions_dir()
  existing = {p.name for p in root.iterdir()} if root.is_dir() else set()
  base = default_session_id(cwd)
  if base not in existing:
      return base
  ```
  只扫描 `sessions/`（不含 `sessions/archived/`）；而 `archive_session`（`sessions.py:161-179`）把目录移动到 `sessions/archived/<id>`，于是「归档 → 新建」会重新分配到同一 id。
- 复现（`review/tmp/out/repro_session_id_collision_a.txt`）：
  ```
  archived: ['proj_5f6e98b6c554']
  new_session_id after archiving: proj_5f6e98b6c554 -> collides: True
  list_sessions ids+archived: [('proj_5f6e98b6c554', False), ('proj_5f6e98b6c554', True)]
  duplicate session ids visible to the UI: True
  ```
- 影响面：
  1. `tui/panels.py:341-355` 用 `table.add_row(..., key=info.session_id)` → Textual `DataTable` 对重复 row key 抛 `DuplicateKey`（`.venv/.../textual/widgets/_data_table.py:1693`）；`tui/app.py:319-333` 的 `_refresh_panel_for`/`on_panel_ready` 用 `contextlib.suppress(Exception)` 吞掉异常 → 会话面板静默停留在旧内容或只渲染一半（用户看不到任何报错）。
  2. 切换/归档/删除都以 `info.session_id` 定位（`panels.py:357-390`、`app.py:351-423`）：选中的「已归档」行会把**当前活动会话**关掉/删除（`delete_session` 按 id 解析到 `sessions/<id>`，语义与用户看到的行不一致）；删除归档行时 `delete_session` 只查 `sessions/<id>`，报 `FileNotFoundError`。
  3. `is_session_locked(path.name, group_root)`（`sessions.py:115`）让新旧两个 id 相同的会话共享锁文件语义 → 「运行中」标记可能指向另一份记录。
  4. 审计/恢复混淆：`--resume <id>` 与 T8 的会话清单指向两个不同历史，违反 §5.1「会话是持久化与隔离的单位」。
- 建议：`new_session_id` 同时扫描 `archived/`；或归档时改名为 `f"{id}__{int(time.time())}"` 并保留原 id 字段；无论如何给 `SessionInfo` 加 `archived` 复合键并让 TUI 行键用 `f"{'A' if archived else 'L'}:{id}"`，`selected_session()` 返回结构化对象而非裸 id。

### AD-4（P1）Codex 后端进程中途死亡：回合不落 `OUTCOME_UNKNOWN`，挂起到 900s 超时才 FAILED

- 证据：`src/teamagents/codex.py:74-99`（`_read_loop` 在 stdout EOF 时只结算 `_pending` 里的 call future，对已提交的回合不做任何处理）
  ```python
  for future in self._pending.values():
      if not future.done():
          future.set_exception(CodexError("app-server closed the connection"))
  self._pending.clear()
  ```
  `_turn_done[run_id]` 无人唤醒，`self._states[run_id]` 停留 `RUNNING`，`start_or_resume` 的 `await done`（`codex.py:291`）一直等；唯一出口是 runtime 的 `asyncio.wait_for(..., timeout=spec.limits.turn_active_timeout_s)`（`runtime.py:387-393`，默认 **900s**，`models.py:231`），且结果是 `FAILED`（"turn active-time limit reached"）而非 `OUTCOME_UNKNOWN`。
- 复现（`review/tmp/out/…`/终端输出）：
  ```
  turn started: turn-1 state: RUNNING
  after child SIGKILL: task.done = False | _turn_done done = False | _states = RUNNING | proc.returncode = -9
  turn STILL HANGS 5s after the backend died -> only runtime turn_active_timeout_s (default 900s) can stop it
  ```
  脚本：`review/tmp/repro_codex_process_death.py`。
- 影响：与 §9.2「外部工具已执行但结果未知 → 保持 OUTCOME_UNKNOWN」和 §10.2「断线后先核对线程历史及活动回合；无法确认时进入结果不明状态」直接冲突；成员槽位（`max_parallel_workers`）与任务被占 15 分钟，Leader 无法 `signal_done`（`control.py:892-916` 把活跃回合列为 blocker）；`aclose()`/`close()` 也没有「进程已死 → 立即结算」的分支。
- 建议：`_read_loop` 结束时（或 `_drain_stderr` 观察到退出）对每个未 `done()` 的 `_turn_done` 设 `OUTCOME_UNKNOWN` 并触发 `status_hook`；`start_or_resume` 改为 `asyncio.wait([done, proc.wait()], FIRST_COMPLETED)`；补 `doctor`/live 测试覆盖「杀掉 app-server 子进程」场景。

### AD-5（P2）Codex 中途消息重复注入：同一内容在同一 prompt 出现两次（违反 T3「重复投递不重复注入」）

- 证据链路：
  1. `control.py:975-987`：成员回合 RUNNING 时，新投递追加到 `run.input_delivery_ids`（`append_run_inputs`）并 push 到 `mid_turn_pushes`；
  2. `runtime.py:241-250` → `codex.py:431-434` `deliver_mid_turn` 只把文本塞进**内存** `_queued_input`（注释说 "updates ride the next turn"）；
  3. `codex.py:316-326` `_render_input` 把队列拼成 `<queued_updates>`，**同时** `render_view` 已把该 delivery 渲染进 `<inbox>`（`runners.py:74-76`，delivery 仍是 pending，因为 `runtime.py:530` 的 `ack_run_deliveries` 用的是 `_execute_inner` 起始处的 run 快照 `runtime.py:365`，不含中途追加的 id）。
- 复现（`review/tmp/repro_codex_midturn_dup.py`，输出见 `review/tmp/out/`）：
  ```
  A) queued only in memory: ['{"text": "IMPORTANT UPDATE", ...}']
  B) deliveries: [(2, 1, 'applied'), (4, 2, 'pending')]
     pending_deliveries(cx): [4]
  C) second turn count of 'IMPORTANT UPDATE': 2
  C) has queued_updates block: True
  C) prompt1 queued block: False
  ```
- 影响：Codex 成员在同一回合上下文里看到两遍同一条指示（`<inbox>` + `<queued_updates>`），可能重复执行；T3 的「重复投递不重复注入」在适配器边界被破坏。另注：文本只存在于内存，进程退出即丢（delivery 仍 pending，会被重新注入，所以不是静默丢失，但「已投递」语义仍不准确）。
- 建议：二选一——(a) `deliver_mid_turn` 对 Codex 直接丢弃（下一次调度自然注入 pending delivery，语义与 Deep Agents 的 `<inbox>` 一致）；或 (b) 注入后立即 `store.ack_deliveries(...)` 对应 delivery 并把 `input_delivery_ids` 落到 DB。任一方案都比「两处都注入」好。

### AD-6（P2）Codex 进度/最终摘要文本重复：`item/agentMessage/delta` 与 `item/completed` 双份累积

- 证据：`src/teamagents/codex.py:344-352`
  ```python
  if method == "item/agentMessage/delta":
      ... self._progress.setdefault(run_id, []).append(delta)
  elif method == "item/completed":
      item = params.get("item") or {}
      if item.get("type") == "agentMessage" and item.get("text"):
          self._progress.setdefault(run_id, []).append(item["text"])   # 与 delta 重复
  ```
  随后 `codex.py:303`（`complete_task` 的 summary）、`codex.py:311`（`reply_text`）、`codex.py:356-362`（进度增量）都用 `" ".join(...)` 消费同一个 list。
- 复现（`review/tmp/repro_codex_progress_dup.py`）：
  ```
  outcome.status: COMPLETED
  reply_text: 'fake reply fake reply'
  progress events: ["'fake reply'", "'fake reply fake reply'"]
  ```
- 影响：① `complete_task` 的 summary 落进任务结果（T17「最终结果映射正确」）；② `run_progress` 事件重复（UI/共享空间摘要）；③ `_reported` 游标按 list 长度切分，一旦重复就会把重复文本当"新增"再上报一次。
- 建议：以 `item.id`（或 `params.itemId`）去重：收到 `item/completed` 时丢弃该 item 已累积的 delta，或用「只有 delta 或只有 item/completed」的单一来源。

### AD-7（P1）重启后停留在 `WAITING_APPROVAL` 的 Codex 回合：不核对、不可恢复，补批批准会把整段输入作为新回合重放

- 证据：
  1. `runtime.py:137-159` `reconcile()` **只**遍历 `[TurnStatus.RUNNING]`；`WAITING_APPROVAL`/`WAITING_TASK` 的回合不会被核对，也不会转成 `OUTCOME_UNKNOWN`。
  2. 复现（`review/tmp/repro_orphan_approval.py`）：
     ```
     parked approval: appr_97016af069374f49 run: WAITING_APPROVAL
     after restart, approval in DB:
       pending approvals: ['appr_97016af069374f49'] | run status: WAITING_APPROVAL
     ```
     （Codex 子进程随主进程死亡，`CodexRunner.resolve_approval`（`codex.py:398-406`）在新的 runner 上找不到等待中的 future → 返回 False，没有任何路径唤醒该 run；任务停在 RUNNING。）
  3. 用户此时若决定批准：`control.py:1171-1175 _wake_approval_run` 把 run 置回 `RUNNING` → `runtime.py:279-286` 重新 `_execute` → `codex.py:246-269` 用**持久化的 thread_id** 再次 `turn/start`，输入文本由 `_render_input(view, wake)` 重新渲染 —— 即「同一任务输入的新 Codex 回合」。旧回合可能已经把命令执行完（用户批准的正是它），属于 §9.2「外部工具已执行但结果未知」窗口，却被当成可安全重试。
     **已实测（重跑 `repro_orphan_approval.py`，追加批准决策）**：
     ```
     approval_decision receipt: True {'approval_id': 'appr_df87685066bd40d8', 'status': 'APPROVED_ONCE'}
     runs after the decision: [('c341b9', WAITING_APPROVAL, 'turn-1'), ('b6412b', WAITING_TASK, None)]
     events tail: [..., ('run_started', '{"run_id": "run_...c341b9", "agent_id": "cx", "wake": "approva'),
                   ('approval_requested', '{"approval_id": "appr_cab59887f4144efa", "run_id": "run_...c341b9')]
     ```
     即：run 被重新执行，Codex 侧以全新回合重放同一输入，并且**又发起了一次新的批准请求**（同一操作被重复询问）；同时 `codex.py:286-288` 用新回合 id 覆盖了 `external_turn_id`（此处新 app-server 进程重新计数为 `turn-1`），**原提交意图的回合引用被覆盖**，恢复/审计线索丢失。
- 影响：① T8 要求的「恢复…成员线程和批准」不完整；② 任务永久 RUNNING + `_completion_blockers` 把 pending approval 列为 blocker（`control.py:892-916`）→ 该会话目标无法 `signal_done`，只能靠用户拒绝/删会话；③ 补批批准可能重复外部副作用。
- 建议：`reconcile` 覆盖 `WAITING_APPROVAL`/`WAITING_TASK`：对 Codex 回合（有 `external_turn_id` 或 external thread）先 `thread/read` 核对，无法确认则 `OUTCOME_UNKNOWN` 并把 pending 批准置 `EXPIRED`（`storage.expire_approval` 已存在但只用于 ONCE 消费）；恢复时禁止用同一个 run 直接重放 `turn/start`。

### AD-8（P2）D-4 声称「每回合显式传 sandbox」未落实：sandbox 只在 `thread/start` 传一次

- 证据：`src/teamagents/codex.py:256-263`（回合参数只有 `threadId`/`input`/`approvalPolicy`/`approvalsReviewer`/`effort`）
  ```python
  params = {"threadId": thread_id, "input": [...],
            "approvalPolicy": self.approval_policy, "approvalsReviewer": "user"}
  if self.effort: params["effort"] = self.effort
  ```
  `sandbox` 只在 `_ensure_thread` 的 `thread/start`（`codex.py:231-235`）出现；恢复走 `if self.thread_id: return self.thread_id`（`codex.py:229-230`），既不重新 `thread/resume` 也不重传 sandbox/approvalPolicy。
- 影响：与 `docs/DECISIONS.md` D-4「每回合显式传 sandbox/approvalPolicy/approvalsReviewer="user"，不沿用本机 auto_review 的宽松设置」不一致。若真实 Codex 对 resumed thread 沿用持久化/本机配置，P0 里实测过的「本机会放行 /tmp 写入」风险会在恢复路径回归。`tests/fake_codex_app_server.py` 完全忽略这些参数，所以现有测试无法发现（test_p5_codex_adapter 只验证握手/消息/批准/取消）。
- 建议：把 sandbox 加入 `turn/start` 参数（若 CLI 版本支持），或在 resume 路径显式 `thread/resume` 重传；至少让 `doctor` 校验 `turn/start` 的 params schema 含 `sandbox`，并在 live 测试（`tests/test_p5_live_codex.py`）里断言本机宽松配置被覆盖。标注：**部分待验证**（本沙箱无 `codex` CLI，无法对齐真实 schema）。

### AD-9（P2）`delete_session` 只保护 worktree 成果：`isolated` 成员的成果被静默删除

- 证据：`src/teamagents/sessions.py:140-149` 只收集 `members/*/work/.git` 为**文件**（worktree）的目录；`sessions.py:195-210` 只对这些目录跑 `cleanup`，随后 `shutil.rmtree(path)`（`sessions.py:210`）把整个会话目录（含 `members/<id>/work/` 里的隔离成果、`artifacts/`）一并删除。`workspace.cleanup` 里针对 `ISOLATED` 的保护分支（`workspace.py:111-113` "isolated directory still holds results; archive them first"）在删除路径上**从未被调用**。
- 影响：§12.3「存在未合并成果…的目录不能自动删除」「成果和工作目录清理另行记录」对 `isolated` 策略失效；T13/T18 的成果保留要求被违反（数据丢失）。
- 建议：`delete_session` 遍历所有 `members/*/work`：对 worktree 用现有 `cleanup`，对 isolated 目录复用 `Workspace(path=..., policy=ISOLATED)` + `cleanup`（同样要求先归档成果），或至少要求显式 `--force`。

### AD-10（P2）Codex `reconcile` 用「线程最后一个回合」的状态推断当前 run，未校验回合身份

- 证据：`src/teamagents/codex.py:436-454`
  ```python
  turns = ((result or {}).get("thread") or {}).get("turns") or []
  if not turns: return None
  last = turns[-1]
  status = _TURN_STATUS_MAP.get(last.get("status", ""), TurnStatus.OUTCOME_UNKNOWN)
  ```
  未比对 `last["id"]` 与 `run.external_turn_id`（DB 里已有该字段），也没有把 `inProgress`（映射为 `RUNNING`）→ `OUTCOME_UNKNOWN` 之外的语义。
- 影响：若线程被外部（Codex CLI 自身、用户）继续使用，或 DB 的 `external_turn_id` 与线程末尾回合不一致，会把**别的回合**的结论写成本 run 的结果（`runtime.py:144-146` 直接 `set_run_status`）；`tests/test_p5_codex_adapter.py:107-111` 的断言 `state in ("COMPLETED", None)` 过宽，掩盖了这一点。
- 建议：`reconcile` 按 `run.external_turn_id` 在 `turns` 中查找；找不到匹配则返回 `OUTCOME_UNKNOWN`；测试改成断言具体状态。

### AD-11（P2）`reconcile` 与 thread 的 `RUNNING` 映射：`inProgress` → `Running` 被显式排除，但**非当前 run** 的完成态会被误用（同 AD-10）

合并入 AD-10，不单列。

### AD-12（P3）Codex per-run 状态字典与 `_queued_input` 无清理/无上限

- 证据：`src/teamagents/codex.py:202-210`（`_states`/`_current_turn`/`_turn_done`/`_progress`/`_reported`/`_approval_ids`/`_buffered_notes`）只写不删；对比 `runners.py:534`（`_paused_kind.pop`）与 `runtime.py:531`（`_step_counts.pop`）。`_queued_input` 也无上限。
- 影响：长会话（多回合）内存单调增长；`_buffered_notes` 在 `turn/start` 失败时残留（`codex.py:268-288` 失败分支未清理 `_turn_done`/`_progress`）。
- 建议：`start_or_resume` 收尾（含失败分支）统一清理本 run 的条目；给 `_queued_input` 设上限并记事件。

### AD-13（P3）`list_sessions` 体积统计把 worktree 全量文件计入，且 `rglob` 无异常保护

- 证据：`src/teamagents/sessions.py:117-118`
  ```python
  info.size_mb = round(sum(f.stat().st_size for f in path.rglob("*") if f.is_file()) / 1e6, 1)
  ```
  worktree 成员的工作副本（可能是完整仓库）会算进「会话占用」；`path.rglob`/`stat` 遇到不可读目录会抛 `OSError`，而此处没有 try/except（仅 `_read_meta` 有）。
- 反证与说明（已测）：**断链符号链接不会崩**（`repro_list_sessions_robust.py`：`is_file()` 为 False），故本条只针对不可读目录/权限异常，属 P3。
- 建议：`try/except OSError: info.size_mb = -1`（或只统计 `team.db`/`artifacts/`），并排除 `members/*/work`。

### AD-14（P3）`runners.py` 配置版本重建与绑定工具的一致性

- 证据：`runners.py:427-457` `_ensure_graph` 以 `run.config_revision` 为缓存键（符合 DP-6），但 `_bound_tools` 只在第一次 `start_or_resume` 构建（`runners.py:465-471`），且 `bound_tool_names()`（`runners.py:591-593`）依赖它 → 绑定被移除的成员在 runner 被替换前仍可调用旧工具；运行时确实会在配置版本变化时替换 runner（`runtime.py:78-115`），但 `_ensure_graph` 的 revision 缓存与 `_bound_tools` 的「一次性」语义不一致，容易在后续改动中产生错配。
- 建议：把 `_bound_tools` 与 `_graph_revision` 一起在版本变化时重建（或干脆去掉 graph 级缓存、由 runner 替换承担重建）。

### AD-15（P3）`_switch_effort_to_max` 修改共享 `catalog`，影响会话内其他成员

- 证据：`runners.py:538-553` 直接 `self.catalog.models[self.agent.model_profile] = profile.model_copy(...)`；`session.py:110` 的 `catalog` 是**整个会话共享**的 `UserConfig`（`open_session(catalog=...)` 由 CLI/TUI 传入同一对象）。
- 影响：某成员因拒斥回退 `max` 会顺带改写同一 profile 下其他成员的配置（同 profile 多成员时行为漂移）；回退本身只做一次（`_effort_fallback_used`，`runners.py:506-511`）✅ 无无限重试。
- 建议：把成员级 override 存到 runner 自身（`model_override` 或 profiles 深拷贝），或只改 `catalog.models` 的**副本**。

### AD-16（P3）`session_id` 未做路径字符校验（自伤面）

- 证据：`session.py:51-54` `base = sessions_dir() / session_id`；`sessions.py:161-210` 同样用 `root / session_id`。`--resume`/`--session` 由本地用户输入，Agent id 有白名单（`models.py:187-192`）而 session_id 没有。
- 影响：`delete_session("../../x")`/`archive_session("../../x")` 可越过 `sessions/` 操作外部目录；`shutil.rmtree`/`shutil.move` 直接生效。仅限本机用户自伤（非提权），但违反「路径检查需要覆盖路径穿越」（§12.2）的防御性原则，也可能被未来的非交互入口误用。
- 建议：`session_id` 校验 `^[A-Za-z0-9._-]+$`（禁 `..`、`/`）并在 CLI/TUI 入口统一执行。

## 四、与方案/文档的偏差（A 类）

| 基准 | 声称 | 实现 | 结论 |
|---|---|---|---|
| D-4 | 「每个回合通过 `turn/start` 的 `effort` 参数显式传入」 | `codex.py:262-263` ✅ | 一致 |
| D-4 | 「每回合显式传 `sandbox`/`approvalPolicy`/`approvalsReviewer=user`，不沿用本机 `auto_review`」 | `sandbox` 只在 `thread/start`（`codex.py:231-235`）；恢复路径不重传 | **偏差（AD-8）** |
| D-4 | 成员级可用 TeamSpec/用户配置覆盖 model 与工作目录 | `session.py:139-147` 把 profile 映射成 `-c` overrides ✅，工作目录来自 `workspace_policy` ✅ | 一致 |
| D-8 | effort 被拒后回退 `max`，**仅一次** | `codex.py:270-276`（`effort_fallback_used` 守卫）、`runners.py:503-522` ✅ | 一致（但见 AD-15 的共享 catalog 副作用） |
| D-9 | 删除前检查「会话被其他进程持有（文件锁）」 | `sessions.py:192-193` ✅ | 一致，但**同进程**锁泄漏会造成误报（AD-2） |
| D-9 | 删除前检查「成员 worktree 有未提交/未合并成果 → 拒绝」 | 仅 worktree；isolated 无保护（`sessions.py:140-149`） | **偏差（AD-9）** |
| §10.2 | 「断线后先核对线程历史及活动回合…无法确认时进入结果不明状态」 | 进程死亡时挂起至 900s 超时（AD-4）；重启后 `WAITING_APPROVAL` 不核对（AD-7） | **偏差** |
| §10.2 | 「持久化 `thread_id` 后再提交回合，并记录提交意图」 | `codex.py:240-241` 先 `set_codex_thread` 再 `turn/start` ✅；turn id 提交后 `set_run_external_turn`（`codex.py:286-288`）✅ | 一致（除 AD-4 之外：进程外死亡时该记录无法兑现） |
| §10.2 | 「取消必须等待 interrupted/terminal 状态…不能因发出取消请求就立即认定停止」 | `codex.py:408-426` ✅（含超时 → `OUTCOME_UNKNOWN`）；`tests/...cancel_waits_for_confirmed_stop` 覆盖 | 一致 |
| §10.2 | 「Codex 不注入 `assign_task`/`send_message`/拓扑工具」 | CodexRunner 不注册团队工具；`_render_input` 显式告知（`codex.py:322-325`）；控制面另有「Codex 成员只能由 Leader 委派」（`control.py:186-188`） | 一致（`test_codex_member_input_has_no_team_management_tools` 佐证） |
| §12.3 | 「原目录存在未提交任务输入时，默认选择共享模式并解释原因，不能悄悄忽略」 | `workspace.py:67-70` ✅（测试覆盖） | 一致 |
| §12.3 | 「存在未合并成果…的目录不能自动删除」 | worktree ✅（`workspace.py:82-110`）；isolated ❌（AD-9） | 部分偏差 |
| §12.3 | 「从明确提交建立成员分支与 worktree，记录基线、分支和目录」 | `workspace.py:71-79` 记录 branch/base_commit，但 `Workspace` 对象**不落库**（`prepare()` 返回值只被 `session.py:80-85` 取 `path`），重启后无从得知基线 | **偏差（并入 AD-1 建议：需要持久化 workspace 记录）** |
| §16 P5 完成条件 | 「混合团队闭环、Codex 异常恢复、合并冲突和成果保留通过」 | Codex 异常恢复见 AD-4/AD-7；worktree 复用见 AD-1 | **完成条件未满足** |
| §16 P6 完成条件 | 「…键盘与恢复流程通过」 | 会话恢复的 id/锁问题见 AD-1/2/3 | 部分未满足 |
| §17 T24 | 「同一会话继续使用原成员上下文；新会话隔离」 | Codex thread 复用 ✅（DB 持久化）；worktree 成员会话无法二次打开 ❌（AD-1）；id 复用导致新会话与归档会话同名（AD-3） | 部分未满足 |

## 五、未验证/存疑项

1. ~~**AD-7 的「补批批准 → 新 Codex 回合重放」**~~ → **已实测确认**：`repro_orphan_approval.py` 追加批准决策后，run 离开 `WAITING_APPROVAL`、重放 Codex 回合并再次发起批准请求（见 AD-7 第 3 条输出）。
2. **AD-8 的真实 schema**：本沙箱无 `codex` CLI（`which codex` 为空），`turn/start` 是否接受 `sandbox`、resumed thread 是否沿用本机 sandbox，均无法实测。**待验证**（建议用 `tests/test_p5_live_codex.py` 补断言）。
3. **`runners.py` 的 interrupt/resume 与工具调用配对**：`awrap_model_call` 抛 `TurnLimitExceeded`（`runners.py:266-271`）时，检查点里可能留下「有 tool_calls 无 ToolMessage」的 AIMessage，下一回合同一 thread 复用时行为未验证；`awrap_tool_call` 的 `interrupt()`（`runners.py:314`）在 resume 后 `recheck` 通过再执行 ✅（读码），但「步骤上限打断后重启」这条路径没有测试。**待验证**（P2 候选）。
4. **effort 回退重试的输入重复**：`runners.py:503-522` 重试时复用同一个 `graph_input`（`{"messages": [HumanMessage(...)]}`），首轮失败前该消息可能已被检查点提交，重试会再追加一次 → 成员在同一 thread 看到两遍输入。需要构造 `reasoning_effort` 拒斥的脚本化模型才能确认。**待验证**（与 AD-5 同类，P2 候选）。
5. **`sessions.is_session_locked` 的 TOCTOU**：`archive_session`/`delete_session` 先判锁再 `shutil.move/rmtree`（`sessions.py:167-178`、`192-210`），另一进程恰在窗口内 acquire 时会得到已移走的目录 fd（数据分裂）。窗口极小，**待验证**（P3）。
6. **`_completion_blockers` 对孤儿批准的长期影响**：仅在「重启后补做批准」路径上确认了 blocker（`signal_done` 因 pending approval 被拒是设计意图）；孤批准永不 `EXPIRED` 是读码结论（`grep` 显示只有 `consume_once` 会 `expire_approval`）。**待验证**是否还有别的清理路径。

## 六、自检（实际运行过的命令与结果）

| 命令 | 结果 |
|---|---|
| `.venv/bin/python -m pytest tests/test_p5_codex_adapter.py -q` | `5 passed in 2.22s` |
| `.venv/bin/python review/tmp/repro_lock_leak.py` | 失败 open 后 `locked=True`、fd 6 指向 `session.lock`、重试 `SessionInUse`（AD-2 确认） |
| `.venv/bin/python review/tmp/repro_worktree_reopen.py` | 2/3 次 `prepare` 均 `WorkspaceError`（AD-1 确认） |
| `.venv/bin/python review/tmp/repro_worktree_open_session.py` | 首次 `open_session` OK、第二次 `WorkspaceError`（AD-1 端到端确认） |
| `.venv/bin/python review/tmp/repro_session_id_collision_a.py` | 归档后 `new_session_id` 复用同 id；`list_sessions` 两条同 id（AD-3 确认） |
| `.venv/bin/python review/tmp/repro_codex_process_death.py` | 子进程 SIGKILL 后 5s：`_turn_done` 未决、状态仍 RUNNING（AD-4 确认） |
| `.venv/bin/python review/tmp/repro_codex_midturn_dup.py` | 第二回合 prompt 中同一条消息出现 2 次、`<queued_updates>` 存在（AD-5 确认） |
| `.venv/bin/python review/tmp/repro_codex_progress_dup.py` | `reply_text='fake reply fake reply'`、进度事件重复（AD-6 确认） |
| `.venv/bin/python review/tmp/repro_orphan_approval.py` | 重启后 run 仍 `WAITING_APPROVAL`、批准仍 `PENDING`；追加批准决策后 run 重放且再次请求批准（AD-7 确认） |
| `.venv/bin/python review/tmp/repro_list_sessions_robust.py` | 断链符号链接**不会**使 `list_sessions` 崩溃（反证 AD-13 的强版本） |
| `which codex` | 空 → 无法对齐真实 app-server schema（AD-8 只能读码判断） |
| 只读约束 | 仓库非 git 工作区（`git status` 报 not a repository），改用 mtime 判定：只有 `review/**` 由我写入，`src/**`、`tests/**`、`docs/**` 无我的改动（`runners.py` 的 11:11 改动来自外部） |
| `.venv/bin/python -m pytest tests/test_p5_codex_adapter.py tests/test_p5_workspace.py tests/test_p6_sessions_ui.py tests/test_p2_recovery.py -q` | `20 passed in 10.37s`（11:11 修订后复跑） |

## 七、给 Leader 的修复优先级建议

1. 先修 AD-2（锁释放）与 AD-1（worktree 复用）——两者叠加会让 `git_worktree` 会话在第一次重启后彻底不可用，且是同一条 `open_session` 路径上的问题。
2. AD-3（id 复用）需要产品决策：是「归档 id 永不复用」还是「TUI 行键改名」；前者更符合 §5.1 的会话身份语义。
3. AD-4/AD-7 是 Codex 适配的核心恢复语义，建议与 `tests/test_p5_live_codex.py` 一起补故障注入用例（杀子进程、重启带未决批准）。
4. AD-5/AD-6 影响可读性与任务摘要正确性，改动面小，适合紧随其后。
