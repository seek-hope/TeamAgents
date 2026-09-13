# Rust 重构全面审查（recording: reconstruct @ d93c40e）

- 范围：core/、engine/、tui/ 三个 crate 全部源码与测试，对照 Python 基准（src/teamagents/）、
  docs/ 与 README。
- 方法：5 个只读审查 agent（core / engine 运行时层 / engine 工具层 / tui / 文档），
  真实二进制与假服务端到端复现 + PTY 真终端实验；基线 core 14 / engine 42 / tui 40 全绿。
- 修复：4 个修复批次并行（core / engine-runtime / engine-tools / tui），随后文档批次。
  本文档记录发现、证据与修复去向；修复完成状态在每个条目末尾标注。

严重度统计：严重 2 · 高 11 · 中 22 · 低 17（含重复归并）。

---

## 一、严重（会崩/会丢）

### S1 [严重] `/settings` 浮层与 toast 在终端宽 <48 时 panic，且崩溃不恢复终端
- `tui/src/ui.rs:625-643`（settings 框 `clamp(48,92)` + `Clear`）、`ui.rs:1323`（toast）。
- 证据：PTY 40x20 输入 `/settings` → `index outside of buffer`（ratatui `Clear` 不做交集），
  退出无 `?1049l`、无鼠标关闭序列；探针：20/30/40/47 列 panic、48 列 ok；toast 在 40x24、60x24 panic。
- 修复：矩形夹取到 buffer + `main` panic hook 恢复终端。→ TUI 批次 T1

### S2 [严重] `/` 命令菜单在 12–27 列宽 panic
- `tui/src/ui.rs:1005`：`clamp(24, area.width.saturating_sub(4))` min>max。
- 证据：PTY 26x20 输入 `/` → `min > max. min = 24, max = 22`。
- 修复：min ≤ max，过窄不画。→ TUI 批次 T2

---

## 二、高

### S3 [高] core `emit`/`schedule` 全量吞错：写失败仍回 `ok:true`
- `core/src/control.rs:172-194`（schedule/emit 全程 `let _ =`）、118-130、1876-1889、1927-1950、1997-2010。
- 证据：probe2（/tmp/ta-review）——另一连接持写锁时 emit 等 10s 后返回 `{"ok":true}`，事件未落库；
  begin 失败被吞后退化为 autocommit，破坏单事务不变量；engine `runtime.rs:688-701` 又二次吞。
- Python 基准：`storage.py` 的 BEGIN IMMEDIATE 与 `Control.emit` 不吞、会抛。
- 修复：返回 Result 向上传。→ core 批次 C1 + engine-runtime 批次 E1

### S4 [高] 载入路径 panic：无 spec / 旧 limits 键直接杀进程、毒化互斥锁
- `core/src/control.rs:174,182` `expect("spec")`、`storage.rs:379` `expect(...)`、
  `models.rs:131-132` Limits `deny_unknown_fields`（Python 明确丢弃未知 limits 键兼容旧会话）。
- 证据：probe1（无 spec emit → exit 101）、probe3（legacy `max_runs_per_goal` → panic）。
- 修复：map_err + 容忍未知键。→ core 批次 C2

### S5 [高] shell 工具 >64KiB 输出死锁：命令被误判超时、输出全丢
- `engine/src/tools.rs:313-356` 先 try_wait 轮询、退出后才 wait_with_output；子进程阻塞在管道写。
- 证据：`seq 1 20000`（≈108KB）→ `Err("command timed out after 8s")`；200KB 截断分支不可达。
- 修复：读线程 + 到点 kill。→ engine-tools 批次 N1

### S6 [高] MCP stdio 不排空子进程 stderr：服务端一写日志就卡死握手
- `engine/src/mcp.rs:236-243`（piped 但从不读）。证据：噪声 stderr 服务端 6s 不完成握手。
- 修复：inherit/null 或排空线程。→ engine-tools 批次 N2

### S7 [高] 回合活动超时后成员线程继续执行（幽灵写入）
- `engine/src/runtime.rs:599-621` 超时只放弃 recv，不发中断；core validate 不校验 run 状态。
- 证据：run FAILED 后仍持续调模型并写共享空间（ghost-1/2/3），直至 200 步上限。
- 修复：超时调 request_interrupt。→ engine-runtime 批次 E2

### S8 [高] ChatRunner 模型步数上限硬编码 200，`max_model_steps_per_turn` 形同虚设
- `engine/src/chat.rs:455`；`runtime.rs:640-660` 只把它当工具调用计数；`chat.rs:627` 不落 `note=turn_limit`
  → core 永不发 `limit_reached`（D-10 / ACCEPTANCE T22 与实现不符）。
- 证据：spec limit=3 实测模型被调用 200 次；Python oracle 对应用例通过。
- 修复：按 spec 计数模型请求 + note。→ engine-runtime 批次 E3

### S9 [高] 一次性批准（once）永不生效；EXPIRED 记录被放行
- `engine/src/gateway.rs:215-232` 只在 run_id+tool_call_id+op_hash 同时命中时放行，恢复后模型换新 tool_call_id
  → 再弹批准、once 永不消费（实测首个批准永远 APPROVED_ONCE、run 卡 WAITING_APPROVAL）；
  `_` 分支把 EXPIRED 也放行（Python 只放行 ONCE/SESSION + revision 相同）。
- 修复：跨 crate 合同 `approval_find_run` + `expire_approval`，语义对齐 permissions.py/runners.py。
  → core 批次 C5 + engine-runtime 批次 E4

### S10 [高] 表格滚动后点击行错位 1~2 行（几何仍有重复计算）
- `tui/src/main.rs:504` 命中算式 vs `ui.rs:548-553/849` 渲染体高、`ui.rs:233` 写死 rows_y；
  60 列时 hint 折 2 行错位更大；`pty_click_check.py` 只覆盖未滚动场景。
- 修复：几何并入 `ui::Geometry` 单一来源。→ TUI 批次 T3 + T12

### S11 [高] 面板聚焦时 Ctrl+A/S/D/C 触发破坏性动作
- `tui/src/app.rs:1361-1449` 不检查修饰键：Ctrl+A 归档、Ctrl+D×2 删会话、Ctrl+A/S/D 批准/拒绝、
  Ctrl+C 取消任务；文档把 Ctrl+A/E/U/D 定义为编辑/滚动。
- 修复：动作键拒绝 CONTROL/ALT + Ctrl+D/U 滚动守卫覆盖面板。→ TUI 批次 T4

### S12 [高] README 快速上手与实现相反（"不读项目内配置、不读 MCP 与 skills"）
- `README.md:35-36` vs `engine/src/session.rs:169`（三条路径都读项目配置）、MCP stdio 已实现、
  Skills 注入已实现；同文件 110-114 段与 USER-GUIDE §0 表均已标 ✅，自相矛盾。
- 修复：文档批次。→ docs D1

### S13 [高] USER-GUIDE 四处"未移植/会失败"标注在 D-19 后已失效
- `docs/USER-GUIDE.md:31`（项目配置）、`:71`（MCP）、`:82`（Skills）、`:189-190`（git_worktree）；
  实现均已落地并有测试（workspace.rs 的 worktree 生命周期、mcp_tools 真实 stdio 测试等）。
- 修复：文档批次。→ docs D2

---

## 三、中

### C3 投递批次账本不回写
- `core/src/storage.rs:456-463/482-493/1098-1104`；probe5：ack 后仍 `(next_batch_no=1, last_applied_batch=0)`，
  Python 应为 `(2,1)`；破坏"两版共享会话 DB"（D-15）。→ core C3

### C4 `run_started` 事件全仓库无生产者
- `core/src/control.rs:1891-1923` 只发 task_started；Python runtime.py:424-427 每回合必发 → TUI 死分支。→ core C4

### C6 `wait_for_tasks` results 形状：数组 vs 以 task_id 为键的对象
- `core/src/control.rs:587-590/1971-1978` vs Python control.py:425 / runtime.py:483。→ core C6

### C7 `drop_pending_deliveries` 丢 `dropped_reason`
- `core/src/storage.rs:747-754` vs Python storage.py:626-634。→ core C7

### N3 open_session 失败不释放会话锁
- `engine/src/session.rs:176/266-269`；实测失败后重试永远 "already running"，需重启进程。→ engine-tools N3

### N4 MCP 子进程继承 engine 全量环境（含 API key）
- `engine/src/mcp.rs:234-243`；实测注入的 TAPROBE_SECRET 可见；Python 只给 6 个白名单变量。→ engine-tools N4

### N5（并入 S8）绑定 MCP 调用绕过工具预算

### N6 guard_url 判据弱于 Python + IPv6 URL 全被误拒
- `engine/src/tools.rs:359-419`；实测 224/198.18/192.0.2 被放行，`[::1]` 被误拒（host 先按 ':' 拆）。→ engine-tools N6

### N7 会话 pid 锁抢占窗口（空锁被当 stale 删除）
- `engine/src/sessions.rs:43-81`；`is_session_locked` 同步误报。→ engine-tools N7

### G4 ChatRunner 成员历史仅内存、重启即丢
- `engine/src/chat.rs:244,267`；USER-GUIDE §3 称"成员私有线程重新装载"；RECONSTRUCT 未列。
- 实测：重启后旧 marker 不在请求里。→ engine-runtime E10（懒持久化或明确标注）

### G5 CodexRunner::reconcile 调不存在的方法 `thread/status`
- `engine/src/codex.rs:697-712`；真实 CLI schema（0.154.0，99 方法）无此法 → 恒 OUTCOME_UNKNOWN；
  Python 用 `thread/read {includeTurns:true}`。→ engine-runtime E5

### G7 xhigh→max 归一化与"被拒后改判 max 重试"缺失（D-8 项）
- `engine/src/chat.rs:324-378`；Python providers.py:22-38 + runners 的 _switch_effort_to_max。→ engine-runtime E6

### G8 Codex 审批 600s 自动 decline 后 core 仍 PENDING，用户后续批准静默无效
- `engine/src/codex.rs:462-472` + `runtime.rs:325-337`。→ engine-runtime E7（用 expire_approval 闭环）

### G9 runner 线程 panic 被误报为"活动超时"
- `engine/src/runtime.rs:609-621` Err(_) 同时吃 Timeout/Disconnected。→ engine-runtime E8

### G6 engine 侧 ChatRunner/审批链路零端到端测试（F-1/2/3 全数漏网的原因）
- 端到端场景全由 ScriptedMember 驱动（稳定 tool_call_id 掩盖 once 问题）。→ engine-runtime E11（新增假 OpenAI harness）

### T5 bracketed paste 未启用（`Event::Paste` 死代码，多行粘贴误提交首行）
- `tui/src/main.rs:157-171`。→ TUI T5

### T6 中文输入光标按字符数定位（每个 CJK 差 2 列）
- `tui/src/ui.rs:1136/1169`；实测 "中文ab" 光标 x=7 应为 9。→ TUI T6

### T7 滚轮目标按 focus 而非指针（与 D-20 #7/文档不符）
- `tui/src/app.rs:1130-1150`。→ TUI T7

### T8 窄终端 Ctrl+Home 到不了最早一条（固定 80 列估算）
- `tui/src/app.rs:1152-1172`。→ TUI T8

### E3 面板键盘焦点与键位文档不一致（Ctrl 组合被当普通键、Tab 未入文档）
- 同 S11，文档侧 README:61 / USER-GUIDE:230 补 "Tab 进面板"。→ docs D3

### E4 长输出 artifact 机制未移植且未记录（文档承诺 /artifacts）
- `engine/src/tools.rs:337-345`（只截断）、`gateway.rs:137`（read_artifact 预授权但无 executor）；
  Python execution.py:132-165 + runners.py:474 有落盘与挂载。→ engine-tools N2 组（artifacts）

### E5 测试计数过时
- `README.md:66-70`（engine 27/tui 21）、`docs/RECONSTRUCT.md:61-64`（tui 33）、`README.md:94`（D-1..D-17）；
  实际 14/42/40、DECISIONS 已到 D-20。→ docs D4

### E6/E11 `/settings` 菜单文案仍写"（语言、动效）"
- `tui/src/app.rs:31` + `tui/src/i18n.rs:174`。→ TUI T11

### E7 DECISIONS D-20 补充一（sidebar_width）被补充四推翻但未标注
- `docs/DECISIONS.md:333-338`；代码已无该函数。→ docs D5

---

## 四、低

- C8 validate/reduce 三处边界不一致（publish_shared 空 content、patch_id 空 operations、read_shared 畸形参数）→ core C8
- C9 payload_hash 分隔符与 Python 不一致（RECONSTRUCT 称对齐）→ core C9
- C10 `synchronous=NORMAL` 偏离 Python 默认（未记录）→ core C10
- C11 core 关键路径零覆盖（server dispatch、reduce 回滚、emit 失败、legacy limits）→ core C11
- N8 isolated 工作区 INPUTS.md 每次 prepare 被清空（Python 是 touch）→ engine-tools N8
- N9 web 工具 fail-open（未绑定成员仍可执行 web_fetch）+ 绑定选择不确定 + required 不校验 → engine-tools N9
- N10 doctor 缺 bwrap/codex 功能探针（与 D-3 记录不符）→ engine-tools N10
- N11 `[permissions]` 非法类型静默忽略（Python 抛 ValueError）→ engine-tools N11
- N12 TUI 每秒 list_sessions 全目录递归（11ms/次 @2 万文件，随 worktree 增长）→ engine-tools N12
- G10 Codex 子进程清理不杀进程组（Python killpg 15→9）→ engine-runtime E9
- G11 chat HTTP 重试过宽（4xx 也重试、末尾多睡、无 Retry-After）→ engine-runtime E9b
- G12（并入 S9）EXPIRED 放行
- T9 /settings 语言下拉错位并与 info 行重叠 → TUI T9
- T10 未知/带参数斜杠命令被当普通消息发给 Leader → TUI T10
- T12 残余几何重算（main.rs 手写边框内缩）→ TUI T3
- E8（并入 G7）xhigh 文档未标差异 → docs
- E10 TUI-CODEX-REFERENCE 过时（动效偏好、Python 测试作文末验证）→ docs D6

---

## 五、根因分析（为什么要成批修，而不是逐条打补丁）

1. **几何/布局的"第二份实现"是最贵的一类 bug**：S10/T12、S2、S1 都是"渲染与命中/边界各算一遍"
   的变体；本项目已经有过一次同源事故（D-20 补充二）。修复方向统一为"几何只允许 ui::geometry
   产出"，并在测试里锁定。
2. **静默吞错（`let _ =`）是数据丢失的温床**：S3 + engine 侧二次吞掉，让一次锁竞争升级为
   "事件丢失 + run 永久 RUNNING"。修复原则：core 不吞错、engine 显式处理或显式记录。
3. **ChatRunner（默认成员后端）是全项目最薄弱的一环**：S8/S9/S7/G4 全部集中在它身上，
   根因是它没有端到端测试（G6），且缺少"运行时语义"的对照物（Python 靠 LangGraph middleware
   与 interrupt 实现的能力，需要在 Rust 里等价重建或明确降级）。
4. **文档滞后于 D-19/D-20 两轮实现**：README/USER-GUIDE/RECONSTRUCT 的"未移植"标签大面积过期，
   以及测试计数、决策编号（S12/S13/E5/E7/E10）。这类问题不修会让后来者做出错误判断。
5. **几处安全/健壮性边界弱于 Python**：MCP 环境白名单（N4）、web 工具 fail-closed（N9）、
   guard_url 判据（N6）、锁的原子性（N7）。都是"照 Python 抄一遍就能对齐"的低风险高收益项。

## 六、跨 crate 合同（S9 修复）

core 新增两个 JSON 方法，engine 按此调用（形状冻结）：

- `expire_approval`：`{"approval_id": "..."}` → `{"ok": bool}`；APPROVED_ONCE/PENDING → EXPIRED
  （Python `storage.py:935-942`）。
- `approval_find_run`：`{"session_id", "run_id", "operation_hash"}` → `{"approval": 对象|null}`；
  返回该 run+op_hash 下最新一条 status ∈ {PENDING, APPROVED_ONCE} 的批准。

engine 语义（对齐 Python `permissions.py::check` + `runners.py:325-360`）：PENDING → park（复用行）；
APPROVED_ONCE + revision 相同 → 放行、执行后 consume；APPROVED_SESSION + revision 相同 → 放行不消费；
DENIED → 拒绝；EXPIRED → 重新请求。

## 七、修复批次与状态

| 批次 | 写域 | 条目 |
|---|---|---|
| core | core/** | C1-C11 |
| engine-runtime | engine/src/{runtime,chat,codex,gateway,scripted,core_client}.rs | E1-E11（S3 引擎侧、S7、S8、S9） |
| engine-tools | engine/src/{tools,mcp,workspace,sessions,session,config,cli,worker,main}.rs | N1-N12、E4 |
| tui | tui/** | S1、S2、T3-T12 |
| docs（紧随其后） | README、docs/** | S12、S13、E3、E5、E7、E10 及新决策记录 |

## 八、修复结果（2026-09-13 完成）

四个修复批次全部落地；集成验收（主流程实跑）：

| 套件 | 基线 | 修复后 |
|---|---|---|
| core `cargo test --offline` | 14 | **38** |
| engine `cargo test --offline` | 42 | **67** |
| tui `cargo test --offline` | 40 | **54** |
| PTY 冒烟 / 点击检查 | — | 通过 |
| 真实模型冒烟（DeepSeek `--plain`，`/tmp/ta-live`） | — | 通过（goal_done + 回复） |
| `teamagents doctor` | — | 通过（bwrap 实跑、codex schema 99 方法） |

逐条修复内容、测试名与保留项见 `review/fix-notes-rust-review-2026-09-13.md`。
文档同步由后续批次处理（README / USER-GUIDE / RECONSTRUCT / DECISIONS / ACCEPTANCE / STATUS / TUI-CODEX-REFERENCE）。

集成期补充修复（主流程直接改的跨域尾巴）：engine `runtime.rs` 的 7 处 `let _ =` 改为
`core_best_effort`（失败打日志）；engine `gateway.rs::canonical_json` 分隔符对齐
Python `json.dumps` 默认（新增 `operation_hash_matches_python_json_dumps` 向量测试）。
