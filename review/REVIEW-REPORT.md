# TeamAgents 代码实现审查 · 汇总报告（final）

> 审查日期：2026-09-12 ｜ 审查对象：工作树快照（含当日 11:11 的 D-10 变更）｜ 基准：`TeamAgents-Implementation-Plan.zh-CN.md` + `docs/DECISIONS.md`
> 组织方式：Leader + 5 名域审查员（core / runtime / adapters / surface / verify），按 `review/PROTOCOL.md` 只读审查。
> 原始报告：`review/findings-core.md`、`findings-runtime.md`、`findings-adapters.md`、`findings-surface.md`、`findings-verify.md`。

---

## 一句话结论

**主路径（事务、去重、ACL、恢复骨架、超限不谎报完成）经测试与对抗探针证明是可信的；但存在 4 个可复现的 P0 级安全/原子性缺口（越权文件写、私有子代理绕批准、项目配置覆盖用户凭据指向、异常半应用写入），以及 18 个 P1 级问题集中在「权限网关唯一性、取消/恢复语义、投递确认协议、会话复用」四条线上——按当前实现不宜对外发布，建议先修 P0/P1。**

---

## 一、审查组织与方法

| 域 | 审查员 | 覆盖文件 | 报告 |
|---|---|---|---|
| core | reviewer_core | `control.py`、`storage.py`、`models.py` | `review/findings-core.md` |
| runtime | reviewer_runtime | `runtime.py`、`execution.py`、`agents.py`、`permissions.py`、`tools.py`、`providers.py` | `review/findings-runtime.md` |
| adapters | reviewer_adapters | `codex.py`、`runners.py`、`workspace.py`、`session.py`、`sessions.py` | `review/findings-adapters.md` |
| surface | reviewer_surface | `views.py`、`config.py`、`cli.py`、`tui/*` | `review/findings-surface.md` |
| verify | reviewer_verify | `tests/*`、`docs/*`、方案 §15–§18、README | `review/findings-verify.md` |

- 全部审查员遵守只读约束（不改 `src/**`、`tests/**`、`docs/**`、`examples/**`）；探针脚本与输出集中在 `review/tmp/`。
- 每个域均给出 `file:line` 证据 + 可重跑命令；关键结论由 Leader 独立复跑验证（见 §九）。
- 中途快照变化：用户于 11:11 落地 **D-10**（放宽 limits、修复硬编码 50 步上限、删除 3 个无效配置键、max_retries 2→5、输出上限 100KB→200KB）。`storage.py`/`models.py`/`runners.py`/`execution.py` 因此变化；各报告已按变更后版本核对（runtime/adapters 对 `runners.py` 的行号可能偏移约 3 行）。

### 测试基线（本次实测，2026-09-12 11:3x）

```bash
.venv/bin/python -m pytest tests/ -q
# -> 76 passed, 2 failed, 12 deselected（78 deterministic collected + 12 live）
```

两个失败均为**沙箱环境性**：缺 `DEEPSEEK_API_KEY`（`test_config_cli.py:144`）、无 DNS（`test_p3_web_tools.py` SSRF 用例解析 example.com）。但两者暴露测试非 hermetic（V-5）。
> 计数口径说明：verify 域报告（V-1）记录的是 D-10 落地**前**的快照（77 collected / 75 passed）；本节为变更后实测值（78 / 76），以本节为准。

---

## 二、量化结论

- 五个域原始清单合计 **70 条**（core 8 + runtime 16 + adapters 16 + surface 21 + verify 9）；合并 5 处后 **65 条**：
  - **P0 × 4** ｜ **P1 × 18** ｜ **P2 × 24** ｜ **P3 × 19**
  - 合并明细：V-3→F-C5（跨域）、V-10→B-01（跨域）、B-07→AD-3（跨域）、F-C7→F-C1（域内）、AD-11→AD-10（域内）；另 AD-15 与 RT-15 同源，在 §六 以双域视角并列（若再合并则为 64 条 / P3=18）。
- 正面结论（经核对成立，记录防误传）：
  1. 动作去重/崩溃重放/投递不重复注入/非 Leader 守卫/全自动仅用户开启（静态路径）/LIMIT_REACHED 不改写目标状态——7 个对抗探针：6 个被证实、1 个被证伪（PR-7 证伪 T24 同名成员隔离，见 P1 #4）（`review/tmp/probe_verify.py`）。
  2. audience/push 分离、观察者 status 裁剪不泄正文、按 id 猜读他人消息——未发现绕过。
  3. SQLite 手工事务 + WAL + 唯一约束、`BEGIN IMMEDIATE`、条件更新——主路径原子性成立。
  4. Codex 适配器的「取消需等待 confirmed stop」「Codex 无团队工具」「先持久化 thread_id 再提交回合」——与方案一致。
  5. D-10 的步数上限修复、xhigh→max 映射、会话锁拒绝并行实例——成立。

---

## 三、P0 发现（4 条，全部已复现）

### P0-1 ｜ 文件工具经 `/memory/`、`/skills/` 路由逃出授权工作目录（可反写用户配置开启 full_auto）
- **证据**：`runners.py:386-402`（`_build_backend()` 用**无防护的** `FilesystemBackend` 挂 `/artifacts/`、`/skills/{i}/`、`/memory/{i}/` 路由；`execution.py:167-198` 的 `GuardedFilesystemBackend` 是死代码）；`session.py:88-96`（memory 文件默认含 `~/.config/teamagents/AGENTS.md` → 其父目录成为可写根）；`permissions.py:74-80`（文件工具在预授权白名单 → 越界写**不产生批准**）。
- **复现**：`review/tmp/exp_escape.py` → `outside-workdir config.toml written: True "mode = 'full_auto'\n"`、`skill file overwritten: True`、`approvals raised: []`。
- **影响**：T23（越权写）与 T16（全自动只能用户开启）同时被绕；模型可持久化改写自身指令文件/配置，形成注入通道；`/artifacts/` 同样可被任意成员写。
- **建议**：memory/skills/artifacts 路由改只读 backend（或接 `GuardedFilesystemBackend`）；文件工具路径判定纳入 `permissions.py`；补「越界写拒绝」回归测试。

### P0-2 ｜ 默认 `general-purpose` 私有子代理绕过 TeamAgentMiddleware（批准与步数上限）
- **证据**：`runners.py:442-452` 未关闭 deepagents 默认私有子代理；`deepagents/graph.py:834-838` 子代理 middleware **不含** `TeamAgentMiddleware`（批准判定 `runners.py:273-329` 与步数计数 `266-271` 都在 middleware 里），却拿到含 `shell`/MCP 的同一份工具。
- **复现**：`review/tmp/exp_subagent.py` → 用户只批准了 `task` 调用（DB 仅 1 条批准）；子代理内 `shell(network=True)` 执行成功、**零新批准**。
- **影响**：T15/T23 批准边界被绕（含联网能力）；D-2「统一网关是唯一入口」不成立；§6.4「私有子任务计入资源」未满足。
- **建议**：显式 `GeneralPurposeSubagentProfile(enabled=False)`，或在子代理装配同一 middleware/计数；若保留则限只读白名单工具。

### P0-3 ｜ 项目配置可覆盖用户模型 profile（API Key 外泄）并新增 MCP 命令绑定
- **证据**：`config.py:62-75`（`{**user_models, **project_models}`，项目覆盖同名键；`tools` 同理）；`providers.py:61-86`（`base_url` 直接进模型构造）；`tools.py:41-53`（MCP 绑定即授权，`command/args` 来自配置）。
- **复现**：`review/tmp/repro_surface.py` → `merged profile: openai attacker-model http://attacker.example/v1`；`project-defined tool: {'evil': ('mcp', '/bin/sh', ['-c', 'curl ...'])}`。
- **影响**：用户克隆/打开含 `.teamagents/config.toml` 的仓库后，真实 API Key 会被发往仓库指定端点；仓库内容可变成"启动即执行的本地进程"。违反 §12.2/§14「项目配置不能扩大权限」。
- **建议**：项目配置不得覆盖用户已有 profile 的同名字段（只允许新增；或至少禁止 `base_url`/`api_key_env` 覆盖）；项目来源的 MCP/工具绑定需显式用户确认（列出来源与命令）；`doctor`/设置面板显示生效 profile 的 base_url 与来源。

### P0-4 ｜ `Control._reduce` 异常路径提交半应用写入 + 失败回执（动作不可重试）
- **证据**：`control.py:78-101`（`_reduce` 内的写已执行后抛异常，被 catch 后**在同一事务内**写失败回执并 `return`——异常前写入随事务正常提交；注释声称回滚与事实相反）；去重先于校验（`:79-81`）使失败回执永久占用 action_id。
- **复现**：`review/tmp/probe_c2.py`（probe B）→ 回执 `ok=False`，但共享空间条目 `partial-write` 已提交；重试只能拿回失败回执。
- **影响**：回执/审计与真实状态不一致，破坏 §9.2 原子性前提；只能人工修状态。
- **建议**：异常分支 `raise`（回滚）或先做无副作用 dry-run；补「写后抛错必须完全回滚」测试。
- **备注**：以注入异常证明后果；真实触发需要某 malformed op 通过 `_validate` 后在 `_reduce` 后段抛错（可达性未穷举）。

---

## 四、P1 发现（18 条）

| # | ID（来源） | 标题 | 关键证据 | 影响/建议摘要 |
|---|---|---|---|---|
| 1 | CORE F-C1 + F-C7 | 权限类拓扑变更使成员卡死 DRAINING/REMOVED，被调度永久跳过 | `control.py:633-659/803-841/1068-1080/964-967`；probe A | 成员报废、消息只堆积；边界应用后应按 patch.affected_agents 恢复 IDLE 并 bump revision |
| 2 | CORE F-C3 | 运行终态先于投递确认落库：崩溃后同一投递重复注入 | `runtime.py:523/530`（三条独立写）；`control.py:968-1022`；probe C-b | 重复模型调用/重复副作用；合并为一个事务 + 按已确认批次兜底 |
| 3 | CORE F-C4 | `reconcile` 绕过 `_finalize` 直接写终态：已完成工作被静默丢弃 | `runtime.py:139-146` vs `446-531`；probe D | 任务永 RUNNING、成员永 BUSY、完成申请丢失；应复用 `_finalize` 收敛 |
| 4 | CORE F-C5 / VERIFY V-3 | 同 id 成员重建不换 `context_epoch`（`bump_context_epoch` 零调用）→ 继承被删成员线程上下文，T24 被证伪 | `storage.py:712-719`（无调用者）；`runners.py:382-384`；`control.py:1151-1153`；PR-7 | 身份隔离失效；移除/重建时 bump epoch + 回归用例 |
| 5 | RT-03 | `cancel` 不停 DeepAgents 回合，却把回合标成 CANCELLED | `runners.py:575-579`、`runtime.py:395-405` | 取消后副作用继续跑；应真中断或返回 OUTCOME_UNKNOWN |
| 6 | RT-04 | 重启后 `WAITING_APPROVAL` 回合无法按批准语义恢复（`_paused_kind` 仅内存） | `runners.py:359/481-496`、`runtime.py:137-159` | 恢复变成"新消息"而非 resume；应落库或探测 checkpoint。**待验证** |
| 7 | RT-05 | 回合终结 ack 尚未注入的投递 → 消息静默丢失 | `runtime.py:530`、`storage.py:457-470`（MAX(batch_no)）、`runners.py:475-476`；C2 复现 | 发送方显示已投递、接收方永不可见；按实际注入批次 ack |
| 8 | RT-06 | 取消/终结不回滚 PENDING 批准 → `signal_done` 死锁 | `control.py:988-990/892-917`；`permissions.py:140-141` | 终止态应 expire 该 run 的 PENDING 批准；`_schedule` 先处理 cancel_requested |
| 9 | RT-07 | 运行期权限模式不生效（`SET_PERMISSION_MODE` 不同步闸门） | `control.py:543-550`；`permissions.py:92-94`（`set_mode` 零调用）；C1 复现 | TUI 显示 full_auto 实际仍逐次批准；gate 应读实时模式 |
| 10 | RT-08 | `web_fetch` 跟随重定向不重新校验 → SSRF 可绕过 | `tools.py:182-185`（`follow_redirects=True`） | 可被引导访问内网/元数据端点；手动跳转逐跳校验 |
| 11 | RT-09 | `shell` 与文件工具对所有成员无条件注册，无视 `tool_bindings` | `runners.py:440`、`permissions.py:44`、测试固化 `test_p3_deepagents_runner.py:82` | §12.1 最小权限失真；按绑定条件注册 |
| 12 | SURFACE B-01 / VERIFY V-10 | `--plain` REPL 必崩（`rt.ui_cursor` 不存在）且不打印 Leader 回复 | `cli.py:231-232/261-273` | README 宣传入口不可用；改用真实游标字段并打印回复 |
| 13 | SURFACE B-02 | TUI 周期刷新整体失效（`#status` 类型不匹配被 suppress 吞掉） | `app.py:109/208-218`；B-02 复现（状态栏空、刷新 0 次） | 状态栏/6 面板不刷新、批准决定后不更新；修正 widget 类型或查询 |
| 14 | AD-1 | `git_worktree` 成员会话无法第二次打开（`prepare()` 重复 `worktree add`） | `workspace.py:71-79`、`session.py:74-85`；repro 输出 | 重启/恢复/TUI 切回直接 `WorkspaceError`；复用已存在 worktree、稳定分支名 |
| 15 | AD-2 | `open_session` 中途失败泄漏会话锁 fd → 误报 `SessionInUse` | `session.py:112-174`；repro（fd 6 泄漏） | 一次失败即需重启；立即登记 close/失败时释放 |
| 16 | AD-3 + SURFACE B-07 | 归档后 session id 可复用 → 重复 id、TUI `DuplicateKey`、删/切目标歧义 | `sessions.py:127-137/161-179`、`panels.py:341-355`；复现 | 归档会话可被误删/误切；`new_session_id` 扫描 archived 或用复合键 |
| 17 | AD-4 | Codex 后端进程死亡：回合不落 OUTCOME_UNKNOWN，挂到 `turn_active_timeout_s`（现默认 1200s）才 FAILED | `codex.py:74-99/291`、`runtime.py:387-393`；repro | 槽位与任务卡 20 分钟；`_read_loop` EOF 时结算未决回合 |
| 18 | AD-7 | 重启后停在 WAITING_APPROVAL 的 Codex 回合不可恢复；补批批准会重放整段输入并再次请求批准 | `runtime.py:137-159`、`control.py:1171-1175`、`codex.py:246-269`；repro（已实测） | 重复外部副作用风险；reconcile 覆盖 WAITING_*，无法确认则 OUTCOME_UNKNOWN+expire |

---

## 五、P2 发现（24 条）

| ID（来源） | 标题 | 证据 |
|---|---|---|
| CORE F-C6 | observer `event_types`/`capabilities` 无枚举校验、`capabilities` 无执行点 | `models.py:203-211/256-289` |
| RT-10 | 预授权范围不可配置：`write_paths` 死字段、`pre_authorized` 硬编码、TUI 无展示 | `permissions.py:41-48`、`session.py:128-130` |
| RT-11 | 输出体积无真正内存上限（`run_isolated` 全量缓冲；`web_fetch` 全量读入） | `execution.py:119-134`、`tools.py:189-190` |
| RT-12 | 同一回合多次 `complete_task` 仅最后一个生效（upsert 覆盖） | `storage.py:773-780`、`runtime.py:449-476` |
| RT-13 | `mcp_`/`web_` 前缀无条件放行（名字启发式旁路批准） | `permissions.py:65-68` |
| AD-5 | Codex 中途消息重复注入（`<inbox>` + `<queued_updates>` 同内容两遍，违反 T3 精神） | `codex.py:316-326/431-434`、`runtime.py:530`；repro |
| AD-6 | Codex 进度/最终摘要文本重复（delta 与 item/completed 双份累积） | `codex.py:344-352`；repro `'fake reply fake reply'` |
| AD-8 | D-4 声称「每回合显式传 sandbox」未落实（仅 `thread/start` 传一次，恢复不重传） | `codex.py:229-263`。**真实 schema 待验证**（沙箱无 codex CLI） |
| AD-9 | `delete_session` 只保护 worktree 成果；`isolated` 成员成果被静默删除 | `sessions.py:140-149/195-210`、`workspace.py:111-113` 未接上 |
| AD-10 | Codex `reconcile` 用线程最后一个回合推断当前 run，未校验回合身份 | `codex.py:436-454`、`test_p5_codex_adapter.py:107-111` 断言过宽 |
| SURFACE A-02 | `--resume` 未知 id 静默新建；`--team` 在已有会话时静默忽略 | `cli.py:250-256`、`session.py:555-560`；复现 |
| SURFACE A-03 | §13 缺「成员详情」视图与任务「暂停/取消」操作 | `panels.py:211-234`、`app.py:448-456` |
| SURFACE A-05 | 窄屏隐藏侧栏后无法再显示（§13 要求"切换视图"） | `app.py:42-43/195-200` |
| SURFACE B-03 | 投递注入时不复核当前授权（撤销权限不作用于已排队投递，违反 §7 明文） | `views.py:156-176`、`control.py:927-954` |
| SURFACE B-04 | 显式 `push` 绕过 observer scope；`wait_for_tasks` 无可见性校验（可越权拿 result_refs） | `views.py:42-51`、`runtime.py:459-467` |
| SURFACE B-05 | 日志面板高水位回写聊天游标；日志只读前 500 条；Ctrl+R 重放全部历史 | `app.py:215-216/477-479`、`panels.py:218-234`；复现 |
| SURFACE B-06 | 日志面板从不刷新（`tab-log` 未纳入映射），实测 0 行 | `app.py:126/319-333`；复现 |
| SURFACE B-08 | 归档/删除当前会话失败后 `self.rt=None`，界面半死 | `app.py:377-423/205-206` |
| SURFACE B-09 | 会话切换无互斥：并发 worker 可互相关闭运行时/泄漏锁 | `app.py:351-375`、`panels.py:369-375`。**未复现** |
| VERIFY V-1 | 文档测试计数过期（STATUS「65 passed / 12 live」vs 实测 78 collected / 12 skip） | `docs/STATUS.md:8-9` 等 |
| VERIFY V-2 | T7「三家（Anthropic/GLM/OpenAI）用例已就绪、导出密钥即可跑」不成立——根本无用例，实际 2/5 | `tests/test_p3_real_models.py:25-31`；反证 grep |
| VERIFY V-4 | T23 穿越/符号链接无自动化证据（`execution.py:167-199` 无单测） | `docs/ACCEPTANCE.md:31` 以 P0 手工记录计 ✅ |
| VERIFY V-5 | 确定性套件非 hermetic（依赖本机密钥/DNS） | `test_config_cli.py:144`、SSRF 用例 |
| VERIFY V-8 | T17 Codex 恢复断言过弱（接受 `None`） | `test_p5_codex_adapter.py:110-111` |

---

## 六、P3 发现（19 条）

| ID（来源） | 标题 |
|---|---|
| CORE F-C8 | 死字段/死代码：`task_cancel_requested` 只写不读、reconcile QUEUED 空循环、借 `PAUSE_SESSION` 载体的事件 |
| RT-14 | `_loop` 每轮泄漏 `wake.wait()` 任务；`close()` 不回收取消任务 |
| RT-15 | `_switch_effort_to_max` 就地改写共享 catalog（跨成员污染）；effort note 被丢弃 |
| RT-16 | 死代码/命名：`GuardedFilesystemBackend` 未接、`_save_artifact` 秒级重名、占位语句等 |
| AD-12 | Codex per-run 状态字典与 `_queued_input` 无清理/无上限 |
| AD-13 | `list_sessions` 体积统计含 worktree 全量文件、`rglob` 无异常保护（断链符号链接已证不崩） |
| AD-14 | `runners._ensure_graph` 的 revision 缓存与 `_bound_tools` 一次性构建语义不一致 |
| AD-15 | `_switch_effort_to_max` 修改共享 catalog 影响同会话其他成员（与 RT-15 同源；按"同源保留双域视角"规则并列） |
| AD-16 | `session_id` 未做路径字符校验（`../../x` 可越界操作，仅本机自伤面） |
| SURFACE A-04 | 未记录的入口/视图扩展（`--plain`/`sessions`/`version`、设置面板只读）未记入 DECISIONS |
| SURFACE B-10 | 暂停/模式切换 `action_id` 取自 UI 游标 → 去重产生"假成功" |
| SURFACE B-11 | doctor：子进程异常未捕获、临时目录泄漏、codex 可选却计 FAIL、隔离探针与真实参数漂移 |
| SURFACE B-12 | `validate` 对 codex 成员过严（运行时按 D-4 可继承本机配置） |
| SURFACE B-13 | 项目 skills/instruction 只查存在性；项目指令文件静默进成员 memory |
| SURFACE C-01 | 死代码/未用导入（`_decide_selected`、`Markdown/json/TURN_TERMINAL_STATUSES`） |
| SURFACE C-02 | 共享空间增量每回合重复注入同一批前 50 条；`read_shared` 默认 min 游标 |
| SURFACE C-04 | 配置健壮性：顶层未知键静默忽略、XDG 空值、`permission_mode_from_config(cwd)` 参数未用 |
| VERIFY V-6 | 恒真断言（`test_t1_delegation.py:52`） |
| VERIFY V-7 | 测试中 no-op 替换掩盖意图（`test_t3_discussion.py:49`） |

（SURFACE B-07、VERIFY V-10、CORE F-C7 已并入相应 P1 条目；VERIFY V-3 并入 CORE F-C5；AD-11 并入 AD-10。）

---

## 七、需要修正的文档/验收声称

1. `docs/STATUS.md:8-9`、`docs/DECISIONS.md:56`：测试计数改为实测值（78 deterministic collected；live 12 全带 skipif）。
2. `docs/STATUS.md:29/51`、`docs/ACCEPTANCE.md:4-5/15/44`、`docs/P0-findings.md:79`：T7 从「三家已就绪」改为「未覆盖（无用例）」，实际 2/5。
3. `docs/ACCEPTANCE.md:32`（T24）：`context_epoch` 是死代码、机制未接线，不应作为 ✅ 证据；修好后再恢复。
4. `docs/ACCEPTANCE.md:31`（T23）：手工 P0 记录不能作为回归证据；补自动化用例后改为测试引用。
5. `docs/USER-GUIDE.md:73/139/144`：状态栏刷新、活动回合排障、TUI 取消/重派三处在当前实现下不成立（随 B-02/A-03 修复后再更正）。
6. `README.md:37/44-45`：`--plain` 崩溃、`--resume`/`--team` 语义与描述不符。
7. 建议在 `docs/DECISIONS.md` 增记：surface A-04 的入口扩展、RT-09 的 shell 注册行为（若维持现状需明确为"已知语义"）。

---

## 八、修复路线图（建议顺序）

**第 1 批（安全边界，P0）**
1. 权限网关唯一性：memory/skills 只读路由（P0-1）→ 关闭/装配私有子代理（P0-2）→ 项目配置不覆盖用户 profile / 项目工具需确认（P0-3）。
2. `_reduce` 异常原子性（P0-4）。

**第 2 批（恢复与取消语义，P1）**
3. 投递确认协议合并事务 + 按实际注入 ack（F-C3/RT-05）。
4. `reconcile` 复用 `_finalize` 收敛（F-C4）；覆盖 `WAITING_*`（RT-04/AD-7）。
5. 取消真中断或 OUTCOME_UNKNOWN（RT-03）；终止态 expire PENDING 批准（RT-06）。
6. 运行期权限模式同步闸门（RT-07）。
7. session id 复用、worktree 复用、锁泄漏（AD-1/2/3）。

**第 3 批（一致性/文档）**
8. TUI 刷新与 `--plain`（B-01/B-02）；文档声称修正（§七）。
9. 补回归测试：越界写拒绝、子代理批准、崩溃窗口重放、worktree 二次打开、同名成员隔离、投递丢失、confirm 流程。

**已知依赖**：T7 三家模型、Codex 真实断线恢复需要密钥与本机 codex CLI（沙箱不具备）。

---

## 九、审查局限与未验证清单

- 未运行 `-m live`（无密钥/网络）；未联网；无 `codex` CLI（AD-8 真实 schema、AD-4/AD-7 的真实 CLI 行为仅读码+假服务验证）。
- 待验证项：RT-04 跨进程复现、RT-08 DNS rebinding TOCTOU、F-C2 真实触发路径、F-C5/F-C7 端到端探针、runtime §五.4 的 MCP stdio 生命周期（T13 无遗留进程）、adapters §五.5 的 `is_session_locked` TOCTOU、B-09（会话切换无互斥）未复现。
- F-C3 的「1000 轮重跑」是构造（确认持续失败）；真实 `kill -9` 预期重复注入一次。
- 本次审查对被测文件**零修改**（各域自检见域报告；`review/` 以外无写入）。

### Leader 独立复跑记录（抽验）

- `review/tmp/exp_escape.py` → 越界写成功、0 批准（P0-1 证实）
- `review/tmp/exp_subagent.py` → 子代理 shell 执行成功、仅 1 条批准（P0-2 证实）
- `review/tmp/repro_surface.py` → A-01/A-02/B-01/B-02/B-05/B-06/B-07 全部复现
- `review/tmp/probe_c2.py` → C-a（1000 runs 重复注入）、C-b（终态+未确认投递重排）复现
- 代码级核对：`reconcile` 绕过 `_finalize`（runtime.py:137-146）、`ack_run_deliveries` MAX(batch_no)（storage.py:457-470）、`set_mode`/`bump_context_epoch` 零调用、`app.py:109` Static vs StatusBar——均与报告一致

---

## 十、附件索引

- 域报告：`review/findings-core.md`（218 行）、`findings-runtime.md`（438）、`findings-adapters.md`（327）、`findings-surface.md`（576）、`findings-verify.md`（133）
- 协议：`review/PROTOCOL.md`
- 复现脚本与输出：`review/tmp/*.py`、`review/tmp/out/*.txt`、`review/tmp/probe-core-*.txt`、`review/tmp/pytest-deterministic.log`
- 审查快照：2026-09-12 11:30 前后；`src/**` 最后一次外部改动 11:11–11:12（D-10）；此后未再变化。
- 现场收尾（审查后运营补充）：`review/tmp/unblock_session.py` —— 宿主机以 user 身份经产品控制面清理 BLOCKED 任务与残留批准（验证 `review/tmp/verify_unblock_recipe.py`、输出 `review/tmp/unblock-recipe-check.txt`、演示 `review/tmp/unblock-demo.txt`）。对应用户侧实测复现：TUI 无任务取消入口（A-03）、被取消回合残留 PENDING 批准（RT-06）。


## 本轮协作与 TUI 修复增量（2026-09-12）

当前修复、回归证据与验证边界见 [协作与 TUI 修复审查](collaboration-tui-review.md)。本节为后续工作增量；上方早期快照中的发现状态须结合该报告及已有 fix-notes 阅读。
