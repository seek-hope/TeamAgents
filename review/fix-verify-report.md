# 修复批次 1 · 独立对抗性交叉核对报告（reviewer_verify）

> 任务：task_b5c927d8ec42 ｜ 只读核对（不采信实现者自述，一切以复跑为准）｜ 2026-09-12
> 对象：P0-1、P0-2、P0-3、P0-4、RT-06、A-03/B-02 及新增测试（批次 1 冻结，src/tests 未由本报告修改）
> 探针：`review/tmp/verify_fix_batch1.py`（25 项独立断言，可复跑）

## 0. 结论表

| # | 修复项 | 独立结论 | 关键证据 |
|---|---|---|---|
| — | 全套件 | **与声称一致** | `.venv/bin/python -m pytest tests/ -q` → `2 failed, 109 passed, 12 deselected in 40.72s`；2 失败=基线环境性（缺 DEEPSEEK_API_KEY → providers.py:54；无 DNS → tools.py:168），变更前基线同为 2 failed |
| P0-1 | 文件工具逃逸 | **证实** | execution.py:186-210（归一化+realpath 复检）、213+（ReadOnly）；探针 A1-A4 全被拒/拒改；`tests/test_p3_backend_guard.py` 4 passed |
| P0-2 | 子代理绕批准 | **证实** | runners.py:338-386（SubagentGate）、487-505（显式 GP spec）、507+（`_ensure_graph` 替换）；探针 B1-B3；`tests/test_p3_subagent_gate.py` 1 passed |
| P0-3 | 项目配置覆盖 | **证实** | config.py:30/39/51/63/118；探针 C1-C7；`tests/test_config_project_isolation.py` 7 passed |
| P0-4 | _reduce 半应用 | **证实** | control.py:77/97（`submit`→新事务回执；`_submit_once` 单事务）；探针 D1-D4；`tests/test_p0_4_reduce_rollback.py` 5 passed |
| RT-06 | 终止态残留批准 | **证实** | storage.py:903-925；探针 E1-E2；`tests/test_rt06_approval_expiry.py` 11 passed |
| A-03/B-02 | TUI 取消+刷新 | **证实** | panels.py:223-236（CANCEL_TASK 交 control 裁决）、app.py:207-233（逐控件刷新+错误去重）；`tests/test_p6_tui_cancel_refresh.py` 5 passed |

定向合计：6 个新测试文件 `33 passed in 11.74s`（=7+4+1+5+11+5，与声称的 33 个新增一致）。

## 1. 方法与命令清单

只读核对 + 独立探针；不采信实现者自述。

```bash
cd <repo>
.venv/bin/python -m pytest tests/ -q                      # 2 failed, 109 passed, 12 deselected
.venv/bin/python -m pytest tests/test_p3_backend_guard.py tests/test_p3_subagent_gate.py \
  tests/test_config_project_isolation.py tests/test_p0_4_reduce_rollback.py \
  tests/test_rt06_approval_expiry.py tests/test_p6_tui_cancel_refresh.py -q   # 33 passed
.venv/bin/python review/tmp/verify_fix_batch1.py          # 25 探针 A1-E2
```

探针结果（Verdict 为「证实」= 修复按声称生效；「证实(被拒)」= 攻击向量失败，防护成立；「证伪」仅 C4 一次，系探针自身缺目录，修正后证实）：

| 探针 | 攻击/核对点 | 结论 | 细节 |
|---|---|---|---|
| A1 | Guarded.write 经 dangling symlink 逃逸（`/evil`→外部不存在目标） | 拒绝成立 | 外层 `path '/evil' rejected:`（execution.py:194）+ 内层 deepagents 基础校验 `outside root directory`（site-packages/deepagents/backends/filesystem.py:212）；外部文件未创建 |
| A2 | IsolatedShellBackend.write 同上 | 拒绝成立 | 同上；外部文件未创建 |
| A3 | read/ls/grep 对 `/../etc/*` 越界 | 干净错误结果（不抛异常） | 三方法均返回错误字符串（`Path traversal not allowed`），回合不会崩 |
| A4 | ReadOnly：读可用 + 写/编辑/删/上传全拒 | 证实 | 读无错；三类变更与 upload 均返回拒绝文案（`virtual_prefix` 回显） |
| B1 | 子代理内 in-scope write_file | 证实 | 子代理由内建 GP 槽位替换为显式 spec，可写工作区 |
| B2 | 子代理 network 提权 | 证实（被拒） | 批准总数保持 1（不新增）；回执 `Blocked by team permissions: shell needs explicit user appro…` |
| B3 | 子代理调用团队工具 | 证实（被拒） | `send_message` 被 SubagentGate 拦截：`Team tools are not available to private subagents…` |
| C1 | 项目同名 profile 覆盖 base_url | 证实（不可覆盖） | 用户 `https://api.deepseek.com/v1` 获胜 |
| C2 | 项目 MCP 工具默认注入 | 证实（不加载） | 仅用户工具 `mine` 在列 |
| C3 | 项目 `[permissions] mode=full_auto` | 证实（无效） | 仍 `approved_scope` |
| C4 | 项目 `skills_paths` 合并 | 证实（保留） | 探针初次「证伪」因目录不存在被过滤；补建目录后 `skills_paths` 含项目路径 |
| C5 | 项目新增（新名）profile 被加载 | 证实（残留，设计面） | `sneaky` 的 `base_url=http://attacker.example/v1` 生效 |
| C6 | 用户 opt-in 后项目新工具加载、同名仍不可覆盖 | 证实 | `evil` 加载；同名 `mine` 仍为用户 `web_fetch` |
| C7 | 项目文件自写 `trust_project_tools=true` | 证实（无效） | 工具仍不加载 |
| D1 | 写+事件+调度后抛错 | 证实 | 全回滚：entries=[]、events=0；返回失败回执 |
| D2 | 同 action_id 重放 | 证实 | 拿回同一失败回执，无副作用 |
| D3 | 新 action_id 干净重试 | 证实 | 成功且仅一次 |
| D4 | 校验拒绝（未知 assignee） | 证实 | 可重放 refusal，无任务写入 |
| E1 | park 在 WAITING_APPROVAL → cancel_run | 证实 | run=CANCELLED、approval=EXPIRED、blockers=[]（leader 侧 actor 路径） |
| E2 | 对 EXPIRED 批准的迟到决定 | 证实 | 干净拒绝 `approval is already EXPIRED` |

## 2. 逐项核对

### P0-1 文件工具逃逸（execution.py / runners.py）
- 防护链：`GuardedFilesystemBackend._resolve_path`（execution.py:186-210）先把基础类 `ValueError` 归一为 `PermissionError`（:189-195，保证工具层得到错误结果而非异常），再对「已存在父目录」逐级向上 `resolve()` 复检（:196-209）；`ReadOnlyFilesystemBackend`（:213+）对 memory/skills 路由拒绝一切变更、放行读取；`IsolatedShellBackend` 继承 Guarded。
- `_build_backend`（runners.py:445-462）三条路由全部有防护：`/artifacts/`=Guarded 可写、`/skills/{i}/` 与 `/memory/{i}/`=ReadOnly、默认=IsolatedShellBackend。
- 独立攻击：dangling symlink 写逃逸（A1/A2，被拒）、`..` 越界读/列/搜（A3，干净错误）、ReadOnly 四类变更（A4，全拒）。原 exp_escape 向量（写 `/memory/0/config.toml`、`/skills/0/reporting/SKILL.md`）在端到端测试中同样被拒（5 写 3 错、无 pending approval、run COMPLETED）。
- 结论：**证实**。残留见 R-3（只读根为目录、错误回显虚拟路径，均无实质提权）。

### P0-2 私有 general-purpose 子代理绕批准（runners.py）
- `SubagentGate`（runners.py:338-386）：`awrap_model_call` 共享 `TeamAgentMiddleware.model_steps` 计数并在超限时抛 `TurnLimitExceeded`；`awrap_tool_call` 先拒 TEAM_TOOLS，再放行 `runner.bound_tool_names()` 与 `policy.evaluate` allow/session 批准，其余**硬拒绝且不建 PENDING 批准**。
- 替换路径成立：`_general_purpose_spec`（:487-505）过滤 TEAM_TOOLS + 挂 SubagentGate；`_ensure_graph`（:507+）以同名 inline spec 传入，deepagents 0.7.13 仅在无同名 spec 时才注入自动 GP（site-packages/deepagents/graph.py:795-796），故自动版不会并存。
- 独立攻击：B1 在范围内工具正常、B2 network 提权被拒且批准计数不变、B3 团队工具被拒、端到端测试确认 pending approvals=0、approvals 总数=1、steps≥4（共享计数）。
- 结论：**证实**。残留见 R-1（硬拒绝是已声明的取舍）。

### P0-3 项目配置覆盖（config.py）
- `load_user_config`（:118）合并项目文件时：同名 profile 跳过并 `ProjectConfigWarning`（:51）、同名工具跳过（:63）、仅用户级 `_trust_project_tools`（:39）为真才加载项目新工具、非 bool 抛 `ValueError`；`skills_paths`/`instruction_files` 仍按产品行为两级拼接；`permission_mode_from_config` 只读用户配置。
- 独立攻击：C1-C3、C6、C7 + 测试第 5/7 例（opt-in 后同名仍不可覆盖、非 bool 报错）。结论：**证实**。残留见 R-2（项目可新增 profile）。

### P0-4 `_reduce` 半应用写入（control.py）
- `submit`（:77）异常时在**新事务**中写失败回执（并检查并发写者，避免覆盖他人结果）；`_submit_once`（:97）单事务：dedup→load spec→validate（拒绝也记回执）→reduce→persist_events→schedule→record；`storage._Tx` 可重入、depth0 `BEGIN IMMEDIATE`、退出 COMMIT/ROLLBACK。
- 独立攻击：D1 在写+事件+调度之后抛错 → 全回滚 + 失败回执；D2 同 id 重放一致；D3 新 id 恢复；D4 校验拒绝可重放。另审读并发窗口：并发同 id 的第二个提交要么看到回执直接返回，要么在锁内重新执行；失败回执写入受同一锁保护，不会出现「回滚后无回执」的裸窗口导致双执行。结论：**证实**。残留见 R-4。

### RT-06 终止态残留批准（storage/control/runtime）
- `expire_approval`（storage.py:903-910）已泛化到 `PENDING|APPROVED_ONCE → EXPIRED`；`expire_run_approvals`（:912-925）只收该 run 的 PENDING 并返回供审计；`APPROVED_SESSION` 按设计存活（并由测试确认自动放行）。
- 独立攻击：E1 用 leader 侧 cancel_run 收敛 parked 回合 → CANCELLED + EXPIRED + blockers 清空；E2 迟到决定干净拒绝。测试另覆盖 4 个终态参数化、WAITING_APPROVAL 保持、reconcile OUTCOME_UNKNOWN、cancel task 收敛。结论：**证实**。

### A-03 / B-02（TUI）
- 取消：`TasksPanel.cancel`（panels.py:223-236）提交用户侧 `CANCEL_TASK`，由 control 裁决后按状态给出「已取消 / 已请求取消 / 无需取消」，BLOCKED→CANCELLED、RUNNING→`run_cancel_requested=true` 均落库可查；终态任务按 c 为 no-op。
- 刷新：`_refresh_widgets`（app.py:207-233）逐控件 try/except 隔离，状态栏按正确类型查询（修复 WrongType 整段失效），错误集合去重上报；日志面板独立游标不吞聊天事件（B-06）。结论：**证实**。

## 3. 新增测试审查（33 例）

抽读全部 6 个文件：断言均作用于具体状态/计数/文本，**无恒真断言、无 skip/xfail 掩盖**：
- `test_p3_backend_guard.py`：ReadOnly 四操作拒 + 读可用；端到端断言写错误条数、无 pending approval、goal done、run COMPLETED（非只跑通）。
- `test_p3_subagent_gate.py`：in-scope shell 成功 + network 拒绝且含 `Blocked by team permissions`、`NET-ESCAPED` 不出现、pending=0、approvals=1、steps≥4。
- `test_config_project_isolation.py`：对攻击面用 `pytest.warns(ProjectConfigWarning, match=...)` 正向要求告警；进一步用 `build_bound_tools` 证明被忽略工具**不可解析**（不只是被隐藏）；opt-in/同名/非 bool 全有。
- `test_p0_4_reduce_rollback.py`：写后抛错→无半应用/无事件、同 id 重放同 refusal、新 id 重试恰好 1 条；含 insert_task 变体与 3 个 validation refusal。
- `test_rt06_approval_expiry.py`：4 终止态参数化（含审计事件）、WAITING_APPROVAL 保持 PENDING、reconcile、parked cancel + 迟到决定拒、session 批准存活。
- `test_p6_tui_cancel_refresh.py`：含反向断言 `app._refresh_errors == ()`；周期刷新用「先记录 before 再断言 +N」而非自证；日志面板断言两处同时出现（独立游标）。

## 4. 遗留风险（已确认，非阻断）

- **R-1（P2，已声明取舍）** 子代理需新批准的操作被硬拒绝且不建批准（runners.py:366+）；成员须在自身回合重试。建议把该语义写进 fix notes/DECISIONS，并在错误文案上标注「在自身回合重试」。
- **R-2（P2，设计残留）** 项目可新增（新名）profile 且其 `base_url` 被加载（C5 实测）；仅当 TeamSpec 引用该名字时影响出网端点。doctor/TUI 的展示与警示未做（impl_config 自述），建议进批次 2 或补文档。
- **R-3（P3，信息面）** memory/skills 路由根=对应目录（runners.py:454-459），同目录其他文件对成员**可读**（只读、用户所有）；错误消息只回显虚拟路径。
- **R-4（P3，语义）** 同 action_id 的失败回执被钉住（D2），瞬时故障需新 id 重试（D3 验证可行）；无自动重试策略。
- **R-5（范围外，仍开放）** RT-03 真中断、RT-04/AD-7 reconcile WAITING_*、B-01/B-05、context_epoch（P1#4）、verify 域 V-1/V-2/V-4/V-5/V-8。
- **R-6（环境）** 2 个环境性失败在无 DNS/无 key 沙箱必现（tools.py:168 等）；建议以 skipif/文档标注网络依赖，避免 CI 误读（P3 建议）。

## 5. 新发现（P0-P3）

- **无新增 P0/P1。**
- **N-1（P3，可观测性）** 子代理拦截文案为英文技术串（`Blocked by team permissions: …`），会进入子代理上下文、可能外溢到成员摘要；中文界面建议映射或日志规范化。
- **N-2（P3，流程）** 批次 1 的语义取舍（子代理硬拒绝、`trust_project_tools` 用户级开关）在 `docs/DECISIONS.md` 中无记录（grep 无匹配项）；按仓库约定建议补记（如 D-11）。
- **N-3（P3，测试健壮性）** SSRF 用例依赖真实 DNS（见 R-6），与批次 1 无关。

## 6. 自检

- 本报告与探针只新增于 `review/`，未修改 src/、tests/、docs/；未执行 `-m live`。
- 结论均来自本会话亲跑的 pytest 与探针输出；对实现者 fix notes 的每条主张均做了独立动作（其中 C4 排除了一次探针自身误差）。
- 全套件口径复核：109 passed / 2 failed（环境）/ 12 deselected，与批次说明一致。
