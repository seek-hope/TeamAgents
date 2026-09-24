# 验收对照表（R2 重构，A01–A36）

基准：`docs/TeamAgents-Agent-System-Rebuild-Plan.zh-CN.md` §12/§16 与 D-42；当前实现于 **2026-09-24** 核对。
✅ = 所列路径有自动化证据，不代表场景中的每项发布条件均已证明；🔶 = 部分覆盖/存在已知缺口；⚠ = 尚未实现。

Rust 原生重构 R2-P0 于 2026-09-23 完成隔离故障/开销探针及最小真实 PTY→后台→重连检查；SQLite 为 3.53.2，
所有探针通过。它不改变下方旧产品 T1–T24 的实现状态，不含生产 kernel、真实模型或系统断电验收。
复跑命令、单机测量和边界见 [R2-P0 记录](../review/r2-p0-2026-09-23.md)；权威阶段范围见
[R2-P0 执行合约](archive/R2-P0-CONTRACTS.zh-CN.md)。

R2-P1 于 2026-09-23 落地小型 kernel 与直驱参考：`core/src/kernel` 无 I/O 状态转换、
`engine/src/providers` 单次尝试协议边（DeepSeek 原生字段保留）、基础工具统一入口产出结构化
`ToolReceipt`、追加式 JSONL 完整轨迹；`engine/examples/rebuild_p1.rs` 是评测组 A 参考循环。
确定性测试 22 项、假服务契约 7 项、新旧请求/工具行为对照 1 项全部通过；3 个真实 DeepSeek
任务（shell 验证、掩码输出 read_history 翻页、网页搜索+抓取）在参考循环上完成。
参考循环不带生产恢复承诺；持久化、多实例、MCP/Skills、其余供应商与正式性能实验仍属后续阶段。
证据与复跑见 [R2-P1 记录](../review/r2-p1-2026-09-23.md)。

R2-P2 于 2026-09-23 落地持久化单实例：`core/src/v2` 单会话单 SQLite（WAL+FULL、格式印记拒绝外来/错版）与
可信事务入口 `Control::submit`（命令/输入/信封去重、begin_request 修订检查、attempt 诚实记账、
import_response 原子消费、派发线性化点、取消持久化优先、预算闸门、批准自动过期）；
`engine/src/jobs` runner 以 READY/GO/CANCEL 握手执行 shell（重复 GO 去重、OUTCOME_UNKNOWN 恢复、
pid+boot_id+start_ticks 身份、抽象 socket+token 鉴权）；`engine/src/v2` 驱动按相位机推进，
模型/工具等待在事务外、状态转换全落库，支持输入/暂停/取消/批准干预与崩溃恢复。
故障注入 16 项（驱动 7、runner 8、spawn 1）全部通过；验证中发现的三处真缺陷
（预算拒绝事件随回滚丢失、批准后派发回执重放阻塞、spawn 失败驱动静默死亡）已修复并各有回归。
3 个真实 DeepSeek 任务经持久化驱动完成（文件验证、长输出问答、修 bug），证据与复跑见
[R2-P2 记录](../review/r2-p2-2026-09-23.md)；原始归档在 `review/eval/runs/2026-09-23-r2-p2-driver/`。
多实例、MCP/Skills、其余供应商、TUI 接入与正式性能实验仍属后续阶段。

R2-P3 于 2026-09-24 落地多实例：能力授权（范围授予/级联撤销/派发线性化点重查）、实例生命周期
（PAUSED/PARKED/TERMINATED 可运行性状态、重置 epoch 封闭、终止结清任务与回收授权）、收件箱与委派
（envelope_id 边界去重、背压公开失败、窄返回路径、终态结清消耗返回能力）、等待/唤醒（ALL/ANY+计时器、
注册即求值、事实产生点同事务唤醒、唤醒原因入上下文、阻塞诊断不误报普通环）、模型可见协作面
（spawn/delegate/send/wait 按授权注入 schema，原子 spawn 一事务创建实例+派生授权+初始任务）、
任务结清闭环（完成回合按存储候选结清任务、队列续跑 note、空队 close_completion）、多实例 supervisor
（发现循环驱动全部 ACTIVE 实例、共享单写存储 worker、输入按实例即时唤醒、全终止退出）与 A18 共享目标预算
（无目标实例按承接任务队列归属计费）。验证中发现并修复三处真缺陷（supervisor shutdown 挂起、WAITING
实例消息唤醒缺口、无目标完成回合卡死），各有回归。确定性证据：控制面新增 25 项、驱动 3 项、supervisor
端到端 4 项；`make check` 全绿——core 216 / engine 422 / tui 109。任意获准通信图可执行、单写上下文与
统一目标预算成立；真实模型验收、MCP/Skills、其余供应商、TUI 接入与正式性能实验仍属 P4 及以后。
证据与复跑见 [R2-P3 记录](../review/r2-p3-2026-09-24.md)。

R2-P4 于 2026-09-24 落地供应商协议层（R17）与 MCP/Skills（R18，进行中）：协议层与供应商名分离，
参照 pi-ai 的 model.api 路由按协议分发——chat-completions / responses / anthropic 三适配器 + AnyProvider
工厂（目录显式声明协议，凭据在配置边界解析，跨协议不转发旧原生块）；resolve_profile 把目录键解析为
线上模型 id 与选项快照，supervisor 按实例解析进 kernel，异构模型同会话各自完成回合。MCP 经 V2Toolkit
进统一回执契约：boot 时加载绑定服务（required 失败公开报错、optional 只丢能力）、schema 随请求注入
kernel profile、崩前已派发调用恢复后 OUTCOME_UNKNOWN 绝不重放（A25）；skills 的 search/read 经
basic_tool_schemas 广告、bindings 门控执行。R19-a v2 会话 daemon 与重连协议落地（greeting 先告、
checkpoint 快照+水位同一读事务、断线按水位续读），TUI 侧同步客户端就绪（R19-b①）。R20 完成检查
修复闭环落地：required_checks 仅用户/项目可预定义（机器契约不可冒充），检查骑乘同一 operation
回执账本、过检才落 SUCCEEDED、声明输入散列完成前重核、失败进修复或有界 BLOCKED、承认未交付
绝不升级。确定性证据：providers_fake 23+4、v2_supervisor 5、v2_mcp 6、v2_daemon 2、v2_driver 18、
core 契约 4；R19-b② TUI 对话主界面 v2 化落地：--daemon/--state-root 连接会话 daemon，
history 为权威对话流、事件驱动刷新，批准面板（daemon 新增 approvals 读面）、预算/状态条、
断开重连指示，提交输入 command_id 即 envelope。确定性证据补 v2_daemon 3、v2app 9、
真终端冒烟 pty_v2_smoke（进 make pty）；`make check` 全绿——core 220 / engine 458 / tui 121。
真实模型验收（§12）与实例/任务/授权面板（R19-b③）仍属本阶段及以后。
证据与复跑见 [R2-P4 记录](../review/r2-p4-2026-09-24.md)。

R2-P5 于 2026-09-24 推进可靠性（R21 进程与存储故障、R22 压缩/控制/协议竞态，进行中）。
R21 补齐：多供应商边界对齐 pi coding agent（429+配额/账单文本判永久错误、effort 归一）；
单一协调者锁（第二 daemon 拒启、shutdown 释放）、目标截止闸（过期拒绝新请求与新副作用派发）、
未知结果停放（OUTCOME_UNKNOWN 停放相关任务并逐任务通知）、Shell 服务跨 CLI/TUI 退出存活直测、
磁盘满分类（`Control::submit` 边界把 SQLITE_FULL 归类 StorageFull，驱动闩停派发并只重试停放）。
R22 落地 v2 上下文压缩（A20）：schema 1→2（`context_entries.compressed_by`、`model_requests.kind`，
v1 库单事务迁移、其余版本拒绝）；core 三命令 `begin_compression`/`compress_context`/`fail_compression`
（压缩请求与回合共用目标预算与截止闸门、不动执行位置与 revision、摘要单事务提交并重标覆盖段、
原文经 `read_history`/readback 始终可达）；driver 按真实窗口占用触发（真实 usage 与估值取大者、
90% 阈值减输出预留）、摘要调用计入目标预算并落制品、失败回退不压缩且连续三次熔断、
恢复释放遗留压缩预留；TUI 把压缩摘要作为独立条目显示。验证中发现并修复一处真缺陷：
校验完成后再起一回合、二次 finish 会把实例永久留在 COMPLETION_PENDING
（`complete_goal`/`block_goal` 现于同事务写下运行时收尾条目并让终态目标释放实例）。
A32 负载点在 v2 生产路径复测（250×20KB 合成历史 ≈1.26M 估算 token）：追加 p50 5.4ms、
整步四命令 p50 9.2ms、请求构造 p50 397ms、库增长 10.65MB（每步 42.6KB，无全量复制）、
重开 1.3ms、daemon 历史页 p50 24.3ms、1/4/16 并发只读页 29.8/45.6/256ms、追加后 RSS 9.1MB；
探针 `engine/examples/rebuild_p5_load.rs`，报告 `review/tmp/r2-p5-load/report.json`。
`make check` 全绿——core 228 / engine 465（3 ignored）/ tui 129。
真实供应商混用（A27）与长程/权限验收（R23）尚待授权执行：本机有 DeepSeek 与 OpenAI 凭据、
无 Anthropic，届时按真实/假服务分列。证据与复跑见 [R2-P5 记录](../review/r2-p5-2026-09-24.md)。

R2-P6 于 2026-09-24 完成性能实验（R24 预登记 → R25 试跑 → R26 三轮正式）：
`review/eval/r2-p6/`（design.md 预登记、三个冻结 manifest、run.py/analyze.py、REPORT.md）。
模型 DeepSeek Flash 原生 1M（D-36）、`full_auto`、每 trial 全新工作目录、验收在 trial 结束后同一目录执行。
四个批次共 **135 trial**（试跑 18 + 正式 72 + 第二轮 27 + 第三轮 18），**A/B/C 三组在每个任务的每次重复
都通过验收（135/135）**，合计约 3.60M 真实 tokens、约 55 分钟。结论：**H1（B 相对 A 未观察到退化）✅**；
**H2（C 相对 B 有可复现收益）未证实**——逐任务配对成功差恒为 0，且三轮 99 个 C 组 trial 中**零次
spawn/delegate**（事件与 tasks 表核验），协作能力可用但模型始终选择单人成队（方案对 C 的定义允许如此）。
成本：B 相对 A 约 +10%～+15% tokens，C 在此任务集上只增加开销。按 §13.2/§16，该结论符合预登记口径
（样本不足即标未证实），但**不能宣称协作收益**；要跑出收益需要远超本轮预算的任务规模（已记入 REPORT 局限）。
证据与复跑见 [R2-P6 报告](../review/eval/r2-p6/REPORT.md)。

R2-P7 于 2026-09-24 推进切换（R27–R29）：`teamagents init` 写配置并准备 **v2 状态根**
（默认 `$XDG_STATE_HOME/teamagents/v2`，格式/版本印记经 store 写入），`teamagents doctor` 校验 v2 根
（外来/错版库直接失败、`journal_mode=wal`、`synchronous=FULL`、读写探针）并把旧版布局列为只报告项；
daemon 默认根与 init/doctor 统一（socket `v2/daemon.sock`）。R28 按 §14 先出清单再删除：
`review/r28-legacy-cleanup.py` 清掉旧 v1 状态与探针临时数据 **3894 项 / 881MB**（0 失败），
保留凭据、`~/.agents/skills`、`~/.codex`、`review/eval/**` 证据、Git 历史与 `/tmp/tb21`；
清理后真实复验 init → doctor → daemon 全通过。R29 把**默认入口切到 v2**：`teamagents` 探测/启动当前用户
daemon 后进入 TUI（v1 TUI 路径不可达），`exec` 改为同一 daemon 的无头客户端（真实跑通：自动拉起 daemon、
`{"end":"reply","reply":"2"}`）；v1 入口 `validate`/`sessions`/`serve`/`--plain`/`--resume`/`--team` 明确拒绝，
旧测试按退役记录标注后**已随源码删除**（约 3.3 万行；逐项原因与 v2 等价覆盖见 R2-P7 记录的退役清单）；TUI 的 v1 半部删除并把 v2 需要的换行工具抽到 `wrap.rs`；`make pty` 收敛为 v2 真终端冒烟并通过；旧版基准文档移入 `docs/archive/`，当前文档只保留 v2 信息。
**验收矩阵 A01–A36：36/36 ✅**（A36 由本阶段闭环）。证据与复跑见
[R2-P7 记录](../review/r2-p7-2026-09-24.md) 与 [R2-P5 记录](../review/r2-p5-2026-09-24.md)。

退役完成后本机复跑 `make check` 全绿：core 243 / engine 130 / tui 29。计数低于此前阶段是预期的——
v1 源码与其测试在 R29 一并删除（约 3.3 万行），各项 v2 等价覆盖见 R2-P7 退役清单。

2026-09-22 按用户确认的 D-41 修复 full_auto Shell：主机环境执行、后台服务跨调用及 CLI 退出存活，
默认模式保留 bwrap；补齐实时模式切换、进程组停止、输出读取收尾和相同环境的 `exec --check`。
新增 6 项回归，`make check` 全绿：core 152 / engine 384 / TUI 109，engine 3 ignored。
Terminal-Bench 2.1 同一数据集快照、DeepSeek Flash high / 原生 1M、默认任务超时，预选的 6 个原失败任务
各复跑一次全部通过（0/6 → 6/6），普通 Docker 容器无需额外权限。没有重跑全量 89 题，
原 53/89 成绩保持独立，不据此推导新的总分或稳定通过率。证据于 2026-09-23 归档，
见 [修复与真实复测](../review/terminal-bench-fixes-2026-09-22.md)。

历史实测结果按当时日期保留；2026-09-17 补跑 DeepSeek 5 类真实任务与两项 PTY 检查，详见下方本轮记录。
2026-09-18 完成下载/首次配置优化的回归与公开下载实测，见 [安装验证记录](../review/install-2026-09-18.md)。
同日按 D-38 为 Worker 增加固定环境层，验证空配置与恢复、三种模型协议和 Codex 创建/恢复线程的指令注入；未调用真实模型服务。
同日发布 v0.1.2；公开制品下载、校验、安装、初始化和安装版 TUI 检查均通过，见
[发行验证记录](../review/release-v0.1.2-2026-09-18.md)。
2026-09-19 统一格式、严格 Clippy、开发检查入口与集成测试隔离；离线回归及两项 PTY 通过，见
[工程化记录](../review/engineering-2026-09-19.md)。
同日继续修复工作区恢复、成果保护与归档注册：新增 17 项真实 Git/文件工具回归，
见 [工作区生命周期记录](../review/workspace-lifecycle-2026-09-19.md)。未进行真实模型或竞品对照评测。
同日将最近一次工具 diff 替换为工作区首次观察快照审查：新增 19 项 engine、8 项 TUI 回归，
`make check` 与三项 PTY 全部通过，见 [工作区审查记录](../review/workspace-review-2026-09-19.md)。
这是有明确范围和上限的只读证据，不是完整版本历史或交付测试通过证明。
同日补齐成员移除的运行器/工具进程释放，并以生产会话入口验证 Chat/Codex 的 T24 身份续接与隔离；
新增 9 项 engine 回归，见 [成员生命周期与身份记录](../review/member-lifecycle-2026-09-19.md)。
模型与 Codex 协议使用本地假服务，不计入真实供应商验收。
同日增加用户只读的成员持久记录浏览：覆盖当前及已移除成员、对话树/线性快照、回合检查点、相关事件、分页版本保护、
符号链接与损坏记录拒绝，以及 TUI 迟到响应和窄屏导航；见 [成员记录浏览记录](../review/member-history-2026-09-19.md)。
这不是 Codex 完整外部历史、工具日志或内部推理的替代品。
同日将已选 Skills/指令文件通过受限的 `developerInstructions` 注入 Codex 成员，保持与 Chat 成员相同的
项目/用户 `AGENTS.md`、成员选定 Skill、每文件与总量上限及符号链接拒绝规则；见
[Codex 环境注入记录](../review/codex-skills-2026-09-19.md)。仍未进行真实 Codex/供应商行为验收。
同日完成 Chat 成员私有子代理：辅助回合不进入 TeamSpec，不继承父历史，不获得团队工具或递归子代理能力，
继承父成员的已绑定执行工具、工作目录、权限闸门和模型步骤预算；嵌套工具批准可恢复，工具回执在外部副作用
与嵌套历史写入之间以检查点日志保护。证据见 [Chat 私有子代理记录](../review/chat-subagent-2026-09-19.md)。
同日复核并修复任务等待的信息隔离：`wait_for_tasks` 现在要求调用成员是任务直接参与者、Leader 或匹配任务事件订阅的观察者；
立即返回与唤醒结果按当前 `event_types` / `payload_scope` 裁剪，无权的旧等待状态只返回 `UNKNOWN`。
任务结束或等待权限撤销时，即使没有 inbox 投递也能恢复等待回合。新增六项 core 回归，
`make check` 与三项 PTY 通过，见 [任务等待隔离记录](../review/wait-task-isolation-2026-09-19.md)。
同日补齐 B-03 投递时授权复核：持久队列、mid-turn 缓冲、Chat 历史写入与 Codex start/steer 发送前
共同检查原始受众及当前权限，失效投递留原因，降级投递不因再次授权而扩大；范围变更遵守接收者执行边界。
新增九项 core、三项 engine 回归，`make check` 与三项 PTY 通过，
见 [投递授权记录](../review/delivery-acl-2026-09-19.md)。未调用真实模型。
同日修复 Codex 冷恢复：初始化连接后精确匹配持久线程/回合，恢复结果与任务结算，
复用已提交的完成申请；ID 缺失、传输断线或服务中途退出不会触发自动重做，
未确认旧输入保留拒绝审计。新增 1 项 core、8 项 engine 回归，含实际 SIGKILL 与三类断线/坏回执。
真实 Codex 0.155.0 + DeepSeek Flash（用户确认的原生 1M）完成后冷恢复通过，
文件副作用恰好一次；复跑中发现并修复流式文本/历史文本导致的完成申请载荷冲突。
`make check` 与三项 PTY 通过，见 [恢复修复记录](../review/codex-recovery-2026-09-19.md)
和 [真实服务记录](../review/eval/runs/2026-09-19-codex-recovery/REPORT.md)。T17 仍不代表完整服务矩阵已验收。
同日完成 T6 专属实际请求验收，并先复现后修复两处工具读取越界：会话共享制品目录混入私有自动输出、
共享工作根覆盖运行时状态目录。自动输出改为成员私有读回，主动共享制品保持可用；旧无归属日志保留但拒绝成员读取。
新增六项 engine 回归，覆盖四成员私信、历史、观察范围、授权转发、恢复、新会话、图片加载和真实 bubblewrap 边界，
见[私有上下文隔离记录](../review/private-context-2026-09-19.md)。模型为本地协议夹具，没有新增真实供应商验收。
同日补齐方案 §5.2/§7 的共享附件与任务成果引用校验：拒绝私有上下文标识、历史/状态/配置路径及其符号链接；
完整检查引用数组，任务正常结束时复核旧申请与目标变化。拒绝保留审计，损坏数据或持久化失败使结算整体回滚。
新增五项 core 回归和一项普通/全自动模式的实际模型请求检查，见[成果引用记录](../review/output-references-2026-09-19.md)。
本地协议夹具验证拒绝后的正常交付，不新增真实供应商验收。
同日首次用相同多文件任务、DeepSeek Flash 原生 1M 和相同思考档位对照当前 TeamAgents 与 Codex CLI 0.155.0；
两边首轮均通过隐藏 11/11、公开测试和文件保护，发现 TeamAgents 遮蔽旧读回页时丢失来源/页码，
诱发逐层读回包装结果。修复及两项新增回归见[读回指针记录](../review/readback-pointer-2026-09-19.md)，
原始对照与修复版实跑见[真实任务记录](../review/eval/runs/2026-09-19-ledger-comparison/REPORT.md)。
单任务开发过程不证明通用排名，另外三个竞品与更大仓库仍待验证。
同日继续核对实跑中的 Shell 目录恢复错误：沙箱临时目录按调用重建，但恢复失败后旧实现仍执行命令，
且导出的旧 `PWD` 使目录提示失真。修复后跳过本次命令，告知下一次调用从工作区根目录开始，
保留普通导出变量；挂载与临时文件生命周期不变。新增两项真实 bubblewrap 回归，
`make check` 通过，见[Shell 目录恢复记录](../review/shell-cwd-2026-09-19.md)。
同任务原生 1M 复跑在 267.3 秒完成，隐藏 11/11、公开与最终文件保护通过；
三次目录恢复保护后仍可继续，其他失败回执与比较限制见
[真实复跑](../review/eval/runs/2026-09-19-shell-cwd/REPORT.md)。
同日扩展到固定历史提交的真实仓库任务：3 crate、约 2.9 万行 Rust，原始隐藏 2/7、
参考修复隐藏 7/7，公开 12/12。三次 DeepSeek Flash 原生 1M 实跑完整成功率 **0/3**：
一次步骤上限前未改代码，一次超时，两次最终候选有额外空日志。第二、三次的允许源码
在独立诊断副本上均通过隐藏 7/7 和公开 12/12，补充诊断不替换正式失败成绩。
第三次已提交 `goal_done` 却被 CLI 的历史 `FAILED` 判定覆盖；归档后修复该缺陷，
超时/待批准/未知结果/失败验收仍阻止成功。新增评分器 2 项、CLI 1 项回归，
runner 30 项契约及 `make check` 通过；CLI 修复后未追加真实模型复跑。
见[实现与校准记录](../review/repository-eval-2026-09-19.md)及
[三轮原始证据](../review/eval/runs/2026-09-19-repo-session-fork/REPORT.md)。
同日按 D-28/D-35 调整 Chat 的大窗口旧工具结果保留预算：`context_window` 已配置时使用
`clamp(window / 4, 16000, 256000)` 字节，未配置窗口仍为 16000；主成员、私有子代理、
压缩估算和溢出恢复统一采用该策略。主成员/私有子代理实际请求回归、UTF-8 上限与阈值检查通过，
见[上下文预算记录](../review/context-budget-2026-09-19.md)。同条件 DeepSeek Flash 原生 1M
各启动一次的新真实对照因本机 `/tmp` 配额耗尽中止，没有可评分 `result`，不计入完成率。
同日新增显式启用的 Chat 真实模型矩阵入口，五项离线契约覆盖三种流式线上格式、四个协议标签、
配置/原生窗口检查、隔离及失败证据。DeepSeek Flash 原生 1M / high 的文件工具、Shell 同会话续接、
关闭重开后的历史与用量恢复均通过；首跑的一次无效 `complete_task` 调用保留在失败工具计数中。
增加续接前工作目录为空的断言后再次独立实跑通过，防止以额外文件冒充历史恢复。
缺密钥反例写出 `incomplete/skipped` 报告并非零退出。见[矩阵入口与真实记录](../review/live-models-2026-09-19.md)；
不代表其他四家、混合供应商团队或全部远端工具已验收。
同日将原样 `repo-session-fork` 的工作区/状态/评分临时目录移至磁盘上的 `review/tmp/`，
用当前修复版新增一个完整成功样本：CLI **completed/0、943.746 秒**，固定公开 **12/12**、
独立隐藏 **7/7**，最终仅两个允许源码变化，受保护文件及权限保持完整。
DeepSeek Flash 原生 1M/high，共 128 次请求、149 次工具调用，无 `read_history`；
完整 Shell 结果仍有 9 次非零退出，额外全仓检查的失败保留。原三轮 **0/3** 与中止对照原样保留，
本批 **1/1** 不证明稳定成功率、单项修复因果或竞品水平。见[新样本证据](../review/eval/runs/2026-09-19-repo-current/REPORT.md)。
同日补做原样仓库任务的 Codex CLI 0.155.0 对照：首个尝试读到工作区外隐藏测试，立即启动中止处理并作废保留；
隔离读取范围后，以全新输入/会话正常完成 **0、472.672 秒**，独立隐藏 **7/7**、公开 **12/12**、
最终文件范围及权限通过。模型回合内的公开检查实际为 11/12，一项被 Codex 原生网络限制阻止；
独立评分使用与 TeamAgents 相同的原有 bubblewrap，不能把两种执行环境混写。
两边各一个有效交付通过样本；协议、工具、Skills、临时目录、网络策略和计时口径不同，
不作稳定性排名或因果提速结论。详情及污染证据见[仓库同题对照](../review/eval/runs/2026-09-19-repo-codex-comparison/REPORT.md)。
同日修复持久历史完整性：旧线程结构损坏不再静默当作空对话；循环/重复/缺失引用及跨分支摘要被明确拒绝，
恢复在模型请求前以已有 OUTCOME_UNKNOWN 停止并保留原文件，回退列表传出错误。历史祖先遍历改为线性，
`/rewind 0` 后压缩不再指向已放弃分支的根。新增 8 项回归，`make check` 通过。
DeepSeek Flash 原生 1M/high 新会话三阶段真实文件/续接/关闭重建通过，17 次请求、20.958 秒，
保留 1 次无效工具调用；不代表新增大仓库完成率或其他供应商验收。见[完整性修复记录](../review/history-integrity-2026-09-19.md)。
同日补齐方案 §5.1/§5.2 的 Leader 约束：导入、保存、动态变更和持久修订读取统一要求唯一且内置的 Leader。
三类非法动态变更在空闲/活动 Leader、内联/已有提案路径均拒绝，不半应用、不进入边界等待，拒绝回执重放稳定。
恢复时区分未保存过配置与已有非法记录；即使指定 `--team` 和全自动模式，也不覆盖坏记录或改动已有工作。
新增 8 项回归，`make check` 通过；未追加真实模型或 PTY 检查，见[Leader 校验记录](../review/leader-invariants-2026-09-19.md)。

同日补齐 Chat 冷恢复与回合归档窗口：真实 `serve` + SIGKILL 验证已保存工具结果、迟到补充、待批准操作和未知副作用；
注入 SQLite 归档失败后，复现并修复同一回合再次请求模型的问题。运行时保留原结果和投递确认，只重试核心事务，
提交前不发终态通知；已知 Chat 模型/协议失败先写检查点，归档失败并重启也不重问模型。
新增 7 项回归，`make check` 通过；模型为本地协议夹具，见[冷恢复与归档记录](../review/chat-cold-recovery-2026-09-19.md)。

同日实测 Python/Node 工具链：系统 Python 的 venv 与离线 wheel 流程原已可用；Node 安装/测试虽正常，
但默认 HOME 指向项目根，使 npm 日志与缓存被打入交付包。Shell HOME 改为成员私有持久目录，
工作区 MCP 使用临时私有 HOME；旧默认值迁移时保留项目文件和普通导出，新快照保留显式 HOME 设置。
新增五项真实 bubblewrap 回归，覆盖缓存续接/成员隔离、旧状态、npm 实际 tarball、Python venv/wheel 与 MCP；
见[工具环境与交付记录](../review/language-toolchains-2026-09-19.md)。没有新增真实模型或供应商验收。

同日为历史仓库评测增加显式夹具配置：`grading.toml` 的可选 `config_home` 只接受固定输入内的相对目录，
隐藏/公开评分用对应绝对 `XDG_CONFIG_HOME`，保持工具 HOME 隔离；公开命令与提示词采用相同环境。
新增两项回归覆盖未声明不继承、路径转义、受保护配置和越界拒绝。参考补丁重新通过隐藏 7/7、公开 12/12，
原始缺陷快照仍为隐藏 2/7；`make check` 通过。环境声明变化后预登记并顺序执行三轮全新样本，
完整交付 **3/3**：每轮 CLI completed/0、独立隐藏 7/7、公开 12/12、最终文件及权限保护通过，
CLI 耗时分别 **938.784 / 750.632 / 482.324 秒**。每轮仍各有 7 次 Shell 非零退出，额外全仓测试失败保留。
三轮期间 49 项输入哈希不变，候选补丁重放与独立评分哈希一致；没有合并模型产出的历史任务补丁。
本组只证明同一任务、同一环境的三次交付，不与历史 0/3、此前 1/1 合并，不代表通用完成率或竞品成熟度。
见[评分环境与重复记录](../review/repository-repeat-2026-09-19.md)和[三轮原始证据](../review/eval/runs/2026-09-19-repo-repeat/REPORT.md)。

2026-09-20 修复组队拒绝的配置污染：实际 Leader 才执行 profile 准备，不覆盖同名用户配置或已被
引用的会话 profile，准备中途/保存失败不安装候选；未使用的拒绝残留允许修正。拓扑操作增加
严格类型与未知字段检查，动态成员广播不能绕过 D-33。新增四项 core、五项生产会话 engine 回归，
`make check` 与三项 PTY 通过，见[组队校验记录](../review/topology-validation-2026-09-20.md)。模型为本地夹具；
D-30 跨存储非原子边界保留，最终拒绝仍可能留下未使用 profile，不将 T10 记为完整验收。

同日继续修复外层组队请求与提案决策：错误类型、未知字段和无提案的拒绝请求明确失败，准备前
复核身份/请求/版本；已有提案批准也补齐 profile、工具和通道默认值。提案读取按会话隔离，
损坏记录保留并报错，空的等待补丁不能推进版本。`Control::submit` 按原始请求记录去重回执，
准备后的操作仍在同一事务中校验和应用；成功、核心拒绝、准备失败均覆盖重开后的重放与参数碰撞，
并验证准备期间的版本竞态。新增六项 core、四项 engine 回归，`make check` 与三项 PTY 通过。
见[组队请求与重放记录](../review/topology-requests-2026-09-20.md)。JSON 协议和数据库结构未变，
旧版预处理回执不迁移，跨存储非原子边界仍保留；本地夹具不计入真实供应商验收。

任务结算补充：五个任务工具增加严格类型和未知字段检查，任务/回合引用及恢复辅助接口按会话限定。
迟到完成再次复核承接者、成员存在性、任务状态和成果引用，不覆盖已取消、已移交或已结清任务；
原 `OUTCOME_UNKNOWN` 回合仍可核对其自身任务，结算不复活已移除成员，也不改写成员新回合的忙碌状态。
新增十五项 core 回归及一项普通/全自动模式的 ChatRunner 参数修正、交付与重开检查，
`make check` 与三项 PTY 通过，见[任务边界记录](../review/task-boundaries-2026-09-19.md)。
本批使用本地协议夹具，T1–T24 仍为十八项有路径证据、六项部分覆盖，不新增真实服务验收。

通信与用户控制补充：补齐消息、共享空间、求助、目标完成、用户输入、批准、权限模式和暂停的
外层参数校验；错误类型不再转为正文或默认值，错误空间参数不会扩大读取范围。
共享读取按每个空间的已读位置合并分页，计数与最新序列不再截在前 1000 条；条目替代引用按会话和可见性检查。
新增十二项 core、两项 engine 回归，包含实际模型工具回执、普通/全自动交付与重开、真实 `serve` 输入拒绝。
`make check` 与三项 PTY 通过，见[动作请求与共享读取记录](../review/action-requests-2026-09-19.md)。
本地夹具不扩充真实服务矩阵；六项部分覆盖继续保留。

持久记录补充：任务/回合列表和事件/批准 JSON 损坏时明确报错，待投递记录不再因格式错误被当作撤权而丢弃；
成员视图、依赖调度和唤醒读取失败向上传播，批准过期失败使归档、停止超时和挂起取消整体回滚。
新增十项 core 回归及一项生产会话恢复检查，八类坏记录均在改权限或构建成员前被拒绝，修复后原回合正常交付。
`make check` 与三项 PTY 通过，证据及完整性检查范围见[持久记录验证](../review/stored-integrity-2026-09-19.md)。
本批不新增真实模型、SIGKILL 或发行证据，T1–T24 仍为十八项有路径证据、六项部分覆盖。

运行时存储故障补充：回合开始与成员视图读取合为现有核心事务，失败保留同一个排队意图；
成员返回后读取失败保留原结果，故障解除后继续归档。运行时诊断经 `runtime_errors` 传给 TUI，
重复轮询不刷屏；`exec --json` 的输入拒绝与存储错误返回失败，并跳过本次交付检查。
新增一项 core、三项 engine、一项 TUI 回归，`make check`、三项 PTY 与评测 runner 30 项契约通过，
见[运行时存储恢复记录](../review/runtime-storage-2026-09-19.md)。本批不新增真实服务或发行验收。

运行中投递补充：先复现两项“补充消息已受理、故障解除后下一次模型请求仍缺失”的问题，
核心现在在同一事务内返回消息与接收成员，运行时周期派送并重试投影错误，保留当前权限复核。
新增一项 core、四项 engine 回归，另覆盖成员工具消息在双方运行中到达，以及实际 SIGKILL 后
重新排队受阻时沿用原回合/工具结果恢复。`make check` 与三项 PTY 通过，
见[运行中消息记录](../review/mid-turn-storage-2026-09-19.md)。本地模型夹具不扩充真实服务矩阵。

排队取消补充：无外部回合 ID 的排队回合在同一核心事务中结清，成员视图读取失败不再阻止取消；
取消任务仅丢弃其尚未消费的就绪通知，保留同回合其他任务和普通消息，避免旧输入重新生成无任务回合。
新增六项 core、两项 engine 回归，覆盖整次回滚、旧取消请求重开、外部停止确认，以及结果等待读取恢复期间
正常关闭/实际 SIGKILL 后沿用原结果。证据与范围见[排队取消记录](../review/queued-cancellation-2026-09-19.md)。
本地夹具不新增真实服务验收；T1–T24 仍为十八项有路径证据、六项部分覆盖。

plain 与终态恢复补充：先复现失败回执误报已接收、存储错误阻塞行模式输入，以及迟到取消覆盖已返回结果。
plain 现在明确反馈拒绝/读取错误/存储等待，`status` 可显示新增事件；Chat 保留已知终态，
冷恢复先修复历史提交日志，再直接归档终态检查点，避免旧取消标记在重新排队时丢弃已有结果。
新增四项 engine 子进程检查，覆盖故障排除后的原输入继续，以及成功/失败结果在读取等待、归档重试、
取消和实际 SIGKILL 组合下保留。另扩展历史日志重放检查，见[修复与证据](../review/repl-finalization-2026-09-19.md)。
本地协议夹具不扩充真实供应商或复杂仓库任务成绩。

归档通知与旧队列恢复补充：核心在归档事务内返回实际状态和是否应用，钩子不再把已取消、
已恢复执行或已结清的旧结果误报为暂停。恢复先批量保护带检查点或外部回合 ID 的旧排队回合，
再核对结果；首次用户输入和开启全自动也遵守该顺序。准备失败整体回滚并重试，未开始的新意图
仍可执行，损坏检查点保留并进入结果不明。新增三项 core、六项 engine 回归，含实际 SIGKILL；
`make check`、三项 PTY 与 runner 30 项契约通过，见[通知与旧队列恢复记录](../review/recovery-state-2026-09-19.md)。
本地夹具不扩充真实服务验收。

执行证据与历史清理补充：旧 QUEUED 除检查点/外部 ID 外，还核对数据库开始事件；
检查点已丢失的 Chat 回合停止自动重做并报告结果不明，Codex 缺少外部 ID 也不新启回合。
恢复和清理共同校验开始载荷与身份，历史清理保留未结清回合（包含 OUTCOME_UNKNOWN）的开始记录；
候选读取、投递与分批事件删除在同一 savepoint 中，后段失败整体回滚。
新增四项 core、一项 engine 回归，并扩展 Codex 检查；`make check`、三项 PTY 和 runner 30 项契约通过。
见[恢复证据记录](../review/recovery-evidence-2026-09-19.md)。全部执行依据丢失时仍无法证明安全恢复，
本批不新增真实服务、陌生仓库或发行验收。

2026-09-20 补齐 Codex 成员原生记录入口：优先原生分页，首次明确不支持时按需读取受限旧 JSONL，
无文件路径时尝试完整历史接口。文件必须属于配置的 Codex 会话目录且线程身份匹配；拒绝符号链接、
错误存储模式、损坏或超大记录。原始条目允许无 ID，使用文件行号定位并保留正文。
新增六项 engine、一项 TUI 回归，覆盖分页/详情、移除后读取、拒绝不得降级、关闭和身份边界。
本机 Codex 0.155.0 的 legacy/paginated 两种存储均用人工数据经生产 worker 验证 40+5 条分页、
工具结果与状态不变；没有调用真实模型。见[原生历史记录](../review/native-history-2026-09-20.md)。
本批不改变 T17/T20 的部分覆盖状态，六项发布验收缺口保持。

2026-09-22 按 D-40 增加 `/model` 自定义供应商向导，支持 Responses、Anthropic、
Chat Completions 任一格式，配置保存和选择不依赖在线模型目录。
`make check` 与四项 PTY 通过，见[自定义供应商记录](../review/custom-providers-2026-09-22.md)。
协议请求与恢复使用本地假服务；未增加真实供应商验收。

## 验收矩阵 A01–A36（逐项证据）

| 编号 | 场景 | 证据 |
|---|---|---|
| A01 | 单 Leader 完成目标 | `v2_driver::end_to_end_shell_then_finish`；R2-P2 真实 DeepSeek 3 任务 |
| A02 | A→B→C→A 通信 | `control::messages_flow_across_an_authorized_ring` |
| A03 | 有限下授与父撤销 | `control::grants_narrow_only_and_parent_revocation_cascades` |
| A04 | 排队动作遭遇撤权 | `revocation_blocks_queued_dispatch_until_reauthorized`、`dispatch_rechecks_permission_revision` |
| A05 | 读取其他实例历史 | `control::read_history_is_user_or_self_only`；daemon history 读面 |
| A06 | 消息应用事务前后重启 | `submit_input_applies_context_once_per_envelope`、`command_replay_returns_stored_receipt_and_rejects_conflict` |
| A07 | 永久启动错误 | `fail_request_closes_and_parks_without_losing_input`、`v2_spawn_failure::*` |
| A08 | 成功后消费前崩溃 | `v2_driver::tool_result_is_reused_after_crash_not_reexecuted` |
| A09 | 未知外部结果 | `control::unknown_outcome_parks_running_tasks_and_notifies` |
| A10 | 重复派发/GO | `jobs_runner::duplicate_go_starts_exactly_one_command` |
| A11 | daemon/runner 分别崩溃 | `jobs_runner::daemon_crash_reconnects_the_same_job_without_restart` |
| A12 | Shell 服务跨退出存活（D-41） | `jobs_runner::a_successful_commands_service_outlives_the_job`；真实 `approved_scope` 6 次批准执行 |
| A13 | 取消/超时/完成竞态 | `jobs_runner::cancel_*`、`v2_driver::user_cancel_stops_a_running_job` |
| A14 | bwrap 不可用 | `tools.rs` IsolationUnavailable；沙箱内实测分类失败（无主机回退） |
| A15 | 环境身份 | runner 持久化 pid+boot_id+start_ticks 并核验（jobs_runner） |
| A16 | 必需检查失败 | `v2_driver::required_checks_failure_repairs_then_passes`、`required_checks_exhausted_parks_the_goal_blocked` |
| A17 | 检查后产物变化 | `v2_driver::check_inputs_must_still_hold_at_completion` |
| A18 | 多实例用量预算 | `control::a_worker_shares_the_budget_of_the_goal_its_queue_serves` 等 |
| A19 | 半条流与失联 | `providers_fake::truncated_stream_before_output_is_transient`、`providers_stall::*` |
| A20 | 长上下文压缩后重启 | `control::compression_*`、`v2_driver::long_context_compacts_before_the_turn_and_survives_a_restart` |
| A21 | 用户直接调整 Worker | `submit_input` 单写上下文 + TUI 目标切换 |
| A22 | ALL/ANY 等待环与计时器 | `control::blocked_report_flags_dead_waits_not_cycles`、`a_due_timer_closes_the_wait` |
| A23 | 结果先到后注册等待 | `control::wait_for_an_arrived_result_is_satisfied_at_registration` |
| A24 | 重置后旧结果迟到 | `control::late_receipt_after_reset_lands_on_the_old_epoch_only` |
| A25 | MCP 批准/取消/未知 | `v2_mcp` 六项 |
| A26 | Skills 权限 | `v2_mcp::skill_call_without_the_binding_fails_honestly` |
| A27 | 异构供应商合作 | 真实：DeepSeek + Kimi 同会话双向真实消息（`review/tmp/r2-p5-a27/report.json`）；假服务：`v2_supervisor::heterogeneous_*` |
| A28 | 断连/慢客户端/重连 | `v2_daemon::handshake_checkpoint_command_and_goal_completion`、`reconnect_backfills_events_after_the_watermark` |
| A29 | 会话隔离/共享项目 | `control::begin_request_rejects_instances_of_other_sessions` |
| A30 | 制品与 DB 写入断点 | `control::artifact_staging_gc_and_publication_ordering` |
| A31 | 写入失败/磁盘满 | `control::disk_full_is_classified_at_the_submit_boundary`、`v2_driver::disk_full_stops_dispatch_reports_and_resumes_after_parking` |
| A32 | 超大历史测量 | `engine/examples/rebuild_p5_load.rs` + `review/tmp/r2-p5-load/report.json` |
| A33 | 双 daemon/旧锁 | `v2_daemon::second_daemon_is_refused_and_shutdown_releases_the_lock` |
| A34 | schema 不兼容 | `core::v2::store::open_refuses_unstamped_foreign_and_wrong_version`、`open_migrates_the_previous_schema_version` |
| A35 | 目标截止时间 | `control::goal_deadline_refuses_new_requests_and_dispatches`、`v2_driver::goal_deadline_parks_the_instance` |
| A36 | 安装/init/doctor/清理/重开 | `cli::init_prepares_the_v2_root_and_doctor_verifies_it`；R28 清理 + 真实复验（`review/tmp/r28/`） |

## 当前实现与重构前的差异（要点）

- 旧版 `TeamSpec`/成员 runtime/Codex 成员/行模式/会话恢复入口全部退役（见
  [R2-P7 记录](../review/r2-p7-2026-09-24.md) 的退役清单）；团队由 Leader 通过
  `spawn/delegate/send/wait` 建立，会话由 daemon 拥有。
- 唯一权威状态是每会话单 SQLite（`core/src/v2`）；旧 `sessions/` 布局不迁移、按 §14 清理（已执行）。
- 默认入口：`teamagents`（自动拉起 daemon 的 v2 TUI）与 `teamagents exec`（同一 daemon 的无头客户端）。
- 用户钩子 `[hooks]`（`notify` 事件通知、`pre_tool` 工具前拦截）在 v2 运行路径生效；doctor 不再探测
  本机 Codex CLI（v2 无 Codex 成员类型）；v1 控制面（`core/src/{control,storage,views,server}.rs`、
  `teamagents-core` stdio 二进制与其测试）已删除，core 用例 243 → 91。见 D-45。
- 工作区策略（§12.3/Q14）已接线：`spawn` 的 `workspace` 参数为 `shared`/`isolated`/`git_worktree`，
  策略在实例启动前记录、实例终止时按其回收（有未提交或未合并成果时拒绝删除并报告）。
  证据：`engine/tests/v2_driver.rs::spawn_resolves_the_requested_workspace_policy`、
  `engine/tests/v2_supervisor.rs::terminating_an_instance_retires_its_workspace`。见 D-46。
