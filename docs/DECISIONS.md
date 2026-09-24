# 实施决策记录（现行）

本文件记录**当前生效**的方向、边界与偏离：任何偏离已确认设计的做法，先与用户确认再实现。
早期实现的决策日志与完整背景归档在 [2026-09 决策史](archive/DECISIONS-2026-09.zh-CN.md)。

## 早期约定（仍然生效）

| 主题 | 现行约定 | 原始记录 |
|---|---|---|
| Skills | 用户级注册根 `~/.agents/skills`；按需 `skill search/read`，技能指令不能扩大执行权限 | D-23、D-34 |
| 用量可见 | 回合与目标的用量/预算在 TUI 与事件里可见 | D-24 |
| MCP | stdio 与 streamable HTTP 两种传输；调用经统一权限、批准、预算、取消与回执入口 | D-25 |
| 上下文压缩 | 按实际窗口占用触发；摘要保留原始要求与验收，原文可经读回检索 | D-28 |
| 任务优先级 | 复杂编码与长任务稳定性优先 | D-35 |
| 原生上下文 | 真实模型评测必须使用模型原生窗口并记录数值与来源 | D-36 |
| 安装与首次配置 | 下载即装；`init` 写最小配置且不覆盖已有文件 | D-37，`docs/INSTALL.md` 记录了更早发行版（≤ v0.1.2）的差异 |
| 自定义供应商 | 通过 `config.toml` 的 `[models.*]`（`protocol`/`base_url`/`model`/`api_key_env`）接入任意兼容服务 | D-40（早期 TUI 的 `/model` 向导随旧界面退役） |
| full_auto | 用户显式开启后使用主机 Shell（D-41）；默认 `approved_scope` 走 bubblewrap | D-41 |

## D-46 工作区策略接入 v2 spawn（2026-09-25）

D-45 发现 `engine/src/workspace.rs` 的共享/隔离/git worktree 策略（[设计](DESIGN.zh-CN.md) §12.3、Q14）只有自身单测调用，
用户确认"接线"后落地：

- **模型可见入口**：`spawn` 工具新增可选参数 `workspace`：`shared`（默认，项目目录）/`isolated`
  （会话状态根下 `<state root>/instances/<id>/work` 的私有目录）/`git_worktree`（该实例自己的分支与
  worktree）。未知取值是该次工具调用的失败（回执 `collaboration` 类错误），**不会**终止 driver。
- **解析与记录**：`driver::prepare_spawn_workspace` 在实例启动前解析策略、创建目录/worktree，并把结果
  写进 `<instances_dir>/<id>/workspace.json`（原子替换）；`workspace_ref` 指向解析后的目录，工具回执
  带上 `path/policy/note`，模型能知道"共享还是隔离、为什么回退"。
- **回退语义（沿用既有语义）**：请求 worktree 但项目不是 Git 仓库或有未提交改动时，按共享模式执行并在
  `note` 里说明原因，不静默忽略未提交的输入。
- **回收**：supervisor 在实例 `TERMINATED` 时按记录回收：共享记录只删记录；隔离目录若除自建
  `INPUTS.md` 外已有内容、或 worktree 有未提交/未合并成果，则**拒绝删除并打印原因**（保留现场），
  其余情况删除目录与记录。回收失败不影响终止本身。
- **失败不留垃圾**：spawn 被控制面拒绝时保留已准备的目录（不按猜测删除），同 id 重试会复用同一目录。
- 证据：`engine/src/workspace.rs` 单测（策略/回退/记录/回收，含"未提交成果不删"）、
  `engine/tests/v2_driver.rs::spawn_resolves_the_requested_workspace_policy`（隔离/worktree 行与回执，
  未知策略被拒且不建目录）、`engine/tests/v2_supervisor.rs::terminating_an_instance_retires_its_workspace`
  （终止后目录与记录被回收，共享项目不动）。文档同步见 `docs/USER-GUIDE.md` §3、README 特性表。
- 附带清理：`core/src/models.rs` 的 `AgentSpec`/`RuntimeKind` 只被旧签名使用，随 `prepare` 改签名为
  `(id, policy, project_cwd, member_dir)` 一并删除。

## D-45 清理早期实现残留并恢复 `[hooks]`（2026-09-25）

用户逐项确认后的落地：①执行历史重写；②清理早期实现代码；③移除 doctor 的 Codex 探测；④删除两个
`#[ignore]` 的旧入口测试；⑤按原有线协议恢复 `[hooks]`；⑥保留 `review/tmp/` 作探针目录；⑦推送。

- **hooks（⑤）**：`engine/src/hooks.rs` 沿用既有的线协议——`notify` 的 argv[1] 是事件名、事件 JSON 走
  stdin，异步、10 秒上限、失败只记 stderr；`pre_tool` 在任何原生工具调用前同步执行，exit 0 放行、
  exit 2 拒绝（stderr 首行作原因回给模型），其他退出码/启动失败/超时一律放行并记 stderr。
  v2 事件集为 `tool_call`（含 `tool`/`arguments`/`ok`/`error`）、`team_action`、`run_completed`、
  `run_failed`、`run_cancelled`、`run_paused`（进入 PAUSED 的边沿，启动时的既有值不算）。
  崩溃恢复的重放不再重问 `pre_tool`（决定在首次派发时做过）；必需检查是用户自己的验收命令、
  不是模型工具调用，因此不经过 `pre_tool`。证据：`engine/src/hooks.rs` 单测 +
  `engine/tests/v2_driver.rs` 的 `a_pre_tool_hook_vetoes_a_tool_call_and_the_turn_continues` 与
  `notify_hooks_receive_tool_call_and_run_completed`；doctor 继续检查钩子程序可执行。
- **旧控制面删除（②）**：删掉 `core/src/{control,storage,views,server,references}.rs`、`core` 的 stdio
  二进制 `teamagents-core`、6 个随其存在的测试文件，以及 `core/src/models.rs` 里只有它们使用的类型；
  `BUILTIN_TOOL_BINDINGS` 移到唯一的产品使用点 `engine/src/bound.rs`。core 从 243 个用例降到 91 个，
  产品路径与验收矩阵引用的用例（`core/src/v2/control.rs` 单测、`v2_invariants`、`kernel_properties`）全部保留。
- **doctor 的 Codex 探测（③）**：v2 已无 Codex 成员类型，移除 `codex app-server` 与
  `codex protocol schema` 两项检查及辅助函数；`--resume/--team/--plain` 的"不再支持"报错保留，
  因为用户可见的明确错误比静默忽略更好。
- **两个旧入口测试（④）**：即使启用也必然失败（断言的是已删除入口返回 `ok:`），随源码删除。
- **历史重写（①）**：`verification/tla/states/`（TLC 状态文件，未压缩约 23 GB）只出现在两个未推送的
  提交里，用 `git filter-repo --path verification/tla/states --invert-paths` 从历史移除。核对：
  重写前后 `HEAD^{tree}` 与 `git ls-files` 完全一致（内容未变），历史里已无该路径对象，
  仓库 pack 从 6.18 GiB 降到约 27 MiB；重写前的 `.git` 备份留在仓库同级目录，确认无误后可删。
  `verification/REPORT.md` 里引用的旧提交号 `bc536bb5` 同步更新为重写后的 `d37e1b4`。
- **工作区策略**：本次未动，用户随后选择"接线"，见 D-46。

## D-44 形式化验证发现的两处修复（2026-09-24）

用户确认"全部修复"后落码；两处发现都由 TLA+/TLC 规格先给出反例、再由代码探针确认，各自带回归测试。
修复台账见 [review/fix-notes-verification-2026-09-24.md](../review/fix-notes-verification-2026-09-24.md)。

- **V-W1 wait 的 tool_call 未被回答**：wait 只在 drain 路径回答自己的 tool_call；"注册即满足"与
  "被取代/关闭 epoch"两条路径把等待置 SATISFIED/CANCELLED 却不追加 tool 响应，严格线协议端点会拒绝
  下一次请求。修复：`answer_closed_waits` 把回答推广到两条路径（理由文本与 drain 一致，去重键仍为
  wait id）。规格侧 `ResolvedWaitIsAnswered`；回归 `wait_call_answered_outside_the_drain_path`。
- **V-G1 终态目标仍接受新工作**：委派任务、开新操作、继续记账三条路径都不看目标状态，目标结清后新回合
  仍记到已结清的目标上。修复：`budget_goal` 只认 ACTIVE 目标（实例指针与最旧开放任务两条路径），
  `complete_goal`/`block_goal` 结清即摘除实例指针（`detach_goal`，响应与事件带 `detached`），
  `delegate_task` 要求目标 ACTIVE 并提示先建目标。规格侧 `NoStaleActiveGoal`、
  `RegisteredWorkNeedsAnActiveGoal`、`RequestsResolveToActiveGoals`；回归
  `a_settled_goal_takes_no_new_work`。
- **语义边界（有意保留）**："新工作"的线性化点是请求而不是操作——请求在目标 ACTIVE 时准入，之后目标
  结清，其操作与用量仍落到该目标（诚实记账，不是新工作）。`complete_goal` 只检查开放操作、不检查任务，
  所以目标可在自己名下任务仍开放时结清，那些任务的后续请求没有记账目标；收紧需要先给 driver 一个
  "拒绝完成"的已提交结果，未列入本次范围。
- **V-P1 终止后残留执行指针**：规格↔代码的可执行对应测试（`core/tests/v2_invariants.rs`）在随机游走里
  发现终止实例后 phase 停在 `MODEL_PENDING` 而 `active_request_id` 指向一个已取消的请求；修复为终止分支
  与 `reset_instance`/`fail_request` 一样归一化执行指针。回归
  `terminating_an_instance_normalizes_its_execution_pointer`。
- **V-P2 压缩请求可被当作回合导入**：同一套可执行对应测试走到 `import_response` 作用在压缩请求上
  并成功；修复为控制面拒绝 `kind != 'turn'` 的导入（压缩请求由 `compress_context` 提交），回归
  `import_response_refuses_a_compression_request`。
- 证据：`make verify-model-all`（控制面/制品/等待/任务/压缩五个模块穷举全绿）、`make check`（含
  `core/tests/v2_invariants.rs`：穷举长度 ≤ 2 命令序列 + 60 条固定种子游走 + 覆盖率断言 + 检查器灵敏度
  反向验证）；性质↔代码↔验收编号映射见 [verification/README.md](../verification/README.md)。

## D-43 多供应商边界对齐 pi coding agent（2026-09-24）

用户指示多供应商支持直接参考 pi coding agent（现 `earendil-works/pi` 仓库的 pi-ai 包）。
据此把 R17 对照中列出的差距按当前影响落码，范围如下：

- **已落码**：tool call id 跨协议归一（同一请求内原 id 一致映射）、max_tokens 按上下文
  剩余钳制（4096 安全余量）——此两项随 R17/R20 已先行落地。本轮补齐：供应商无关的
  重试文本分类（429 携带配额/账单耗尽措辞 → Permanent，先于状态码表），effort 值在
  配置边界归一（deepseek xhigh→max 沿用既有用户决策；anthropic xhigh/max→high 按
  pi clampReasoning；其余原样透传，目录条目即用户对模型能力的声明）。
- **暂不落码**：图像降级（v2 尚无图像流；ponytail 已写明 pi 式升级路径——目录声明
  input 模态 + 边界占位符替换）；cost 费率目录（A18 按 token 记账，暂不按美元）；
  google/vertex/bedrock 等更多协议适配器（需要哪个加哪个，适配器模式已就位）。

## D-42 系统方向与范围确认（2026-09-23）

用户在完成 19 项需求澄清、重新审视 Python kernel 与 LangGraph 后，明确选择：
「Rust 小型 kernel + Rust 持久化实例运行时 + SQLite + 独立工具进程管理 + Rust TUI」，
并要求逐项反思理由与替代方案。此记录确认系统方向及范围。

- 产品实现继续使用 Rust。小型 kernel 负责简洁模型交互与执行决策，持久化实例运行时负责
  长任务、权限、调度、恢复与工具执行；不以 Python/LangGraph 为前提。
- 单人成队合法；实例默认隔离上下文、消息和工具访问权限。Leader 默认管理，可有限下授；
  经授权的实例可以直接通信，通信连接可以形成任意图，取代 D-33 的成员间直连限制。
  共享项目目录默认获授权，可按需采用独立目录/worktree；full_auto 仍是逻辑隔离。
- 首个可用版本包括 Rust TUI、基础文件/Shell/网页工具、MCP、Skills、多供应商及实例间混用模型。
  不需要外部 Codex 后端；短期助手使用统一实例机制，旧 D-39 的独立 helper 循环不构成新架构要求。
- 后台任务跨 CLI/TUI 退出继续；用户可重连接回、查看任意实例历史、直接对话和暂停/取消。
  这些用户权限不自动授予 Leader 或其他 Agent。实例在会话内复用，可终止/重置，跨会话默认隔离。
- 默认可持续工作，允许目标级预算，永久故障和重复失败有界处理。重启自动恢复已授权工作；
  外部操作结果未知时先核验，仍不能确认则停放相关任务并通知，不盲目重放。
- 必需检查必须通过，其余完成结论附证据和未验证项；独立审查按需进行，不强制多实例。
  验收同时关注同模型、同预算的成功率和长任务可靠性：单实例不退化，按需协作有可复现增益。
  先小规模测算费用，再确定正式评测预算；本条不含真实模型调用。
- DeepSeek Flash 即用户确认的 DeepSeek V4.1 Flash，主验收基线使用原生 1,000,000 上下文。
  D-36 原生窗口规则与 D-41 full_auto/approved_scope、进程组取消及密钥环境语义继续有效。
- 无需旧配置/会话兼容，允许清理旧会话、运行状态、缓存及旧配置；保留凭据、评测原始记录、
  审查证据、其他应用数据与 Git 历史。清理在切换阶段按归属清单执行，本轮没有删除数据。

完整需求映射与验收见 [设计与验收基线](DESIGN.zh-CN.md)；
单库原子边界、runner 握手、I/O 候选、并发参数等工程论证见
[45 项设计复核](../review/archive/agent-system-design-review-2026-09-23.md)。这些论证不是实测性能结论，
也不代表用户逐项批准了尚待探针选定的库、参数和统计精度。与本条已确认范围冲突的旧要求由本条取代，
未涉及的有效行为契约继续适用。
