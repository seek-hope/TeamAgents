# TeamAgents 形式化验证报告（2026-09-24）

本报告是形式化验证工作的**结论与台账**：证明到什么程度、靠什么证据、哪些**没有**证明。
规格、性质与代码锚点的逐条映射在 [README.md](README.md)；修复台账在
[review/fix-notes-verification-2026-09-24.md](../review/fix-notes-verification-2026-09-24.md)。

## 1. 结论速览

**能声称的**：

- 7 个协议面（控制面、制品/GC、等待/唤醒、任务/委派/目标结清、压缩、daemon 协议、必需检查）的
  安全性性质在**抽象模型**上用 TLC 穷举成立；活性性质只在明确标注的弱公平假设下成立。
- 同一组不变量在**真实 `core::v2::Control`** 上以可执行对应测试重算：长度 ≤ 2 的命令序列穷举
  （38 种命令、含被拒绝的组合）+ 60 条固定种子的覆盖驱动随机游走，每步之后检查 23 组不变量，
  并带覆盖率断言与检查器灵敏度反向验证。
- 内核纯函数（线协议视图、输出裁剪、分页、响应分类、参数散列）的性质以有界穷举直接检查；
  其中 readback 分页的**算术**另有 Kani 机器证明，且被证明的 `page_span` 是**发布函数**
  （`page_output` 调用它）：两条性质对**任意 `usize`** 成立（不溢出、不越界、`eof` 判据等价），
  一条（逐页取回无缝重建）在展开界内成立。
- 上述工作抓到并修复了 4 处代码问题（V-W1/V-G1/V-P1/V-P2，各有反例与回归测试），并纠正了 2 处
  自身写错的性质（空性质/空转检查）。

**不能声称的**（详见 §5）：

- **不是精化证明**：模型 ≠ Rust 实现。模型上的穷举结论不自动成立在代码上；代码侧的结论只来自
  有界探索（穷举 + 采样），不是"所有执行"。
- **活性只在假设下**：唯一写下的活性性质（`V2Wait::NoStrandedPending`）依赖"停放 drain 弱公平"，
  即 driver 轮询循环持续运行；那是实现事实，不是被证明的结论。
- **状态空间有界**：所有穷举都在显式有界配置上（实例/任务/请求/日志条数有限），前沿在 README 记录
  （例如 `MC_task` 把任务加到 2 个会发散）。
- **模型外的代码面**：供应商适配器、MCP、Skills、TUI 渲染与命中、Shell/bwrap 隔离、进程与 job 管理、
  真实供应商行为都不在形式化范围内（它们由样本测试与真实环境验收覆盖，见 §4 的"未覆盖部分"列）。

## 2. 证据清单（全部可复跑）

| 层 | 证据 | 规模 | 复跑 |
|---|---|---|---|
| 协议模型 | `tla/V2Control.tla`（13 不变量 + 4 性质） | 37,269 状态 | `make verify-model` |
| 协议模型 | `tla/V2Artifact.tla`（4 + 4） | 241 状态 | `make verify-model-all` |
| 协议模型 | `tla/V2Wait.tla`（8 + 1 活性） | 505,905 状态 | 同上 |
| 协议模型 | `tla/V2Task.tla`（11） | 5,721,401 状态 | 同上 |
| 协议模型 | `tla/V2Compress.tla`（8） | 8,467 状态 | 同上 |
| 协议模型 | `tla/V2Daemon.tla`（10） | 51,713 状态 | 同上 |
| 协议模型 | `tla/V2Checks.tla`（8） | 469 状态 | 同上 |
| 协议模型（宽配置） | `MC_wide.cfg`（2 实例 / 2 操作） | 275,004,673 状态 / 11 分 25 秒（历史运行，同文件未改动：`git log -1 -- verification/tla/V2Control.tla MC_wide.cfg` = `d37e1b4`（2026-09-25 历史重写后的新哈希）；本轮两次复跑分别跑到约 1.7 亿 / 2.5 亿状态时被本机环境杀掉，未见违反） | `make verify-model-wide` |
| 代码级对应 | `core/tests/v2_invariants.rs` | 38 命令；1,482 条短序列 + 60×24 步游走 | `cargo test --offline --manifest-path core/Cargo.toml --test v2_invariants` |
| 纯函数层 | `core/tests/kernel_properties.rs` | 258 种条目组合 + 分页全枚举 | `cargo test --offline --manifest-path core/Cargo.toml --test kernel_properties` |
| Kani 证明 | `kani/`（直接编译仓库源码，3 个 harness，0 失败） | 2 条对**任意 `usize`** 成立 + 1 条界内 | `make verify-kani`（需 Kani 工具链，约 7 秒） |
| 门禁 | fmt + clippy `-D warnings` + 全测试 | 31 套 | `make check` |

非空与灵敏度证据（"通过"不是因为检查太弱）：

- 检查器灵敏度：`the_invariant_checker_detects_broken_states` 人为破坏状态（未知状态值、终态改写、
  悬挂目标指针、覆盖回抬、覆盖指向非总结），检查器必须报出来。
- 覆盖率断言：随机游走必须真的走到"等被解决 / 目标结清 / 任务结清 / 操作终态 / epoch 重置 / 实例终止 /
  制品 LIVE / 压缩提交 / 批准已决定 / 命令重放 / 撤权后派发被拒"。
- 模型侧非空：把 `V2Daemon` 的 `checkpoint` 拆成两步 → `SnapshotNeverLeadsCursor` 立刻被反证；
  放宽 `V2Checks` 的 `Accept`（有一个 pass 就接受）→ `SuccessRequiresAllChecksPassed` 立刻被反证。
- 线协议配对的覆盖断言本身抓到过一次**空转**：第一版用子串找索引，导致配对检查从未真正执行。

## 3. 验证抓到的问题（全部已修复）

| 编号 | 问题 | 规格反例 | 修复与回归 |
|---|---|---|---|
| **V-W1** | wait 的 tool_call 只在 drain 路径被回答，"注册即满足"与"被取代/关闭 epoch"两条路径不回答 → 严格端点会拒绝下一次请求 | `ResolvedWaitAnswersItsCall is violated`（`ArmWait → Supersede → CANCELLED, answers=0`） | `answer_closed_waits` 推广到两条路径；`wait_call_answered_outside_the_drain_path`；不变量 `ResolvedWaitIsAnswered` |
| **V-G1** | 目标进入终态后仍接受新工作（委派、开操作、记账） | `RegisteredWorkNeedsAnActiveGoal`、`ClosedGoalTakesNoNewOperation` 违反 | `budget_goal` 只认 ACTIVE、结清摘除指针、委派要求 ACTIVE；`a_settled_goal_takes_no_new_work`；不变量 `NoStaleActiveGoal` 等 |
| **V-P1** | 终止实例后残留执行指针（`phase = MODEL_PENDING` 且指向已取消请求） | 代码级不变量：`OneActiveRequest: instance i1 is MODEL_PENDING with 0 pending turn requests` | 终止分支归一化（与 `reset_instance`/`fail_request` 一致）；`terminating_an_instance_normalizes_its_execution_pointer` |
| **V-P2** | `import_response` 不校验 `kind`，压缩请求可被当作回合导入 | 游走走到该路径并成功（规格要求拒绝） | 控制面拒绝 `kind != 'turn'`；`import_response_refuses_a_compression_request` |
| 性质修正 | `V2Checks` 最初写成"SUCCEEDED ⇒ 记录的结论是 pass"——**空性质**（动作自己写那个变量） | 放宽 `Accept` 也照样"通过" | 改为绑在**观察到的检查结果**上；放宽后立刻被反证 |
| 性质修正 | `V2Checks` 的 `BlockForInfra`/`BlockWhenExhausted` 里嵌套量词重名 | 解析报错 | 分开量词变量 |

## 4. 逐项台账 A01–A36

"形式化层"列只写**真的**由模型/代码级/纯函数层覆盖到的部分；其余依赖样本测试与真实环境验收
（证据见 `docs/ACCEPTANCE.md` 与 `review/archive/r2-p5-2026-09-24.md`）。

| 编号 | 场景 | 形式化层覆盖 | 仍未覆盖 |
|---|---|---|---|
| A01 | 单 Leader 完成目标 | `V2Control` 的相位机、`OneActiveRequest`、`NoTurnWithoutWork`；代码级"单活跃 turn 请求" | 真实供应商回合行为 |
| A02 | A→B→C→A 通信 | 代码级：信封应用去重、授权前置、目标指针 | 拓扑/环本身未建模 |
| A03 | 有限下授与父撤销 | 代码级：撤权后派发被拒（`!dispatch_without_grant` 探针 + 覆盖率断言）；模型 `StaleExecutorRejected` | 授权树的级联语义未建模 |
| A04 | 排队动作遭遇撤权 | 同上（撤销后派发必须被拒） | 排队/重授权时序未建模 |
| A05 | 读取其他实例历史 | — | 样本测试 |
| A06 | 消息应用事务前后重启 | `V2Control` 崩溃/恢复、信封去重；代码级回执稳定与重放惰性 | — |
| A07 | 永久启动错误 | `V2Control::FailRequest`（释放预留） | 停放语义、真实 spawn 失败 |
| A08 | 成功后消费前崩溃 | `V2Control`：`RecordBeforeEffect`、`EffectAtMostOnce` | — |
| A09 | 未知外部结果 | `V2Control`：未知结果不重放 | 任务停放的代码级路径未建模 |
| A10 | 重复派发/GO | `V2Control`：效果至多一次 | job 层未建模 |
| A11 | daemon/runner 分别崩溃 | `V2Control` 恢复路径 | 进程/job 层未建模 |
| A12 | 服务跨退出存活 | — | 真实进程证据（D-41） |
| A13 | 取消/超时/完成竞态 | `V2Control`：`CancelledBeforeStartHasNoEffect`、`TerminalOpStable` | — |
| A14 | bwrap 不可用 | — | 真实隔离环境证据 |
| A15 | 环境身份 | — | 进程身份证据 |
| A16 | 必需检查失败 | `V2Checks` 全部 8 条（含"只校验自称成功的候选"） | `execute_check_ops` 的执行细节 |
| A17 | 检查后产物变化 | `V2Checks` 的 stale 类与 `BlockedAfterTheBudgetOrStale` | 真实文件散列复核 |
| A18 | 多实例用量预算 | `V2Control`（预留上限/准入闸门/释放）+ `V2Task`（`budget_goal` 解析规则） | 真实供应商计费口径 |
| A19 | 半条流与失联 | `V2Control`：`SelectionIsComplete`；代码级选中尝试必为完整 | 供应商重试细节 |
| A20 | 压缩后重启 | `V2Compress` 全部 8 条 + 代码级 5 组（覆盖单调、原文不丢、尾部追加） | — |
| A21 | 用户直接调整 Worker | — | 单写上下文的样本测试 |
| A22 | 等待环与计时器 | `V2Wait`（含"停放的 PENDING 终会被关闭或被取代"） | 环检测的图算法本身 |
| A23 | 结果先到后注册等待 | `V2Wait` 注册即求值 + 代码级 `ResolvedWaitIsAnswered` | — |
| A24 | 重置后旧结果迟到 | `V2Control::NoReceiptAcrossEpochs` | — |
| A25 | MCP 批准/取消/未知 | 模型 `NoEffectBeforeApproval`；代码级批准决定终态、待批只属于 PREPARED、拒绝后无效果 | MCP 传输与工具面 |
| A26 | Skills 权限 | — | 真实符号链接/注册根证据 |
| A27 | 异构供应商合作 | — | 真实双方消息证据 |
| A28 | 断连/慢客户端/重连 | `V2Daemon` 全部 10 条 + 代码级回执稳定/重放惰性/日志只增 | 真 socket 层（daemon 测试覆盖） |
| A29 | 会话隔离/共享项目 | `V2Control` 的单会话约束 | 共享目录授权 |
| A30 | 制品与 DB 写入断点 | `V2Artifact` 全部 8 条 + 代码级 LIVE 必有字节 | 真实断电 |
| A31 | 写入失败/磁盘满 | — | `StorageFull` 分类与真实 SQLite FULL 注入 |
| A32 | 超大历史测量 | 纯函数层分页无缝重建（readback 的坐标语义） | 性能数字本身 |
| A33 | 双 daemon/旧锁 | — | 真锁与第二 daemon 证据 |
| A34 | schema 不兼容 | — | 印记/迁移样本测试 |
| A35 | 目标截止时间 | `V2Control` 的截止闸（`AdmissionGate`/派发硬拒） | 真实时钟边界 |
| A36 | 安装/init/doctor/清理 | — | CLI 与清理证据 |

小计：上表"形式化层覆盖"列非空的共 **25 项**（A01–A04、A06–A11、A13、A16–A20、A22–A25、A28–A30、
A32、A35），其余 **11 项**（A05、A12、A14、A15、A21、A26、A27、A31、A33、A34、A36）目前**只**有样本
测试与真实环境证据。**没有任何一项声称"仅靠形式化就已完成"**；反过来，形式化层覆盖到的项也不因此
免去样本/真实环境验收。

## 5. 未证明清单（诚实边界）

1. **模型 ≠ 代码**。模型是抽象状态机；代码级对应只是有界探索的验证，不是精化证明。要跨越这条线需要
   把每个不变量映射成代码上的可执行断言（已做）**并且**证明实现的每一步都在模型步集内（未做）。
2. **活性**：`V2Wait::NoStrandedPending` 依赖"停放 drain 弱公平"；`V2Control` 的恢复活性依赖
   `WF_vars(Recover)`。两处都是对被验证系统之外的调度器提出的假设。
3. **无界**：所有穷举都在有界配置内（请求/尝试/任务/日志条数有限，`MaxOps` 等）。计数器的无界性
   （预算、用量、事件序列）只在模型里以有界占位表示。
4. **模型外代码面**：供应商适配器与重试分类、MCP、Skills、TUI 渲染/命中、Shell 与 bwrap 隔离、
   job/进程生命周期、真实供应商行为。这些只由样本测试与真实环境验收覆盖。
5. **并发**：`Control::submit` 在单连接上串行（单写者），模型不覆盖多连接交错；daemon 的读连接与写
   连接并发只在 A28 的"慢客户端不阻塞写者"结构性说明里体现，没有交错穷举。
6. **纯函数层**：分页算术有 Kani 机器证明，被证明的 `page_span` 是发布函数（`page_output` 调用它），
   两条性质对**任意 `usize`** 成立。未覆盖的部分：(a) `page_output` 的参数解析走 serde_json，
   符号化坐标会让数字比较退化成符号化 `memcmp`（实测不收敛），参数合法性只有具体值穷举；
   (b) `cap_tool_output` 的 24000 字符阈值不可展开，同样只有边界长度的具体值测试；
   (c) 其余纯函数（`prepare_request` 的视图、`interpret_response` 分类、`args_hash`）只有有界穷举/枚举。
   Lean 4 未采用：它是交互式定理证明，需要 elan 工具链与人工证明脚本；本轮的优先级给了"多一个协议面
   的穷举 + 代码级对应 + 把 Kani 用在能收敛的发布函数上"，升级路径写在 README 的后续阶段。

**已知偶发（与本轮改动无关，记录在案）**：`v2_driver::long_context_compacts_before_the_turn_and_survives_a_restart`
在机器刚跑完 Kani/TLA 负载时失败过一次（`requests.len() == 1` 的时序断言，等 15 秒窗口）；隔离复跑 3/3
通过，随后安静状态下整门禁 31 套全绿。它属于负载敏感的时序断言，不是验证层的性质。

## 6. 复跑与可推翻性

```bash
make verify-model-all     # 7 个协议面穷举（秒级到约 20 秒）
make verify-model-wide    # 控制面宽配置（约 11 分钟 / 275M 状态）
make check                # fmt + clippy -D warnings + 31 套测试（含两层代码级检查）
make verify-kani          # 分页算术的 Kani 证明（需 Kani 工具链；约 1 秒）
```

以下任一情况出现，本报告的结论即失效，必须重跑并更新：

- 规格文件被改动（性质变弱、动作被放宽）——TLC 只证明当时那份规格；
- 代码级检查的覆盖率断言失败（游走不再走到某个关键状态）；
- 检查器灵敏度测试失败（检查器不再能发现人为破坏）；
- 门禁或 `verify-model-*` 出现违反；
- 有新发现的偏离（例如再出现"模型认为不可能、代码里存在"的状态）。
