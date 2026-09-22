# 验收对照表（T1–T24）

基准：方案 §17；当前 Rust 实现于 **2026-09-22** 核对。
✅ = 所列路径有自动化证据，不代表场景中的每项发布条件均已证明；🔶 = 部分覆盖/存在已知缺口；⚠ = 尚未实现。
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

## 当前离线基线

在仓库根目录运行（本次 Rust 1.95.0，bubblewrap 与 python3 可用）：

```bash
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml
cargo test --offline --manifest-path tui/Cargo.toml
```

| crate | Cargo 报告通过 | 组成 / 实际执行范围 |
|---|---:|---|
| core | 152 | 25 库单测 + 63 控制场景 + 64 投递/未知输入/成果引用/任务边界/动作请求/持久记录集成测试 |
| engine | 378 | 110 库单测 + 1 CLI 单测 + 267 集成测试；另有 3 项显式 ignored（`eval_grader` 两项、`live_models` 真实入口一项）；`live_codex` 未设开关时提前返回，2026-09-19 已单独运行 Codex + DeepSeek 的真实恢复检查 |
| tui | 109 | 14 库单测 + 11 CLI 单测 + 39 app + 6 history + 31 render + 8 review |

**CI（GitHub Actions，2026-09-15 起）**：`Test core` / `Test engine` / `Test tui` / 行尾空格检查全绿。
runner 上装了 bubblewrap 也用不了（内核/AppArmor 限制非特权 user namespace），因此依赖真实隔离的用例
（`chat_e2e` 4 条、`codex_contract` 1 条、`mcp` 的 workspace 路径及隐藏评分器的真实执行部分）会打印 `skipped: bwrap is unavailable`
自行跳过；它们的权威验证在有 bubblewrap 的开发机上。回归测试自身已与开发机解耦
（不再读 `~/.config/teamagents/config.toml`，也不要求本机装 codex）。
隐藏评分入口在隔离不可用时始终判失败；仅回归测试在验证该失败行为后跳过候选执行部分。

**skip 不计入真实验收**：部分测试在缺依赖/开关时直接 `return`，Cargo 仍显示 passed。
真实 Codex 检查需显式运行以下命令；Chat 真实矩阵用 `live_models` 的 ignored 入口，
配置、清单与证据目录见[开发说明](DEVELOPMENT.md#真实-chat-模型矩阵)。未列入清单或跳过的供应商不计通过。

```bash
# 使用所选模型的原生上下文；这里为用户已确认的 DeepSeek Flash 1M。
TEAMAGENTS_LIVE_CODEX=1 \
TEAMAGENTS_LIVE_CODEX_CONFIG="$HOME/.codex/deepseek.config.toml" \
TEAMAGENTS_LIVE_CODEX_CONTEXT_WINDOW=1000000 \
cargo test --offline --locked --manifest-path engine/Cargo.toml --test live_codex -- --nocapture
# 真终端检查另行运行，须先构建 engine 与 tui
python3 tui/scripts/pty_smoke.py
python3 tui/scripts/pty_click_check.py
python3 tui/scripts/pty_review_check.py
```

## T1–T24 证据

| ID | 场景 | 状态 | 证据 / 说明 |
|---|---|---|---|
| T1 | Leader 委派给 B 并汇总 | ✅ | `engine/tests/scenarios.rs::t1_delegation_and_summary_full_lifecycle` |
| T2 | B/C 并行；慢任务不阻塞 Leader | ✅ | `scenarios.rs::t2_parallel_members_and_mid_run_supplement`（含执行中补充） |
| T3 | B/C 讨论；重复投递不重复注入；越界阻止 | ✅ | `scenarios.rs::t3_channel_enforcement_and_exactly_once_delivery` |
| T4 | 观察者只收到授权事件与载荷 | ✅ | `scenarios.rs::t4_observer_scoped_events_without_extra_rights`；任务等待补充三种 scope、单事件订阅与未订阅结果回归；`core/tests/delivery_acl.rs` 及 Chat/Codex 请求检查补齐 B-03 撤权/降级后旧投递复核、重新授权不扩大旧载荷。见 [任务等待](../review/wait-task-isolation-2026-09-19.md) 与 [投递授权记录](../review/delivery-acl-2026-09-19.md)。使用本地假服务，真实供应商边界另行验收 |
| T5 | 共享空间发布/发现/权限/引用 | ✅ | `scenarios.rs::t5_shared_space_permissions_and_discovery`；`core/tests/output_references.rs` 五项覆盖私有标识/路径拒绝、严格数组校验、结算复核、损坏记录及事务回滚；`private_context.rs::private_output_references_are_refused_and_corrected_through_model_tools_in_both_modes` 检查普通/全自动模式的实际模型回执、正常交付及接收者请求。见[成果引用记录](../review/output-references-2026-09-19.md)。新增 `action_requests.rs` 与生产 ChatRunner 检查补齐错误空间参数拒绝、逐空间游标分页、千条以上统计、替代引用与重开回执，见[动作请求记录](../review/action-requests-2026-09-19.md) |
| T6 | 信息隔离；新会话不继承 | ✅ | `engine/tests/private_context.rs` 六项经生产会话入口检查 A/B/C/Leader 的实际模型请求：A→B 私信、私有工具输出/指令不自动进入 C/Leader；伪造历史参数无效，同成员恢复与新会话隔离；观察授权/转发、私有输出读回、图片重新加载、旧日志拒读与目录重叠拒绝。真实 bubblewrap 隐藏其他成员历史、状态和环境哨兵。模型为本地夹具；不覆盖恶意宿主进程或所有外部后端。见[专属记录](../review/private-context-2026-09-19.md)，任务等待/撤权仍由此前 core、Chat/Codex 回归补充 |
| T7 | 五家模型工具调用与续接；同队混用 | 🔶 | 已有 DeepSeek 单 Leader 实测；新增 `live_models.rs` 统一原生窗口入口，DeepSeek Flash 的文件工具、Shell 续接、会话重建及持久用量检查通过。其余四家与混合供应商团队尚未实跑；三种流式格式的本地契约不代替真实服务。见[本批记录](../review/live-models-2026-09-19.md) |
| T8 | 恢复：杀进程后重建 | ✅ | `engine/tests/recovery.rs` 保留原脚本成员去重/取消检查，并新增 7 项 Chat/核心归档回归：真实 SIGKILL 后的工具结果、补充消息、待批准与未知副作用；归档失败时同一回合不再请求模型，已完成/失败结果可从检查点补归档，暂停时继续重试。模型为本地夹具；见[记录](../review/chat-cold-recovery-2026-09-19.md)。后续补齐旧 QUEUED 丢失检查点后的开始事件核对、显式/自动清理保护和批量清理回滚，见[恢复证据](../review/recovery-evidence-2026-09-19.md)；全部执行依据丢失的限制仍保留 |
| T9 | 单 Leader 可执行并继续对话 | ✅ | `scenarios.rs::t9_baseline_leader_alone_executes_and_keeps_talking` |
| 跨后端组队（Codex 成员） | `review/eval/tasks/team-codex/`：Leader（Chat）委派给 `runtime_kind: codex` 的成员，成员经 `codex_profile` 跑在 DeepSeek 后端（`codex app-server -c ...` 展开 `$CODEX_HOME/<name>.config.toml`，不使用官方订阅），真实运行 completed/exit 0/37.9s/验收通过（`review/eval/runs/2026-09-15-deepseek-codex/`）。顺带修：deltas 与 `item/completed` 的文本合并（原先词间空格 + 重复）、app-server 退出错误带 stderr 尾部 | 组合边界也验过：`team-codex-gate`（工作目录之外的写入 → app-server 请求批准 → 引擎 PENDING → 非交互 exit 3，写入从未发生、也无伪造记录）与 `team-codex-interrupt`（派给 Codex 成员的 sleep 被超时打断：run.txt 只有 started、两个回合都记为取消、无残留进程），证据 `review/eval/runs/2026-09-15-deepseek-codex-gates/`。
| T10 | 自然语言组队；非法结构被拒绝 | 🔶 | 核心校验与 `validate`；`chat_e2e.rs::review_add_agent_auto_creates_member_profile` 覆盖 D-30 自动 profile、`review_add_agent_inherits_leader_tools_and_gets_channels` 覆盖 D-33 默认值（省略 `tool_bindings` 继承 Leader 绑定、显式 `[]` 保持空、自动双向 message 通道、成员间通道被拒）。2026-09-20 新增九项回归覆盖越权/重复 ID 的配置保护、准备和保存失败后重试、残留 profile 修正、严格操作结构与广播限制，见[组队校验记录](../review/topology-validation-2026-09-20.md)；后续十项回归补齐外层请求、准备前校验、提案批准和回执重放，见[请求记录](../review/topology-requests-2026-09-20.md)。真实自然语言组队已跑：`team-collab` 首轮暴露"成员无执行工具 + 补丁形状靠猜"导致 900s 超时，修好工具契约与成员 `tools` 可见性后同一提示词 40s 通过（36 次工具调用、0 失败，见 `review/eval/runs/2026-09-15-deepseek/`）。未知 profile 按 D-30 解释为模型 ID，不保证远端存在；受支持团队动作的外层字段已严格校验；提案操作仍在应用阶段校验，真实多模型组队与 D-30 跨存储边界继续保留 |
| T11 | 动态变更：成员只能提议、Leader 应用、边界生效 | ✅ | `engine/tests/topology.rs::t11_member_proposal_is_leader_decision`（提案→Leader 应用→生效，审计保留提案人）；`chat_e2e::topology_request_stored_proposal_inherits_leader_defaults_when_applied` 补齐生产会话中省略/空操作的提案批准、默认值与重开，模型为本地夹具 |
| T12 | 版本冲突不互相覆盖、不半应用 | ✅ | `topology.rs::t12_conflicting_patches_never_partially_apply`；`core/tests/engine.rs::topology_patch_rejects_*` 覆盖新增第二 Leader、提升普通成员为 Leader、将 Leader 切换为 Codex 的拒绝；先行有效操作不落库，活动回合、配置版本和提案状态不变，重放拒绝后仍可正常变更/派发。见[Leader 校验记录](../review/leader-invariants-2026-09-19.md) |
| T13 | 移除成员：停止后移除、任务移交、成果保留 | ✅ | `topology.rs::t13_removed_member_hands_tasks_to_leader_and_keeps_results`；`member_lifecycle.rs` 6 项补齐安全边界后后台释放、暂停态释放、慢关闭不阻塞 Leader、退出等待清理、MCP 进程组/握手失败回收与成果保留；`session_identity.rs` 另验证 Codex 移除后停止 app-server。`core/tests/task_boundaries.rs` 补齐移交的会话边界、迟到完成不覆盖移交任务及不复活已移除成员，见[任务边界记录](../review/task-boundaries-2026-09-19.md) |
| T14 | 执行中补充；仅相关成员按边界调整 | ✅ | `scenarios.rs::t2_...`（supplement 到运行中的 Leader）；`recovery.rs::mid_turn_input_retries_after_*` 检查状态/投影故障恢复后的实际模型请求，原回合不遗漏、不重复补充；`member_tool_messages_reach_a_running_peer_before_the_sender_finishes` 检查双方均运行时的工具消息。模型为本地夹具，见[运行中消息记录](../review/mid-turn-storage-2026-09-19.md) |
| T15 | 工具批准：自动/越界暂停/其他成员继续/拒绝 | ✅ | 审批门单测 + Codex 批准 park/decide 用例；ChatRunner 端到端：`engine/tests/chat_e2e.rs::once_approval_is_consumed_and_the_turn_completes`（once 执行后消费）、`expired_once_approval_requires_a_new_request`（EXPIRED 重请求）、`denied_approval_blocks_the_operation`；Codex 超时闭环 `codex_contract.rs::codex_approval_timeout_expires_the_row` |
| T16 | 全自动只能用户开启；仍守系统权限与 ACL | ✅ | `scenarios.rs::full_auto_toggle_reaches_the_approval_gate`；仅 `actor=user` 可改模式（`core/src/control.rs` 校验） |
| T17 | Codex 成员：任务/进度/结果/批准/取消/恢复映射 | 🔶 | `codex_adapter.rs`（simple/approval/slow/退出）、`codex_contract.rs`（精确回合、steer 参数、批准、进程组）；`codex_recovery.rs` 七项覆盖生产会话冷恢复、实际 SIGKILL、结果申请复用、断线/缺 ID 不重放、失败原因及旧批准失效。`live_codex.rs` 已显式验证真实 Codex 0.155.0 + DeepSeek Flash 原生 1M 的完成后冷恢复与文件副作用一次，含复跑中发现的真实缺陷修复。见 [记录](../review/codex-recovery-2026-09-19.md)。完整供应商矩阵及真实模型下全部异常/批准组合仍未验收 |
| T18 | 工作目录 shared/isolated/worktree；脏输入不被忽略 | ✅ | `engine/src/workspace.rs` + 单测；`engine/tests/workspace_lifecycle.rs` 的 17 项真实 Git/会话回归补齐主目录变脏后的工作根稳定、跨会话分支唯一、detached HEAD/忽略文件/用户分支保护、删除全量预检、归档注册修复/失败回滚/同名拒绝、旧会话兼容与锁保护。`tui` 删除守卫同源；不承诺跨目录崩溃原子性或外部编辑器并发写入下的完整保护 |
| 多模态输入 | `view_image`（`files` 绑定）读取 png/jpeg/gif/webp（≤5 MiB，按魔数判类型，越界/超限/非图片都拒绝），结果只存路径引用，请求构建时按协议转成图片内容：chat completions → user 消息 `image_url`、Anthropic → `tool_result` image 块、Responses → `function_call_output` 的 `input_image`。回归 `chat_e2e::view_image_attaches_the_picture_to_the_next_request`、`chat::tests::image_references_become_protocol_image_parts`、`tools::tests::images_are_classified_by_magic_bytes_and_bounded`。真实视觉模型的端到端效果待有对应订阅后验收 |
| T19 | 工具生态：文件/Shell/搜索/抓取/MCP/Skills 真实任务 | 🔶 | files/shell/web_search/web_fetch；`live_models` 新增 DeepSeek 真实文件/Shell 任务证据。MCP stdio（`engine/tests/mcp_tools.rs` 启动本地测试服务器、`mcp_stdio.rs` 检查环境白名单/stderr）与 HTTP（`mcp_http.rs` 本地模拟服务）；Chat Skills/AGENTS.md 注入（`session.rs` 单测）；web fail-closed（`tools_sandbox.rs::web_tools_are_fail_closed_and_ordered_by_member_binding`）、长输出 artifacts（`long_shell_output_is_stored_as_a_readable_artifact`）；不等于所有工具已完成真实远端任务 |
| T20 | TUI：流式期间输入/导航/批准可用；窄屏、多行中文 | 🔶 | TUI 106 项单测 + TestBackend 帧；后台请求使用有界队列，停滞 worker 回归验证输入/退出仍响应。成员记录浏览覆盖导航、分页、刷新、窄屏、迟到响应及 Codex 原生页/详情身份；审查含中文、控制字符转义和迟到响应保护；存储等待错误有去重、恢复和再次故障的中英文帧回归；三项无模型 PTY 验证输入/点击/实际文件审查。真实模型 + 真界面仍需服务验收 |
| T21 | 崩溃去重：动作回执丢失仍只产生一次变更 | ✅ | `recovery.rs::t8_...` 断言重放步骤不重复产生副作用（shared 条目仍为 1 条）；`gateway::tests::prepared_topology_*` 覆盖组队准备前后请求的稳定回执、重开后成功/拒绝重放、参数碰撞与版本竞态，见[请求记录](../review/topology-requests-2026-09-20.md)。数据库重开检查不是新增真实模型/SIGKILL 证据 |
| T22 | 资源与失败：限流/超时/成员失败/无人就绪/超限 | ✅ | `recovery.rs::t22_goal_turn_budget_is_enforced`（LIMIT_REACHED）+ 取消/暂停场景 + 模型步数上限 `chat_e2e.rs::model_step_limit_reports_limit_reached`（超限 → `limit_reached` + FAILED）+ 活动超时中断 `timeout_interrupts_the_member_before_further_side_effects`、崩溃不误报超时 `a_crashed_member_is_not_reported_as_a_timeout` |BLOCKED 任务（成员回合被中断所致）由 Leader `cancel_task` 结清后重派，承接者的 `complete_task` 会被拒绝并提示该路径（回归 `core/tests/engine.rs::blocked_tasks_are_recoverable_by_the_leader`）；
| T23 | 权限执行：穿越/符号链接/Shell 越界/MCP 未授权 | 🔶 | `tools.rs` bwrap/路径/版本冲突回归，MCP workspace 模式目录与网络默认隔离，显式 host 才可离开沙箱；远端 HTTP 仍由服务授权，不能宣称覆盖服务端权限 |
| T24 | 会话复用；同名新成员不继承旧身份 | ✅ | `engine/tests/session_identity.rs` 两项专属回归：Chat 重开续用实际文件工具历史/模型覆盖，Codex 只对同会话同成员 `thread/resume`；同名新 ID、新会话同 ID 使用独立上下文，移除墓碑重启后仍拒旧 ID。走生产 `open_session` + 本地假模型/app-server，不代表真实外部服务验收；历史浏览只读取对应会话记录，不改变身份边界 |

**仍待完整验收**：Codex/Chat 五家真实服务的完整发布矩阵，以及真实模型下的异常/批准/TUI 组合。
Codex 对话与工具记录已有按需只读入口；只展示后端实际提供或已保存的内容，不创建外部历史副本，
也不能还原缺失记录或后端未提供/加密的内部推理。版本、文件与分页边界见[原生历史记录](../review/native-history-2026-09-20.md)。
客户端声明 `roots` 能力并应答 `roots/list`（返回成员工作目录），其它服务器发起的能力回 `-32601`（回归 `mcp::tests::roots_list_is_answered_and_other_requests_are_declined`、`mcp_http.rs` 的推送流用例）；MCP HTTP（streamable，2025-06-18）：POST 响应中的 SSE、GET 主动推送流与 DELETE 会话终止均已实现——GET 流建立后服务器通知写日志，服务器发来的请求（如 sampling/roots）按规范回 JSON-RPC 错误而不是让服务器干等；关闭时 DELETE 结束会话；不支持推送的服务器回 405 时按无推送处理（`mcp_http.rs::http_push_stream_answers_requests_and_deletes_the_session`、`http_transport_tolerates_servers_without_push_or_delete`）。旧式独立 SSE 传输已从规范移除，绑定直接报错。
TUI 为 Rust 原生设计（D-20：固定分区、滚动、胶囊状态）。其余取舍见
`docs/DECISIONS.md`（D-17/D-19/D-20/D-21）。

## 当前实现与方案的已知差异

以下是代码现状记录，**不表示新偏离已获批准，也不将原方案的发布条件改为已完成**。

| 范围 | 当前行为与证据 |
|---|---|
| 通信与用户控制请求（§5.2、§7、§9） | 受支持团队动作的外层参数要求 JSON 对象，拒绝错误类型与未知字段；`serve user_message` 同样严格检查文本及布尔补充标记。共享读取按空间游标合并分页，存储错误不重置游标、不返回伪空页；统计使用实际 COUNT/MAX。`supersedes` 必须指向本会话可访问条目，求助关联任务须属于本会话。十二项 core、生产 ChatRunner 与真实 serve 回归见[动作请求记录](../review/action-requests-2026-09-19.md)；其他管理接口及执行工具参数未在本批全面校验 |
| 模型流式与协议完整性（§11、T7） | `engine/src/stream.rs` 支持三种线上格式（chat completions / Anthropic Messages / OpenAI Responses）的 SSE 与单 JSON 响应，保留 thinking/signature 与 usage；`chat.rs` 为 Responses 做双向翻译（`instructions`、工具扁平化、`function_call`↔`tool_calls`），回归 `chat_e2e::responses_protocol_round_trips_a_tool_call` 与 `stream::tests::responses_stream_yields_text_calls_and_usage`。真实订阅闭环仍待各自凭据 | 固定提示开销有预算断言（系统提示只列工具名、描述随 schemas 走，`chat::tests::prompt_overhead_stays_lean` 钉住 3227→471 字符）；
| 动态变更安全边界（§8） | `core/src/control.rs::agent_has_live_run` 不把无 `external_turn_id` 的 WAITING_TASK/WAITING_APPROVAL 算作活动执行，因此 Chat 挂起时可应用 patch；有外部回合 ID 的 Codex 等待仍阻塞。现行修复证据：`core/tests/engine.rs::approval_parked_run_does_not_block_boundary`、`task_wait_parked_run_does_not_block_boundary` |
| 回合归档失败（§9.2） | `Runtime` 保留成员已返回的结果与投递确认，核心事务失败后间隔一秒重试，提交前不发终态通知、不重启原回合；暂停态也重试。进程内队列不是第二份业务权威，进程退出后仍依赖后端持久证据。Chat 的完成/已知模型失败有终态检查点，无结果的外部副作用仍为 `OUTCOME_UNKNOWN`。`recovery.rs` 以可解除 SQLite trigger 故障验证，不承诺数据库损坏修复或通用外部 exactly-once |
| 运行时存储等待（§9、T8/T22） | `prepare_run` 将开始记账、成员视图和唤醒读取放在同一事务，失败保留原排队回合；返回结果后的状态读取失败保留原结果，恢复后再核对取消并归档。恢复核对失败会重试。诊断 `runtime_errors` 是瞬态用户反馈，TUI 去重显示，`exec --json` 遇输入拒绝或运行时错误返回失败且跳过验收命令。证据及尚未覆盖的故障组合见[本批记录](../review/runtime-storage-2026-09-19.md) |
| 运行中消息交接（§6.3、§7、§9） | core `MidTurnPush` 将路由与权限投影放在同一事务，失败保留整批；运行时不在排空后再读完整状态，周期派送并重试，`delivery` 诊断说明阻塞原因。后端注入前仍复核权限，投递确认不变；新增 Chat 实际请求及重新排队故障恢复检查见[记录](../review/mid-turn-storage-2026-09-19.md) |
| 排队取消与输入处置（§6.3、§7、§9） | `CancelRun` 对无外部回合 ID 的 QUEUED 直接结清并记录未消费输入的失效原因，不依赖成员视图；`CancelTask` 只移除目标任务的待投递就绪通知，其余消息/任务仍可调度。批准、任务、回合与输入在同一事务提交或回滚；旧 QUEUED 取消请求经 schedule 收敛。已有外部回合继续等待停止确认。检查、结果读取等待期间退出证据及未覆盖边界见[记录](../review/queued-cancellation-2026-09-19.md) |
| 任务请求与迟到结算（§5.2、§5.3、§6.2、§7） | `assign_task`、`complete_task`、`wait_for_tasks`、`cancel_task`、`cancel_run` 拒绝错误类型及未知字段；任务、依赖、父任务和回合引用限定当前会话。结算复核当前承接者、成员、状态及引用；终态保持幂等，已移交任务不被旧申请覆盖。原结果不明回合只可核对其自身仍归该成员的 BLOCKED 任务；同成员其他新回合的状态保持。读错误或审计写入失败使结算事务回滚；十五项 core 与一项生产 ChatRunner 本地夹具检查见[记录](../review/task-boundaries-2026-09-19.md)，不代表所有管理协议或损坏数据库行已全面严格化 |
| 移除后的资源生命周期（§8、T13） | 调度器根据已提交 TeamSpec 的成员集合回收运行器，等待该成员回合包装线程退出后后台 `close`，暂停态也清理；会话退出 join 清理线程。用量探针不再强持有旧运行器，换模型的空档保留最后计数。stdio MCP 关闭/初始化失败终止进程组并释放请求；HTTP 关闭只 DELETE 一次、拒新调用。已有 HTTP 请求仍受原超时约束，主动脱离进程组的宿主服务子进程不在保证内 |
| 网关与 MCP 隔离（§12.2） | `BoundTools::load_in` 将 stdio MCP 默认放入成员 workspace bwrap（无网），`mcp_execution = "host"` 才显式使用宿主；绑定仍是授权边界 |
| 全自动与越界批准（§12.2） | 原生文件工具仍由 `tools.rs::resolve_in_root` 限定路径，Shell 始终走 `shell_run_with_control` / `bwrap_argv`；full_auto 只跳过批准门，没有扩大文件根或取消原生 Shell 沙箱 |
| 多文件批量编辑 | `edit_files`（`files` 绑定）：先对全部文件做唯一匹配、大小与 `expected_sha256` 校验，按排序持有全批路径锁，暂存新旧内容后再次核对版本，逐文件原子替换；准备失败不提交，运行错误撤销已提交编辑。同一文件一次只允许一条编辑。回归 `batch_edits_are_all_or_nothing`、`batch_edits_reject_oversized_results_before_changing_any_file`、`batch_edits_recheck_all_versions_after_waiting_for_locks`、`batch_commit_rolls_back_when_a_later_rename_fails`。不承诺跨文件崩溃事务或对外部读者同时可见；外部写入/磁盘故障使回滚失败时明确报告并保留恢复副本 |
| 共享目录并发写（§12.3） | `tools.rs::workspace_executor_with_control` 提供进程内路径锁、SHA-256 CAS 与原子替换；`tools.rs::with_path_lock` 追加跨进程建议锁（锁文件在会话状态目录 `sessions/<id>/locks/`，不落项目目录，`File::try_lock` 争用等待上限 10s，FS 不支持时退化为进程内互斥），回归 `tools::tests::path_lock_serializes_two_writers`。外部编辑器（不走本工具的写）仍建议使用 worktree |
| Skills 后端范围（§10、§12.1） | `session.rs::make_runner_factory` 为 Chat 与 Codex 共同计算有界、符号链接安全的 `member_context`；Chat 注入 system prompt，Codex 注入 `thread/start`/`thread/resume` 的 `developerInstructions`。Codex 仍不使用其自身的 Skills 发现协议，也未完成真实服务行为验收 |
| Codex 冷恢复与未知输入（§9.2、T17） | 只读核对保存的线程/回合，终态结果经相同任务完成事务归档；已提交的完成申请保持原载荷。传输断线与明确 RPC 拒绝分开处理，无可靠外部终态进入 OUTCOME_UNKNOWN；仍 pending 的关联输入留 `dropped_reason`，不伪记消费、不自动创建替代回合。外部接受与本地确认之间仍没有通用 exactly-once，未接管历史仍在执行的回合；真实模型证据仅覆盖所列路径 |
| Chat 私有子代理（方案 §5.1、§10.1、§6.4） | `run_subagent` 只在 Chat 成员工具集中出现；辅助 transcript 嵌套在父回合检查点内，不创建 TeamSpec 成员或独立线程。辅助模型请求、工具调用和批准共用父成员的 `TurnControl` 与模型步骤预算；团队动作、`update_plan`、递归 `run_subagent` 和父历史对辅助不可见。外部工具执行前写入 pending marker，执行后先写 child receipt，只有嵌套 `tool` 结果同检查点落盘后才清除 marker；无 receipt 恢复进入 `OUTCOME_UNKNOWN`，有 receipt 则补写结果且不重执行。回归覆盖隔离、绑定工具继承、批准恢复、模型失败回传、预算共享和崩溃窗口 |
| TUI 完整视图与响应（§13） | 日志页签实时显示成员工具活动（`push:"tool"` → `App::on_tool`，失败标 `✗`，尊重成员过滤，环形缓冲 2000 行，回归 `tui::tests::tool_activity_lands_in_the_log_panel`）；六个管理页签；`/settings` 仅切语言，`/model` 单独选模型；团队/日志 `h` 与 `/history` 提供用户只读的本地成员记录浏览。控制请求已通过有界异步队列，停滞 worker 有超时和过期响应保护。工作区审查见下一行。结果不明回合可按 `c` 结清；成员计划有独立状态条、`p` 查看清单；团队模型列显示已知上下文占比（≥80% 警示色） |
| 成员持久记录浏览（用户只读） | Engine `history` 协议从独立只读 SQLite 连接和受限文件句柄读取当前/已移除成员记录；有持久线程 ID 时通过独立客户端读取 Codex 原生分页或受限旧 JSONL。TUI 支持成员→来源→详情、分页、刷新、说明、返回、窄屏换行和 generation 防迟到响应。读取不启动模型回合、不恢复成员线程、不注入 Leader、不确认投递、不执行工具；缺少身份/文件或后端不可用时明确说明。证据：`engine/tests/history_protocol.rs` 9 项、`tui/tests/history_tests.rs` 6 项、三项 PTY；本机 CLI 人工存储联调见[原生历史记录](../review/native-history-2026-09-20.md)。 |
| 工作区实际变更审查（D-32） | `/review` 或团队/日志 `v`：持久首次观察快照包含原有脏输入，跨多批次文件/Shell/外部后端编辑和 Git 提交；同根恢复不重置，共享根明确不归因个人。文件列表、分页 diff、哈希/权限/大小/限制详情；二进制、特殊文件、超限不伪装成完整文本证据。`workspace_review` 16 项、`workspace_review_protocol` 3 项、`tui/tests/review_tests` 8 项及 `pty_review_check.py` 通过。后台 Git 阻塞时可取消实际运行中的确定性成员；关闭回收 Git、迟到读不重建归档状态。不提供回滚、原子快照、跨路径基线迁移或升级前历史，旧会话首次捕获仅从升级后开始；尚未用真实模型验收此界面 |
| 回退节点（D-26） | `chat.rs::ChatTree::rewind_to` 将 leaf 设为目标节点，包含该条输入；TUI 文案已改为“保留该条输入，移开后续对话”。`rewind_points` 只列当前祖先链，旧分支需已知节点 ID 才能访问 |
| 分叉与会话模型（D-26/D-29/D-30） | `fork_session` 复制 Leader 树、TeamSpec、会话 profiles/overrides，并在打开失败时保留源会话；团队任务/运行事实仍重新开始。真实失败注入仍需补充 |
| 工具输出读回（D-28） | 工具完整结果先写入私有历史，模型上下文使用随已知窗口变化且有上限的旧回执预算；`read_history` 按原始来源 ID 与 Unicode 字符位置分页。旧读回页被遮蔽时保留原请求的来源/页码，不推荐逐层读取读回回执。实际请求回归见[读回指针记录](../review/readback-pointer-2026-09-19.md)与[上下文预算记录](../review/context-budget-2026-09-19.md)。 |
| 配置校验与导出（§5.2、§14） | `cli.rs::validate_spec` 已合并 TeamSpec 所在目录的受信任项目配置；Leader 唯一、由 `leader_id` 指定且必须内置，在导入/保存/动态变更/持久修订读取时统一校验。恢复保留非法旧配置，仅无任何配置修订的半创建会话可用显式初始配置重试；任务依赖/委派权限在动作提交时校验。`publish_shared.ref` / `complete_task.result_refs` 已检查类型、私有上下文标识和本地路径目标，任务结算再次复核；发布引用不授予文件/历史读取权。受支持团队动作的外层字段已严格校验；仍没有专用 TeamSpec 导出 CLI，其他管理/执行工具协议及历史数据库行尚未全面收口 |
| 成员计划（update_plan） | 成员记录自己的分步计划（`members/<id>/plan.json`，每轮以 `<plan>` 块回灌；`plan_updated` 事件也进 hooks），TUI 以独立状态条展示；真实运行：模型三次更新计划并把两项都做完（`review/eval/runs/2026-09-15-deepseek-plan/`，回归 `chat::tests::plan_round_trips_into_the_prompt_block`） |
| 事件钩子 | `[hooks] notify = [argv]`：`tool_call`、`run_*` 终态、`team_action` 三类事件把 JSON 写到钩子 stdin（事件名作最后一个参数），主机权限运行、10 秒超时、失败只写 stderr 且不影响回合。回归 `chat_e2e::configured_hooks_see_tool_calls_and_turn_end`、`hooks::tests::hooks_receive_the_event_name_and_json_on_stdin` | `[hooks]`/`[retention]` 只在用户配置生效，项目配置里的相应段落会被忽略并提示（克隆的仓库不能装钩子），回归 `config::tests::every_user_config_field_is_accepted`、`user_hooks_and_retention_survive_loading_and_project_ones_are_ignored`； `pre_tool` 是执行前策略门：exit 0 放行、exit 2 拒绝（stderr 作原因，回执带 `denied_by: pre_tool_hook`）、其它情况放行并打日志（10 秒上限），被拒的调用**不会到达执行器**，回归 `gateway::tests::pre_tool_hook_denies_before_the_executor_runs`、`hooks::tests::pre_tool_policy_decides_by_exit_code`； `doctor` 会检查 `[hooks]` 里配置的程序是否存在/可执行，并打印 retention 策略（钩子配错本来只在事件发生时才在 stderr 露一行），回归 `cli` 集成测试；
 成员级中断的服务链另有独立任务 `resume-task-recovery`：dev 的回合被超时打断 → `task_blocked`（reason=external turn outcome could not be confirmed）→ Leader `cancel_task` → 重派一个『不需要等待』的任务 → dev `complete_task` → Leader 复核 + 结清两个未知 run + `signal_done`，`run.txt` 恰好一行 `started`（副作用无重复），证据 `review/eval/runs/2026-09-15-deepseek-task-recovery/`。| 中断后继续（resume） | 阶段 1 故意超时中断、阶段 2 `exec --resume` 继续同一会话：副作用恰好一次（`progress.txt` 只有一行 `alpha done`）、工作继续完成、`goal_done`（`review/eval/runs/2026-09-15-deepseek-resume/`）。配套修复：`cancel_run` 可结清 OUTCOME_UNKNOWN 回合（`acknowledged_outcome_unknown`），失败回执把 `result` 作为 `detail` 回传（否则模型看不到阻塞项与 run id），回归 `core/tests/engine.rs::acknowledging_an_unknown_run_unblocks_completion`、`chat::tests::refused_tool_receipts_keep_their_detail` | 该链路在 Codex 后端也验过（`resume-codex-recovery`：两个回合都落 OUTCOME_UNKNOWN → task_blocked → Leader cancel_task + 重派 → codex-dev 完成 → 结清两个 run → goal_done，证据 `review/eval/runs/2026-09-15-deepseek-codex-resume/`）。
| 结构化执行与评测 | `teamagents exec --json` 输出稳定 JSONL（`session`/`tool`/`event`/`result`）：`tool` 行给出每次工具调用的名称、参数摘要（≤500 字符）与成功/失败，`result` 行给出退出码、`duration_ms`、各成员 `usage` 与验收命令结果（另写入会话目录 `verification.json`）。工具活动经 `Notify::set_tool_sink` 从 ChatRunner 直达（回归 `chat_e2e::tool_activity_reaches_the_automation_sink`）；回合停在待批准时立即以退出码 3 结束（`cli.rs::exec_outcome` + `exec_tests::parked_approval_reports_approval_required_not_timeout`），不再等到超时报 124。固定任务集 `review/eval/tasks/<id>/` + `review/eval/run.sh`；DeepSeek 真实跑 3/3 completed、验收全通过、16–24s、原始 JSONL 见 `review/eval/runs/2026-09-15-deepseek/`。多供应商矩阵、多成员协作任务与被中断恢复仍未验收 |
| 沙箱内构建工具链 | 宿主 `$HOME` 不可见时，`toolchain_mounts` 将 Rust 工具链与缓存子目录只读镜像，排除凭据/配置文件；`sandbox_builds_with_the_host_toolchain` 验证离线 Cargo。`toolchain_projects` 新增系统 Python 的 venv 续接、离线 wheel 构建/安装，以及 Node 本地依赖、离线 npm ci、失败修正、测试/构建/实际 tarball 检查。Shell 默认 HOME 在成员 `shell/home/`，缓存不进入项目或发布包；工作区 MCP 使用临时私有 HOME。nvm/pyenv 等 HOME 级工具链仍不自动挂载；本地夹具不代表真实模型编码成功率。见[工具环境记录](../review/language-toolchains-2026-09-19.md) |
| Shell 续用状态 | 成员的 `cd`/`export` 跨命令保留，状态在 `members/<id>/shell/`；默认 HOME 的工具缓存也随成员保留。中断不更新目录/导出快照，但不回滚已经写入的文件。除成员 HOME 外，工作区挂载外的临时文件只在单次调用保留；保存的目录消失时跳过当前命令并报告 `ShellStateUnavailable` / `(exit 1)`，下次从工作区根开始，普通导出变量保留。目录捕获取 Shell 实际位置，避免导出的旧 `PWD` 误报。回归 `tools_sandbox` 中三项 `persistent_shell_*` 及 `an_interrupted_command_does_not_advance_the_shell_state`，见[恢复记录](../review/shell-cwd-2026-09-19.md) |
| Shell 长输出与制品 | 会话运行器的自动输出落 `members/<成员>/tool-output/exec-*.log`，有界预览 200KB，`/tool-output/` 仅当前成员只读；显式 `/artifacts/` 仍为会话共享。单输出上限 64 MiB，超出部分丢弃并提示；每成员目录预算 512 MiB，新建时按 mtime 删最旧自动日志。旧会话 `artifacts/exec-*.log` 无可靠归属，文件保留但成员工具拒读。`private_context` 覆盖跨成员、恢复、读回与旧引用；既有大小/清理回归继续通过。历史检查点/对话树仍无配额 |
| 会话保留策略 | 归档会话超过 `[retention] archived_days` 天时在打开会话时清理，或手动 `teamagents sessions prune --days N [--dry-run]`；走 `sessions.rs::delete_session` 的既有保护（运行中/未合并 worktree 跳过并报告），回归 `sessions::tests::retention_removes_only_old_archived_sessions`。回合检查点、对话树、`team.db` 有意不自动删（崩溃恢复/rewind/审计依据） |，另 `sessions prune --history-days M` / `[retention] history_days` 清理会话库里已受理的投递与旧事件（未受理投递与其事件一定保留，清完 VACUUM），回归 `storage::tests::history_pruning_keeps_pending_deliveries_and_their_events`

可复核上述现状（仓库根目录）：

```bash
rg -n 'into_json|fn chat_anthropic|fn rewind_to|fn read_history|cap_tool_output|self.bound.call' engine/src/chat.rs
rg -n 'Command::new|env_clear' engine/src/mcp.rs
rg -n 'fn resolve_in_root|fn bwrap_argv|fn workspace_executor_with_control' engine/src/tools.rs
rg -n 'fork_session|tree_src|profiles.json|model_overrides.json' engine/src/worker.rs engine/src/session.rs
rg -n 'agent_has_live_run|external_turn_id' core/src/control.rs
rg -n 'synchronous submit|fn apply_effect|from_secs\(120\)' tui/src/main.rs tui/src/worker.rs
```

## 更新记录

2026-09-17 复杂编码与长任务第二批：新增 6 项上下文/预算回归和 4 项独立评分回归，engine **208 passed / 1 ignored**；本批未改 core/TUI，实现基线仍为 core 63 / tui 91。评测 runner 的 25 项契约检查通过。真实模型评测严格使用原生上下文（D-36），DeepSeek Flash 的所有成员配置均为 1M：`rust-ledger` 三次产出全部通过隐藏 11/11、公开测试及文件保护检查；两次完整完成，一次总时限超时，完整成功率记 **2/3**。本组没有触发压缩，相关算法由本地回归验证。实现、限制与证据见 [第二批记录](../review/complex-coding-2026-09-17.md) 和 [原生上下文评测](../review/eval/runs/2026-09-17-rust-ledger/REPORT.md)。

2026-09-17 编码能力审查：新增 15 项 Rust 回归，core 63 / engine 198 / tui 91 通过；`live_codex` 的 1 项仍未启用，不能计入真实 Codex 验收。真实 bubblewrap 可用，文件/Shell 隔离用例实际执行。PTY 输入/粘贴/退出与鼠标命中两项检查通过；评测入口的 14 项无模型契约检查通过。

修复批量编辑半提交、特殊文件读取阻塞、暂停期间取消、迟到回执覆盖终态、目标完成提交竞态、三类模型协议不完整响应误执行、Responses 推理续接丢失及评测失败退出 0。整合回归曾有一次 `process_leaks` 的临时脚本出现 `Text file busy`；独立重跑与随后完整 engine 重跑通过，此波动保留在审查报告中。

DeepSeek 真实验证：Rust 修复、精确编辑、多成员协作、批准边界、中断后续接 5 类均符合各自预期；协作在目标完成修复后重跑仍通过，恢复验证为阶段一 124、阶段二 0 且验收全部通过。未运行四个竞品或五供应商矩阵，不能据此宣称同等/更高水平。总审查与后续路线见 [编码能力审查](../review/coding-agent-review-2026-09-17.md)，原始服务证据见 [本轮评测](../review/eval/runs/2026-09-17-coding-review/REPORT.md)。

2026-09-15 文档核对：更新当前测试口径、配置/权限/Skills/恢复与 TUI 说明，补齐实现差异；
只修改文档，未将缺口记作实现修复。示例 TeamSpec 在隔离的临时用户配置下通过 `validate`，
Markdown 本地链接、TOML 片段及 `git diff --check` 通过。更早的记录是对应日期的快照。

2026-09-14 更新：树历史/检查点恢复、取消与迟到压缩、工具输出索引、MCP HTTP 协议、
Skills YAML 描述共 7 项缺陷已修复；另完成 `/model` 成员/供应商/模型/思考强度选择器和
slash 菜单末项显示修复；模型候选合并本机配置和供应商在线目录，后台获取不阻塞操作。
离线测试 **core 43 / engine 116 / tui 70** 通过，真终端冒烟、
点击及模型选择检查通过。测试名、复跑命令和边界见
[`修复台账`](../review/fix-notes-rust-updates-2026-09-14.md)。

2026-09-13 追加审查的 6 项修复与 10 项新增回归检查见
[`review/fix-notes-rust-followup-2026-09-13.md`](../review/fix-notes-rust-followup-2026-09-13.md)，
补充 T8/T21 的真实 Chat 崩溃恢复、T11 配置生效、T19 MCP 模型调用、T22 运行中 shell
中断及 T23 悬空符号链接证据。
