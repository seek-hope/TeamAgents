# 审查报告 · 测试、文档与方案符合性（对抗性验证域）

> 审查员：reviewer_verify　基准：`TeamAgents-Implementation-Plan.zh-CN.md`　协议：`review/PROTOCOL.md`
> 状态：终稿（所有探针已运行；证据与结论见下）

## 一、范围与方法

- 精读：`tests/` 全部（conftest.py、scripted_model.py、fake_codex_app_server.py、mcp_echo_server.py、
  test_t1–t6、test_config_cli、test_p1_guards、test_p2_cancel_pause、test_p2_recovery、
  test_p3_deepagents_runner、test_p3_live_session、test_p3_real_models、test_p3_tools_and_session、
  test_p3_web_tools、test_p4_topology、test_p5_codex_adapter、test_p5_live_codex、test_p5_workspace、
  test_p6_sessions_ui、test_p6_tui、test_p6_tui_live）；
  `docs/`（STATUS、ACCEPTANCE、DECISIONS、P0-findings、USER-GUIDE）、`README.md`；
  方案 §2–§18（重点 §6.4、§13、§15、§16、§17、§18）；源码 `control.py`、`storage.py`、`runtime.py`、
  `runners.py`、`agents.py`、`execution.py`、`views.py`、`providers.py`、`codex.py`、`cli.py`、
  `session.py`、`sessions.py`、`tui/app.py`、`tui/panels.py`、`examples/e2e_*.py`（静读）。
- 运行命令（详见第六节）：确定性套件、`--collect-only`（含 `-m live`）、单用例复跑（定位环境失败原因）、
  对抗探针 7 个（`review/tmp/probe_verify.py`，逐一判定证实/证伪）。
- 未运行 `-m live`、未联网、未修改 `src/**`、`tests/**`、`docs/**`、`examples/**`。

## 二、结论

1. 确定性套件在沙箱为 **75 passed / 2 failed / 12 deselected（77 collected）**，2 个失败均为环境性
   （缺 `DEEPSEEK_API_KEY`、无 DNS），但其中 1 个（config_cli）暴露测试本身非 hermetic，属测试设计缺陷（V-5）。
2. 文档声称的「65 passed / 12 live」与实测收集数不符（V-1）；T7 声称「三家供应商用例已就绪、导出密钥即可跑」
   与测试代码不符——**根本不存在这三家的用例**（V-2），属"文档声称完成但证据不足"。
3. T24 声称的 `context_epoch` 机制在代码中**零调用**（定义在 storage.py:712，全仓库无调用点），
   同名成员重建不继承旧身份的路径实际未接线（V-3）。
4. 6 个关键声称的对抗探针：5 个被证实（含重复动作回执、去重、非 Leader 拒绝、全自动用户专属、
   LIMIT_REACHED 不改写 goal_state），1 个（同名成员不继承身份）被证伪（与 V-3 一致）。详见第三节探针表。
5. 总体判断：实现侧核心语义（去重、恢复、权限、超限不谎报完成）经探针与测试双重验证可信；
   主要问题集中在**验收证据与文档状态的可信度**（计数过期、T7/T24 证据不实、薄弱用例无自动化覆盖）。

## 三、发现清单

| ID | 严重度 | 标题 | 证据（file:line） | 影响 | 建议 |
|---|---|---|---|---|---|
| V-1 | P2 | 文档测试计数过期且 live 计数不成立 | `docs/STATUS.md:8-9`（"65 passed"、"12 passed（约 90s）"）、`docs/DECISIONS.md:56`（"65 确定性 + 12 实跑"）；实测见第六节；live 12 条全部带 `skipif`（如 `tests/test_p3_live_session.py:18`、`tests/test_p5_live_codex.py:23`、`tests/test_p6_tui_live.py:14`） | 状态文档不可信；跳过与通过混同 | 改为实际计数：确定性 77 collected（75 pass + 2 环境失败）；live 标注"12 collected，全带 skipif，需密钥/CLI 才计数" |
| V-2 | P2 | T7「Anthropic/GLM/OpenAI 用例已就绪」不成立 | `docs/STATUS.md:29,51`；`docs/ACCEPTANCE.md:4-5,15,44`；`docs/P0-findings.md:79`；反证：`tests/test_p3_real_models.py:25-31`（PROVIDERS 仅 deepseek/kimi），`grep -rn "anthropic|glm" tests/` 无命中 | 验收表把「未实现用例」记成「缺密钥待跑」；T7 实际 2/5 有测试 | ACCEPTANCE/STATUS 改为「未覆盖（无用例）」，并补三家参数化用例（base_url 中转 + skipif） |
| V-3 | P1 | T24 的 context_epoch 机制是死代码：同名新成员会继承旧身份 | 定义 `src/teamagents/storage.py:712`，全仓库零调用（`grep -rn bump_context_epoch` 仅命中定义）；`src/teamagents/control.py:1152-1153` 恒为 `ctx:<id>:1`；`src/teamagents/runners.py:383` 用该 epoch 组合检查点线程；`docs/ACCEPTANCE.md:32` 却将其列为机制证据 | 成员移除后以同一 id 重建会复用同一检查点线程 → 旧历史/旧身份泄漏，T24 声称的隔离不成立；无测试覆盖 | 在成员移除/重建（`_sync_members`）时调用 `bump_context_epoch`，并补「移除→同名重建」验收用例 |
| V-4 | P2 | T23 穿越/符号链接无自动化证据 | `docs/ACCEPTANCE.md:31`（引用 execution.py 探针 + P0 手工记录）；`grep -rn -i "symlink|trav|\.\./" tests/` 仅 `tests/test_p3_web_tools.py:25`（`file:///etc/passwd`）；`src/teamagents/execution.py:167-199` `_resolve_path` 无单测 | 路径逃逸类回归无自动化防线 | 补 `..`、绝对路径、符号链接（指向 workdir 外）、大小写/前缀混淆用例，单测 `_resolve_path` |
| V-5 | P2 | 确定性套件非 hermetic（依赖本机密钥/DNS） | `tests/test_config_cli.py:144` 直接 `build_chat_model(deepseek)` → 实测 `ModelConfigError`（`src/teamagents/providers.py:54`）；`tests/test_p3_web_tools.py::test_ssrf_guard_blocks_internal_targets` 需解析 example.com（`guard_url`，无 DNS 即 `socket.gaierror`） | 无密钥/离线环境确定性套件必红；"全绿"依赖开发机私有状态 | 用 `monkeypatch.setenv` 注入假密钥或 `skipif`；SSRF 用例改解析打桩（或对 example.com 断言 skip） |
| V-6 | P3 | 恒真断言（同义反复） | `tests/test_t1_delegation.py:52` `assert "b" not in done_event["audience_json"] or True` | 该断言永不失败，且与注释意图相反；给人"已检查受众"的错觉 | 明确断言预期受众集合（如 `audience_json == ["leader"]` 或按真实语义断言 assignee 可见） |
| V-7 | P3 | 测试中 no-op 替换掩盖意图 | `tests/test_t3_discussion.py:49` `action_id=b_send.action_id.replace(":step0", ":step0")` | 看似构造"不同 action"，实为同 action_id 重放；测试本身有效（同 id 去重），但代码会误导维护者 | 直接写 `action_id=b_send.action_id` 并注释"重放同一动作"；若要测"不同 id 不误去重"，另写用例 |
| V-8 | P2 | T17 Codex 恢复断言过弱 | `tests/test_p5_codex_adapter.py:110-111` `assert state in ("COMPLETED", None)`；`src/teamagents/codex.py:440-441,450-451`（thread 缺失/无 turns 时返回 None） | 接受 None 使该断言几乎不约束 reconcile 映射；T17 的"断线恢复"证据不足 | 断言 `state == COMPLETED`（fake server 有 thread 历史时应可判定），并补 thread/read 抛错 → OUTCOME_UNKNOWN 的分支用例 |
| V-9 | P3 | 三项已核对无问题（防止误传） | DECISIONS D-10 所述步数上限修复成立：`src/teamagents/runners.py:437-439` 在图重建时读取 `limits.max_model_steps_per_turn`；`tests/test_p3_deepagents_runner.py:249` 以 3 步预算触发 `limit_reached` 且 run FAILED | （正面证据） | 无 |
| V-10 | P3 | `--plain` REPL 引用不存在的 `rt.ui_cursor`（与 surface 域交叉） | `src/teamagents/cli.py:231-232`（`rt.store.events(rt.session_id, after_sequence=rt.ui_cursor)`）；`grep -rn ui_cursor src/teamagents/` 除 cli.py 自引用外无定义 | `teamagents --plain` 首次发送消息后必抛 AttributeError | 与 `review/findings-surface.md` 同源，去重后以 surface 报告为准 |

### 对抗探针结果表

探针文件：`review/tmp/probe_verify.py`（单文件 7 个探针，基于 `tests/conftest.py` 的 `Harness`/`fake_session` 与
仓库自带假成员驱动，未修改任何被审文件）。运行：
`.venv/bin/python home/rimuru/Projects/Code/for_fun/TeamAgents/review/tmp/probe_verify.py`。

| # | 声称 | 结论 | 关键输出（实测） |
|---|---|---|---|
| PR-1 | 重复动作返回原回执、不重复生效 | **被证实** | 同 action_id 二次 `submit`：`r1 == r2` True；tasks=1；task_created=1 |
| PR-2 | 崩溃后重放同一动作不重复建任务、返回原回执 | **被证实** | 硬关闭会话后新 runtime 重放：settle True；tasks=1；task_created=1；重放回执 ok 且 kind 一致 |
| PR-3 | 已确认投递不重复注入 | **被证实** | 连续 3 次 `control.schedule()`：`pending_deliveries("s1","b")==[]`；b 收件数 1→1；重启后新成员注入 0 条 |
| PR-4 | 非 Leader 不能 signal_done / apply_topology_patch | **被证实** | 成员 b 两次提交均拒（"only the Leader …"）；无 `goal_done` 事件 |
| PR-5 | 全自动权限只能由 user 开启 | **被证实** | member/leader 均拒（"only the local user can change the permission mode"）；user 提交成功且 `permissions_mode == "full_auto"` |
| PR-6 | LIMIT_REACHED 不冒充完成 | **被证实** | 触发超限：`limit_reached` 在、无 `goal_done`、`goal_state == "active"` |
| PR-7 | T24：同名成员移除→重建后 `context_epoch` 递增（不继承旧身份） | **被证伪** | `remove_agent` 与同名 `add_agent` 均应用成功，epoch 始终 1；`ctx_refs` 无新代次（详见 V-3） |

### T1–T24 薄弱证据映射表

> 说明：下表仅标注本次审查**能给出证据**的薄弱/不实项；其余场景为本次已读测试（用例存在、断言为真实检查），
> 未逐条行级枚举，不构成"完全无问题"的保证。

| T | ACCEPTANCE 证据 | 本次核对结论 |
|---|---|---|
| T1 | `test_t1_delegation.py` | 主链路断言真实；含 1 条恒真断言（V-6） |
| T2 | `test_t2_parallel.py` | `asyncio.Barrier(2)` 证明并行开始（15-64 行）+ 执行中补充用例；未见薄弱 |
| T3 | `test_t3_discussion.py` | 去重断言有效；重放代码为 no-op 替换（V-7） |
| T4/T5 | `test_t4_observer.py`、`test_t5_shared.py` | 已读；ACL 拒绝与 scope 裁剪断言直接；未见薄弱 |
| T6 | `test_t6_isolation.py` | 新会话不继承有直接断言（38-60 行）；"同名成员"隔离不属 T6 |
| **T7** | `test_p3_real_models.py` 等 | **证据不实：3/5 供应商无用例（V-2）** |
| T8/T9 | `test_t1_delegation.py` | 已读；未见薄弱 |
| T10–T14 | `test_p4_topology.py` | 已读（含 T13 移除移交 + 成果保留）；未见薄弱 |
| T15/T16 | `test_p5_codex_adapter.py` | 已读；假 app-server 驱动批准/中断路径；未见薄弱 |
| **T17** | `test_p5_codex_adapter.py` | **恢复断言过弱：接受 `None`（V-8）** |
| T18 | `test_p5_workspace.py` | 单元级覆盖脏输入回退与未合并拒绝清理；真实 git worktree 合并动作无端到端用例（存疑项） |
| T19/T20 | `test_p6_sessions_ui.py`、`test_p6_tui.py` | 已读；未见薄弱（真实界面证据依赖 live，未运行） |
| T21 | `test_p2_recovery.py` | 证据充分；PR-1/2/3 独立探针再次证实 |
| T22 | `test_p1_guards.py`、`test_p2_cancel_pause.py` | 证据充分；PR-4/6 独立探针再次证实 |
| **T23** | `execution.py` 探针 + P0 手工记录 | **穿越/符号链接无自动化证据（V-4）** |
| **T24** | `test_p2_recovery.py`、`test_t6_isolation.py`；`context_epoch` | **机制死代码、声称被证伪（V-3、PR-7）** |

## 四、与方案/文档的偏差

1. **T7 偏差（V-2）**：方案 §17 要求五家模型工具调用与续接验收；当前仅 2 家有 live 契约测试，
   另外 3 家无任何用例。文档三处将其描述为"测试已就绪"，属"文档声称完成但证据不足"。
2. **T24 偏差（V-3）**：方案要求"同名新成员不继承旧身份"；`context_epoch` 机制未接线，
   ACCEPTANCE 以未使用的方法作为证据。
3. **DECISIONS D-1..D-10 核对**：D-1/D-3/D-4/D-5/D-6/D-7/D-8/D-10 在代码中均可对应（D-10 的
   `max_model_steps_per_turn` 修复经 runners.py:437-439 与测试验证成立；D-8 的 xhigh→max 映射见
   `providers.normalize_effort`）。D-2 属"实现细节澄清、非偏离"。未发现"偏离方案但完全未记录"的模块级职责；
   `providers.py/runners.py/session.py/sessions.py` 属文件拆分而非职责偏离。
4. **`tests/` 中 T23 手工证据（V-4）** 与 ACCEPTANCE 的 ✅ 标记不匹配——手工 P0 记录不能作为回归证据。

## 五、未验证/存疑项

- V-3 中"同名成员重建后检查点线程复用"的完整链路（LangGraph thread id 组合方式）在探针中只能验证
  `context_epoch` 不变这一事实；线程复用的实际后果标注「待验证」，但机制未接线本身证据确凿。
- live 用例（`-m live`）按协议未运行；其真实模型行为未验证。
- `tests/test_p5_live_codex.py`、`test_p6_tui_live.py` 的 skip 条件（缺 codex CLI / 密钥）未实测，仅静态核对。

## 六、自检（实际运行过的命令与结果）

```bash
# 1) 确定性套件（完整）
.venv/bin/python -m pytest tests/ -q
# -> 2 failed, 75 passed, 12 deselected in 29.90s
#    失败 1: tests/test_config_cli.py::test_xhigh_maps_to_max_for_models_without_xhigh
#            （复跑确认：providers.py:54 ModelConfigError，缺 DEEPSEEK_API_KEY，纯环境原因）
#    失败 2: tests/test_p3_web_tools.py::test_ssrf_guard_blocks_internal_targets
#            （socket.gaierror: Temporary failure in name resolution，无 DNS，纯环境原因）

# 2) 收集数核对
.venv/bin/python -m pytest tests/ --collect-only -q          # 77/89 collected (12 deselected)
.venv/bin/python -m pytest tests/ --collect-only -q -m live  # 12/89 collected (77 deselected)

# 3) 对抗探针（review/tmp/，未修改任何被审文件）
.venv/bin/python home/rimuru/Projects/Code/for_fun/TeamAgents/review/tmp/probe_verify.py
# -> PR-1 证实 / PR-2 证实 / PR-3 证实 / PR-4 证实 / PR-5 证实 / PR-6 证实 / PR-7 证伪
#    （PR-5 首轮因探针自身读取列名笔误“无法验证”，修正后复跑；PR-3/PR-7 亦做过修正后复跑，见探针文件）
.venv/bin/python home/rimuru/Projects/Code/for_fun/TeamAgents/review/tmp/probe_verify.py PR-3 PR-5 PR-7  # 定向复跑
```

- 本仓库**不是 git 仓库**（`git status` → fatal: not a git repository），无法以 `git status` 自证未修改；
  本次审查对 `src/**`、`tests/**`、`docs/**`、`examples/**` 只使用读操作（cat/sed/grep），
  写操作仅限 `review/**`。
- 环境：沙箱无网络、无 `DEEPSEEK_API_KEY` 等密钥；未运行 `-m live`。
