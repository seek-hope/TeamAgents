# 实施决策记录（偏离与澄清）

基准文档：`TeamAgents-Implementation-Plan.zh-CN.md`。**任何偏离方案的做法，先与用户确认再实现。**
本文件只记录已确认的决策；未确认的候选方案写在对话里，不写进代码。

## D-38 Worker 固定环境指令（用户要求 2026-09-18）

用户要求 Worker 在接受 Leader 的提示词前先有 system prompt，以理解 TeamAgents 环境。

- 内置 Worker 的固定环境层放在 system 消息开头，介绍 Leader/Worker 关系、私有上下文与运行时视图、
  任务验收、通信与共享空间、权限、求助、完成任务和 Leader 专属动作；之后才是成员 `instructions`、
  Skills/指令文件和计划。具体任务与团队状态继续作为后续 user 输入投递。
- Chat 后端无条件创建/刷新开头的 system 消息，修复空 `instructions` 且无附加上下文时漏注入的问题。
  恢复时替换旧 system 内容，不重复插入；Leader 不套用 Worker 的角色限制。
- Codex 执行成员通过 `thread/start` / `thread/resume` 的 `developerInstructions` 获得相应环境层和
  成员 `instructions`，不替换原生 system 指令（不传 `baseInstructions`）。不授予团队工具，
  通过自身输出汇报；从 2026-09-19 起，Codex 也接收与 Chat 相同的已选、限量且符号链接安全的
  Skills/指令文件文本，仍不使用 Codex 自身的 Skills 发现协议。
- 已用本机 `codex app-server generate-json-schema` 确认上述两个方法支持 `developerInstructions`。
  验证覆盖三种模型协议的实际请求与恢复、Codex 创建/恢复线程的请求顺序；不等于真实模型行为验收。

证据：`chat::tests::worker_environment_precedes_member_instructions_and_survives_refresh`、
`chat_e2e::worker_environment_reaches_model_before_leader_task_on_all_protocols`、
`codex_contract::worker_environment_is_developer_instructions_on_codex_start_and_resume`。

## D-39 Chat 成员私有子代理（2026-09-19）

按方案中“私有子代理不自动成为团队成员、个体私有子任务计入所属成员资源”的约束，Chat 后端增加
`run_subagent` 作为成员内部工具。该实现不改变 TeamSpec、成员拓扑或 Leader 可见范围：

- 辅助 transcript 嵌套在父成员的回合检查点中；只接收显式 `task`/`context`，不读取父历史，也不获得
  TeamSpec 身份、团队动作、`update_plan` 或递归 `run_subagent`。
- 辅助继承父成员的工作目录、已绑定执行工具、权限/批准闸门和 `TurnControl`；辅助模型请求共用父回合的
  `max_model_steps_per_turn`，辅助最终文本作为父工具结果返回，父成员继续负责所有团队动作。
- 辅助模型失败作为失败工具结果回给父模型；需要用户批准的辅助工具调用可以暂停父回合，批准后从嵌套
  transcript 继续，不把未执行的调用伪装成成功。
- 外部工具的恢复采用两阶段检查点：执行前写入 pending marker；执行完成后先保存 child receipt，只有
  嵌套 `tool` 结果与 marker 清理在同一次检查点写入后才允许继续。崩溃若只有 marker、没有 receipt，
  进入 `OUTCOME_UNKNOWN` 并拒绝重放；若已有 receipt，则补写嵌套结果而不重复执行副作用。

证据：`engine/tests/chat_e2e.rs` 中 `private_subagent_uses_parent_bindings_without_team_identity_or_history`、
`private_subagent_approval_resumes_nested_tool_once_and_keeps_protocol_order`、
`private_subagent_model_failure_is_returned_to_parent_and_parent_can_continue`、
`private_subagent_model_steps_share_the_parent_turn_budget`、
`private_subagent_receipt_recovers_without_replaying_the_child_tool`。

## D-37 下载与首次配置优化（用户确认 2026-09-18）

用户确认“运行安装脚本 → teamagents init → 设置密钥 → teamagents”的扩展，随后明确要求仓库改为 public。

- `install.sh` 仅承担发行引导：自动选择最新版或指定版本、校验 SHA-256、安装成对的 Linux x86_64
  程序；支持自定义目录、本地发行包、curl 公开下载与 gh 认证下载。Shell 脚本是安装入口，产品实现仍为 Rust 三 crate。
- 新增 Rust CLI `init`，模板编译进程序，遵循 XDG；只创建缺失的配置，保留已有文件与符号链接，
  不写入密钥、不创建会话、不改变权限策略。默认模型与档位沿用 D-8，上下文沿用 D-36 的 1M。
- `doctor` 明确报告缺配置、配置不可读、空密钥；Codex 为可选能力，相关问题标 WARN，不阻止内置成员使用。
- 发行包包含安装器、指南及示例；发布检查涵盖三 crate 版本一致性、实际发行包安装与 init。
- GitHub 仓库 `seek-hope/TeamAgents` 已按用户要求设为 public（API 回读：visibility=public、private=false）。
  安装脚本和文档随 main 更新提供；新版 init/doctor 已随 v0.1.2 二进制发行提供。
- 兼容已发布 v0.1.1：该版本没有 init，安装器在配置缺失时复制包内模板；已有配置与会话保持原样。

验证命令与结果见 `review/install-2026-09-18.md`。

## 阅读口径（2026-09-15 核对）

各条记录的日期、旧路径与测试数量是对应实施批次的历史证据，不是当前环境说明。
D-15/D-16 的 TS 方案已由 D-17 取代，Python 实现已按 D-22 移除；当前只有 core/engine/tui
三个 Rust crate，`runtime_kind: deepagents` 仅为兼容字面量，后端为 `ChatRunner`。
历史记录中的 `.py`、Textual、Ink、LangGraph、旧工作目录路径及 `docs/RECONSTRUCT.md` 等
已删除文档不应用于当前操作。当前模块位置见方案 §15，用法见 [用户指南](USER-GUIDE.md)，
测试基线和未兑现的要求见 [验收表](ACCEPTANCE.md)。

后续记录优先：MCP HTTP 已由 D-25 实现；Skills 按 D-23 发现/分发；TUI 动效开关按 D-20
补充六移除；会话模型按 D-29/D-30 落盘；Codex 会话批准按 D-31 限定具体操作。
下文新增的“当前核对”注记只澄清实现现状，不代表批准新的方案偏离。

## D-1 短控制图：事务化步骤管线，不套 LangGraph StateGraph ✅（用户已确认 2026-09-11）

- 方案原文：§2.1「LangGraph 负责……短流程团队控制图的执行」，§4 控制图执行
  `ingest → validate → reduce → schedule → persist → END`。
- 实施：同样一组固定步骤实现在 `control.py::Control.submit`，整步在**一个 SQLite 事务**内提交
  （动作回执 + 事件 + 任务/共享空间变更 + 排队中的执行意图），执行意图持久化为 `QUEUED` TurnRun 后再交给运行时。
- 理由：该管线不调用模型、无图状态需要检查点；业务数据库是权威，事务原子性强于图检查点恢复。
  套一层 StateGraph 只增加壳，不增加能力（DP-1 的实质要求「固定小图 + 拓扑是数据」已满足）。
- 代价与升级路径：控制流步骤无法像图节点那样逐节点重放；若将来控制流出现跨事务的长时间步骤
  （例如需要中途调用外部系统），再引入 LangGraph 并按节点切分。

## D-2 工具权限：统一网关 + 逐调用批准 ✅（按方案 §12.2 执行，非偏离）

Deep Agents 的 `permissions` 只覆盖其内置文件工具，不约束 Shell/MCP；因此
`ToolGateway`（`agents.py`）是所有成员工具调用的唯一入口：团队动作（ACL 校验）与执行工具（权限评估 + 批准）
共用同一路径，身份由运行时注入。此为实现细节，与方案一致。

## D-3 Codex 协议 Schema 不入库 ✅（用户已确认 2026-09-11）

`teamagents doctor` 用本机 `codex app-server generate-json-schema` 现场生成并校验所需方法集合，
避免 4.3MB 生成物随 CLI 升级漂移。方案要求「Schema 根据选定 CLI 版本生成」，此实现满足该要求。

## D-4 Codex 成员默认配置 ✅（用户已确认 2026-09-11）

- 默认模型：不传 `model`，继承本机 `~/.codex/config.toml` 的默认模型与认证（当前为
  `gpt-6-astra`，走 apexin 中转）；成员级可在 TeamSpec/用户配置中覆盖 model 与工作目录。
- 思考强度：**固定 `xhigh`**；若所选模型不支持 `xhigh`，自动回退 `max` 并记住结果（一次性校准）。
  每个回合通过 `turn/start` 的 `effort` 参数显式传入，不依赖本机默认值。
- 沙箱/批准：每回合显式传 `sandbox`/`approvalPolicy`/`approvalsReviewer="user"`，
  不沿用本机 `auto_review` 的宽松设置（P0 已实测本机配置会放行 /tmp 写入）。

当前核对：Rust `session.rs::codex_options` 会映射所引用 profile 的 model/provider，
所以产品入口并非总是“不传 model”；默认 effort 为 xhigh，`/model` 可覆盖（D-27）。
本条中的本机默认模型记录仅代表当时环境。

## D-5 网页搜索：AnySearch 走 HTTP 直连工具 ✅（用户已确认 2026-09-11）

- 用户选择 (a)：接入 AnySearch。实测其接口为 `POST https://api.anysearch.com/v1/search`
  （Bearer `ANYSEARCH_API_KEY`，body `{"query","max_results"}`，返回 `code=0/data.results[]`），
  **不是 MCP 服务**。
- 实施：`tools.py` 提供 `kind="web_search"`（provider=`anysearch`）与 `kind="web_fetch"`
  的内置工具；MCP 通用路径保留给其他搜索/提取服务（如 Tavily MCP）。工具是否出现由用户配置的
  绑定决定，绑定即授权（无逐次批准）。抓取工具带 SSRF 防护（禁内网/回环/保留地址）与体积上限。

## D-6 CLI 增加 `--team FILE`（在方案 §13 五个入口之外的小扩展）

- 缘由：以指定 TeamSpec 启动会话是可测/可复现的前提（示例与测试都要用），
  且与已有 `validate TEAM_SPEC` 对称。方案 §13 的五个入口全部保留。
- 同期另有三个小入口沿用至今：`--plain`（行式 REPL，哑终端回退）、`version`（依赖版本）、
  `sessions`（会话清单，见 D-9）。

## D-7 Python 要求提升到 `>=3.12` ✅（用户已确认 2026-09-12）

- 方案 §3 原文为 Python 3.11+；用户要求提升到 **3.12+**，避免偏旧的 Python 行为。
- 方案正文已同步（§3 现为 Python 3.12+）；实现侧同步：`pyproject.toml` 的
  `requires-python`、锁文件重解析、README 与本文档；
  本机开发/验收用 3.13.15，测试套件（65 确定性 + 12 实跑）在 3.13 上全绿。

## D-8 默认模型统一为 `deepseek-flash` ✅（用户已确认 2026-09-12）

- 用户要求：TeamAgents 的默认模型配置全部使用 `deepseek-flash`。
- 落地：`examples/config.toml` 与用户 `~/.config/teamagents/config.toml` 的
  `leader_main` / `research` / `coding` 全部指向 `deepseek-flash`
  （provider=deepseek，`generation_options = { reasoning_effort = "max" }`）；
  实跑测试与三个端到端示例同步使用它。
- 其他服务（Kimi / apexin / Anthropic / GLM / Tavily）在示例配置里保留为注释，按需启用。
- **推理档位**（用户确认 2026-09-12）：产品默认配置用 **`max`**，
  **测试用 `high`**（跑得快、不改变被测语义）。实测（同一句“1+1 等于几 + signal_done”
  的 Leader 回合）：`high` ≈ 10s、不指定 ≈ 27s、`max` ≈ **240s**；交互场景嫌慢就把
  `generation_options` 改成 `high`。
- **xhigh → max 映射**（用户确认）：模型不支持 `xhigh` 时，配置里的 `xhigh`
  **自动映射为 `max`**，而不是报错或丢弃：`providers.normalize_effort` 对已知不支持
  xhigh 的协议（DeepSeek）在构建模型时直接映射；其余供应商由成员运行器在首次被拒后
  自动改判 `max` 并重建模型重试一次（`DeepAgentsRunner._switch_effort_to_max`，
  Codex 适配器同样回退到 `max`）。

## D-9 TUI 会话管理：切换 / 新建 / 归档 / 删除 ✅（用户要求 2026-09-12）

- 需求：「在当前目录下的多个会话间切换，并支持归档/删除当前目录下的任意会话；
  若删除的是当前会话则退出 TUI」。
- 落地：
  - 新增「会话」面板（`s`/`Enter` 切换、`n` 新建、`a` 归档、`d` 两次确认删除）；
  - 归档 = 移动到 `sessions/archived/<id>` 并置 `CLOSED`，记录全部保留；
  - 删除前检查：会话被其他进程持有（文件锁）→ 拒绝；成员 worktree 有未提交/未合并成果
    → 拒绝并说明；删除当前会话时先释放锁与后端再删除，然后退出 TUI；
  - 归档/删除当前会话后直接退出（符合需求）；其他会话操作后留在原地刷新列表；
  - `teamagents sessions` 复用同一份清单（含“已归档/运行中”标记）。

当前核对：Rust `sessions.rs::archive_session` 仅移动目录，不写 CLOSED；TUI 已归档行只读，
不提供恢复/再次归档/删除操作。归档、删除的现行步骤见用户指南 §3.1。

## D-10 放宽执行上限、删除无效配置 ✅（用户确认 2026-09-12）

用户认为原默认值过于保守，逐项给出新默认值：

- `TeamSpec.limits`：`max_parallel_workers` 4→**8**、`max_members` 16→**20**、
  `max_turns_per_goal` 200→**1000**、`max_model_steps_per_turn` 50→**200**、
  `turn_active_timeout_s` 900→**1200**、`cancel_confirm_timeout_s` 30→**60**。
- `ModelProfile.max_retries` 2→**5**（模型请求的 SDK 级重试）。
- 沙箱命令输出上限 100 KB→**200 KB**（超出仍转存 artifact 文件；Rust 版已于 D-21 对齐：
  落 `<session>/artifacts/exec-*.log`，工具结果给 `/artifacts/...` 引用，成员经同一前缀读回）。
  2026-09-19 按方案 §5.1、§12.2 修复自动输出归属：现存
  `members/<成员>/tool-output/exec-*.log`，通过成员私有的 `/tool-output/` 读回；
  `/artifacts/` 保留为主动共享制品。旧无归属自动日志保留但不再由成员工具读取，
  证据见[私有上下文隔离记录](../review/private-context-2026-09-19.md)。
- **删除未被任何代码读取的配置**：`leader_reserve`、`model_request_timeout_s`、
  `max_auto_retries`（`Limits` 为 `extra="forbid"`，旧 TeamSpec 里若仍写这三个键会校验失败）。
  真正生效的模型请求超时/重试是 `ModelProfile.timeout` / `ModelProfile.max_retries`。
- **修复 bug**：`DeepAgentsRunner` 的回合步数上限原先硬编码 50，与
  `limits.max_model_steps_per_turn` 脱节（把 spec 调大也仍会在 50 步被掐断）。
  现改为在图（重）建时读取会话 TeamSpec，`max_model_steps_per_turn` 真正生效。

## D-11 TUI 上下分区与 Codex 交互参考 ✅（用户要求 2026-09-12）

- 上区显示原右侧所有管理面板，下区显示原左侧 Leader 对话与输入；窄屏同样保留两区。
- 沿用方案中的 Textual；参考 Codex 的 composer/history 与流式消息展示，在现有组件上适配交互逻辑。来源和对应关系见 `TUI-CODEX-REFERENCE.md`。
- `Enter` 发送；`Shift+Enter`/`Ctrl+J` 换行；`↑↓` 在编辑区首末行调取本会话历史，并保留未发送草稿；`Esc` 请求停止 Leader。
- 批准队列改为 `Ctrl+G`，将 `Ctrl+A` 留给行首编辑；流式预览更新同一块区域，最终回复只追加一次，刷新不重放聊天。
- 此条落实用户当前要求；协作修复恢复原方案的调度、边界与身份约束，不引入新的团队权限模式。


## D-12 任务时间排序、界面语言与运行反馈 ✅（用户要求 2026-09-12）

- 任务列表按创建时间倒序显示，最新在最上方；同一时间按任务 ID 确定顺序。状态更新不改变排序，刷新保留选中的任务。
- TUI 默认英文，用户在 `Settings → Interface language` 选择 `中文` 后立即切换。界面语言只影响界面文案，不修改用户输入、模型回复或团队上下文。
- 增加团队运行指示、活动成员数量、回合已用时间和最近活动；团队与任务状态同步显示动效。等待任务、等待批准和终态有不同的静态提示，避免将等待显示为正在执行。
- `Settings → Animations` 可关闭旋转动效，保留状态与计时。语言和动效偏好保存在 `$XDG_STATE_HOME/teamagents/ui.json`（默认 `~/.local/state/teamagents/ui.json`），跨会话和重启生效。
- 动效读取 TUI 缓存的运行状态，不为每帧查询数据库；已用时间自回合创建起计算，包含排队和等待时间。此条落实用户当前要求。


## D-13 顶部合并为单行 ✅（用户要求 2026-09-12）

- 合并标题与状态栏：仅保留应用名、会话 ID 和权限模式；有未完成任务或待批准操作时才显示数量，暂停/关闭状态按需显示。
- 去掉时钟、装饰图标、重复会话名、常态 ACTIVE/IDLE、Leader 名称与模型、重复的活动回合数、零值计数。成员运行信息继续由下区活动提示和输入状态展示。
- 沿用现有状态栏与中英文切换，不引入新的布局组件。


## D-14 TUI 体验优化批次（光标保持、批准提醒、输入历史持久化）✅（用户要求 2026-09-12）

- **周期刷新保留表格光标**：Team/Approvals/Shared/Sessions 面板的 `refresh_from` 在
  `clear()` 重建前后用 `save_table_cursor`/`restore_table_cursor`（panels.py）保持选中行；
  行被消费掉时落到原行号（批准队列逐条处理时自动前进到下一条）。TasksPanel 原有保持逻辑不变。
- **批准到达主动提醒**：`APPROVAL_REQUESTED` 事件除写入对话外，仍处 PENDING 时弹
  toast（severity=warning）并响铃；重放的历史事件（已决定/不存在）不再提醒，避免开局刷屏。
- **任务详情改为 toast**：任务面板 Enter 查看详情不再写入永久聊天记录（`on_data_table_row_selected`
  改用 `notify`）。
- **日志成员筛选可退出**：团队面板高亮成员即筛选日志（仅在表格聚焦时生效，周期重建不再劫持筛选），
  对同一成员按 Enter 取消筛选；团队面板与日志标题增加对应提示文案。
- **流式预览节流**：`_flush_deltas` 的 Markdown 重解析限制为约 5 次/秒；回合结束（缓冲区清空）立即清除预览。
- **输入历史持久化**：`PromptInput` 历史保存到 `$XDG_STATE_HOME/teamagents/composer-history.json`
  （上限 500 条，原子写入），重启与会话切换后仍可 ↑↓ 调取；记录时机从事件回放改为提交时
  （避免重放历史事件重复入史）。`reset_history()` 仍全清（含文件）；会话切换改用 `clear_composer()` 保留历史。
- 会话面板本就不参与每秒周期刷新（仅挂载/激活时刷新），无需改动。
- 不改动：Esc 仍只停 Leader。成员回合的停止入口是任务面板 `c`（中断成员回合会留下 BLOCKED 任务，
  属已知坑，不给键盘直达是保护而非缺失）。
- ChatLog 保持无界增长（用户明确要求保留完整长对话历史供翻看），不设 `max_lines`。
- **顶部状态栏居中 + 间距对齐**（用户要求 2026-09-12，同日两次修订几何）：`#status` 文本
  `text-align: center`，高度保持 1、上下 padding 为 0；标签行（`ContentTabs`）上 padding 1、
  下 padding 0（上方一行空隙，下方与分割线相邻）。表头行悬浮不再变色：
  `datatable--header-hover` 与 `datatable--header` 同款样式（表头仅作索引，不可点击）。
  解释说明行（`#team-hint`/`#approvals-hint`/`#tasks-hint`/`#log-title`）与索引行间距 1→0
  （移除 `padding-bottom: 1`；`#settings-body` 不受影响）；随后按用户要求把各面板解释说明行
  （含 `#sessions-hint`）从面板顶部移到底部，即上下区分割线正上方，上下 padding 均为 0。

## D-15 TS+Rust 重构（reconstruct 分支）

- 背景：用户要求以 TypeScript+Rust 重构 TeamAgents；main 保留 Python 基准。
- 决策：
  1. Rust 承载权威核心（models/storage/control），以 stdio 换行 JSON 服务暴露，
     不引入 napi/socket；TS 承载 runtime/runners/Codex/CLI/TUI。
  2. DB DDL 与枚举字符串与 Python 版逐字一致，两版可共享会话 DB 与事件流。
  3. TS 侧零运行时依赖：`node:test` + Node 原生 TS 执行。
  4. 移植按 action kind 增量推进，Python 场景测试为 oracle；
     进度台账见 docs/RECONSTRUCT.md。

## D-15 补充（重构完成后）

5. runtime 的 `_finalize`/`begin_run`/`wake_info` 下沉到 Rust 核心（权威状态变更不跨进程）；
   编排循环在 TS。
6. deepagents(LangGraph) 后端正名为 `ChatRunner`（OpenAI 兼容工具循环）；`runtime_kind`
   字面量保留兼容旧 spec。
7. 会话锁用 pid 文件（Node 无 flock 内建），与 Python flock 语义等价于"进程存活即占用"。
8. TUI 为零依赖 ANSI 实现，键位/面板/历史持久化对齐，不复刻 Textual 视觉细节。
  9. TUI 用 Ink(React) 而非手写 ANSI：手写版两次实测翻车（转义序列错误致重绘风暴），
     Ink 是 Node 生态的 Textual 对等物；TS 侧因此有且仅有 ink+react 两个运行时依赖。
  10. 渲染只 dirty 时重绘（帧级去重 + 行尾清除），修复"无限弹出/闪屏"。

## D-16 Rust+ratatui TUI（reconstruct 分支，用户要求 2026-09-13）

- 背景：用户要求以 Rust+ratatui（与 Codex CLI 同框架）重构 TUI，界面完美复现 main 分支。
- 决策：
  1. `tui/` 新 crate（ratatui+crossterm）是纯 UI 客户端；执行引擎复用已验证的 TS runtime，
     由新增的 `ts/src/tui-worker.ts`（无头 JSON-lines stdio 服务）承载。不重复移植 runtime。
  2. 复现基准是 main 的 Textual 界面（非 Ink 版）：七面板与列、活动行（spinner/等待批准/
     最近活动）、Leader 流式预览（markdown，≤8 行）、作曲家（高度 3–8、历史 500、相邻去重）、
     状态栏计数、页脚键位、中英双语（i18n.py 的 146 条消息 id 逐字移植）、偏好/历史文件
     与 Python 同路径同格式。
  3. 渲染差异接受项：Textual `$primary 40%` 光标行用预混色 #1F3C6A 近似（终端无 alpha）；
     DataTable 列宽按内容自适应+超宽收缩替代 Textual 原生布局；markdown 为手写子集
     （标题/加粗/行内码/围栏码/链接/引用/分割线），升级路径 tui-markdown crate。
  4. 时序：状态轮询 250ms 单次 `state` 调用驱动全部面板（Ink 版 400ms 同法），替代 Python
     的三路定时器；spinner 120ms、delta flush 80ms（渲染 200ms 节流）与 Python 一致。
  5. core 新增 `shared_entries` 方法（唯一的核心改动，只读、向后兼容）。
  6. `node ts/src/cli.ts` 默认启动 Rust TUI（tui/target/{release,debug}），`--ink` 回退。
  7. 键盘焦点模型：composer 默认；Tab/点击进表格、Ctrl+N 回 composer、Ctrl+G 直达批准
     （Textual 的焦点链在 ratatui 里没有对应物，这是最小等价物）。

## D-17 重构收尾：TypeScript 层全部移植进 Rust（reconstruct 分支，2026-09-13）

- 背景：用户要求「完成 TeamAgents 的 Rust 重构」。D-15/D-16 之后仍剩 TS 编排层
  （runtime/gateway/runners/tools/config/sessions/CLI/tui-worker）；本轮把它整体移植为
  Rust 并删除 TS 与 Node 依赖。
- 决策：
  1. 新增 `engine/` crate（lib + 二进制 `teamagents`）：runtime、gateway、成员后端、
     工具执行器、config、sessions、CLI、worker 协议全部 Rust。
  2. core 的 stdio 方法分发下沉为 `core::server::Server`（lib）：`teamagents-core` 二进制与
     engine 共用同一份实现，engine 直接进程内调用（不再有第二条 JSON 通道）。
  3. 二进制 `teamagents` 身兼三职：CLI（doctor/validate/sessions/version/--plain）、
     TUI 启动器（spawn `tui/target/*/teamagents-tui`，并注入 `TEAMAGENTS_ENGINE`）、
     `serve`（TUI 的无头会话服务，与原 `tui-worker.ts` 协议逐字兼容）。
  4. 运行模型：asyncio → 线程。回合执行、取消确认、超时各用一个线程；进程内 core 用
     `Mutex<Server>` 串行化（SQLite 单写者）。`Runtime::close` 不再等待在飞回合
     （TS 版会等整段模型调用，导致退出/切会话卡住）；留下的 RUNNING 回合由下次启动的
     `reconcile`（RT-04）收敛。
  5. 会话锁：pid 文件 + `/proc/<pid>` 存活检查（Python 用 flock），语义等价，
     不引入 libc 绑定。（2026-09-13 起改为 `File::try_lock` 的 flock 语义，见 D-21：
     kill -9 自动回收，pid 仅作诊断。）
  6. 修两个移植版共有的缺陷：① `ChatRunner` 缺 `base_url` 时一律打 api.openai.com——
     现按 provider/protocol 解析默认端点（deepseek → api.deepseek.com/v1），与 providers.py
     对齐；② 权限模式切换（TUI Ctrl+F / set_permission_mode）不生效——审批门现在每次
     check 前从会话行同步 mode，用户开全自动立即生效。为此 core 增加只读方法
     `session_mode`（与 D-16 的 `shared_entries` 同类）。
  7. 删除 `ts/`、根 `package.json`/`package-lock.json`/`node_modules` 与 Ink 备用 TUI；
     仓库运行时不再依赖 Node。
  8. 成员执行工具对齐 runners.py：ChatRunner 按成员 `tool_bindings` 暴露
     `files`（ls/read_file/write_file/edit_file/delete/glob/grep）、`shell`、
     `web`（web_search/web_fetch）工具，仍全部经 ToolGateway（审批 + 审计）。
  9. 修 TS 版的 shell 沙箱缺陷：bwrap argv 与 execution.py 逐项对齐（ro-bind /usr,/etc,/opt；
     /lib /lib64 /bin /sbin 用 symlink 重建；tmpfs /tmp；unshare-pid/ipc/uts；die-with-parent）
     ——TS 版直接 `--ro-bind /bin /bin` 在宿主 /bin 是符号链接时会让沙箱里找不到 bash；
     且 **缺 bwrap 时不再降级为裸 bash**（plan §12.2 不允许静默降级），环境变量改为白名单
     （不把模型密钥带进沙箱命令）。
- 验证：core 14 项、engine 25 项（含 T1–T5/T9、取消/暂停、full-auto、Codex 适配、
  worker 协议、CLI、bwrap 真实运行）、tui 11 项全绿；真终端 PTY 冒烟通过；真实 DeepSeek
  回合（--plain：直接回答 + 调 shell 工具执行 `echo`，均到 goal_done）、真实
  `codex app-server` 回合（engine/tests/live_codex.rs）、kill -9 崩溃窗口 reconcile 收敛，
  均实测通过。

## D-17 补充（文档核对轮，2026-09-13）

用户要求「检查文档是否与实现同步」后，逐条核对文档与代码，修掉了发现的三处**实现**问题
（能改代码的要改代码，不能只改文档）：

1. **TeamSpec 只认 JSON**：实现与 USER-GUIDE/方案（JSON/YAML）不一致，且仓库示例
   `examples/team.yaml` 本身就是 YAML。现 `--team` 与 `validate` 均支持 JSON/YAML
   （`serde_yaml`，本地 cargo 缓存已有）。
2. **`workspace_policy` 被静默忽略**：之前所有成员的文件工具根都是会话目录（等价 shared），
   `isolated`/`git_worktree` 静默失效。现按策略取根：`shared` = 会话目录、`isolated` =
   `sessions/<id>/workspaces/<成员>/`（文件工具与成员后端同根），`git_worktree` 显式报错
   （Rust 版未移植该策略，拒绝静默降级）。实测：isolated 成员写出的文件落在自己的
   workspace 内，项目目录保持为空。
3. **暂停/中断后的对话历史不合法**：回合在 `wait_for_tasks`/批准处暂停时，assistant 的
   `tool_calls` 没有对应的 tool 消息，恢复时被供应商拒绝
   （真实 DeepSeek 复现：`400 assistant message with 'tool_calls' must be followed by tool messages`）。
   现暂停/中断路径补齐未执行调用的显式结果，保证每次请求的历史合法。
4. **TUI `Ctrl+A`/`Ctrl+E`**：未处理的 Ctrl 组合会当普通字符插进输入框（`Ctrl+A` 打字出 "a"）。
   现按文档实现行首/行尾移动，并吞掉其余未处理的控制组合。
5. 文档侧：README 重写为「先讲怎么启动」；USER-GUIDE 增加 §0 版本适用性表（Python 专有项
   逐条标 ⚠）；STATUS/ACCEPTANCE/TUI-CODEX-REFERENCE 标注适用版本；ACCEPTANCE 增加 Rust 版
   T1–T24 对照表（✅/🔶/⚠，未移植项单列）。

验证：core 14 + engine 27 + tui 21 全绿；PTY 冒烟；真实 DeepSeek 两次端到端
（单 Leader；Leader→isolated 成员委派→恢复→goal_done，文件落在成员 workspace）。

## D-19 功能补全：MCP / Skills / 项目配置 / worktree / Anthropic（2026-09-13）

用户要求「把 main 分支的 Python 版在 reconstruct 上完全用 Rust 重构」，本轮补齐 D-17 之后
剩余的 Python 专有能力，全部以 Python 实现为基准：

1. **workspace 策略齐全**（`engine::workspace`，workspace.py 逐条对齐）：`shared`（会话目录）、
   `isolated`（`members/<成员>/work` + `INPUTS.md`）、`git_worktree`（`teamagents/<成员>-<时间戳>`
   分支与 worktree；重开会话复用既有 worktree；非 git 仓库或有未提交改动时回退 shared 并说明；
   未提交/未合并成果拒绝清理；`merge_branch` 供 Leader 合并；删除会话时先做同样的守卫检查）。
2. **MCP 工具服务**（`engine::mcp` + `engine::bound`）：stdio JSON-RPC（initialize →
   notifications/initialized → tools/list → tools/call），工具名 `<service>_<工具>`、
   `tool_names` 过滤、`required` 失败即成员启动失败、可选失败只丢能力；**绑定即授权**
   （命中绑定集合的调用不经批准门，其余仍走 ToolGateway 审批与审计），与 runners.py 的
   middleware 行为一致。http/sse 传输未实现（记入未移植项）。
3. **Skills 与指令文件**：`session` 解析 `skills_paths` + 项目 `.teamagents/skills` +
   成员目录 skills，以及 `instruction_files` + 项目 `AGENTS.md` + 用户配置目录 `AGENTS.md`，
   内容注入成员系统提示词（8KB/文件、32KB/成员上限）。Python 版用虚拟文件系统挂载，
   这里是提示词注入（ponytail 已注明升级路径）。
4. **项目配置与权限配置**（`config::load_user_config_for` / `permission_mode_from_config`）：
   项目文件可加模型 profile（同名用户定义优先并告警）、工具绑定仅在
   `[permissions] trust_project_tools = true` 时生效、`[permissions] mode` 决定缺省权限模式
   （`--full-auto` 仍可覆盖）；配置里声明的 skills/instruction 路径不存在即报错。
5. **Anthropic 原生协议**：`ChatRunner` 增加 Messages API 分支（`x-api-key` +
   `anthropic-version`；system 提炼、assistant/tool_use 与 tool_result 合并规则、
   `max_tokens`、工具 schema 用 `input_schema`），响应转回内部 OpenAI 风格消息。
6. **core 增强**：`set_catalog`（把用户配置交给内核做拓扑校验，与 Python 的
   `Control(store, session, catalog)` 等价）；`ToolBinding.required` 字段补上。
7. **新增验收**：`engine/tests/topology.rs`（T11–T13）、`engine/tests/recovery.rs`
   （T8/T21 崩溃恢复与去重、T22 目标回合上限）、`engine/tests/mcp_tools.rs`（真实 MCP stdio
   服务器）、`workspace.rs`/`config.rs`/`session.rs` 单测（worktree 生命周期、项目配置信任、
   skills 收集）。

## D-20 Rust TUI 重新设计：不再对齐 Textual，改为 Rust 原生体验（用户要求 2026-09-13）

用户要求：既然 Rust 与 Python 的实现手段不同，Rust 版 TUI **不再追求与 Python 版逐像素一致**，
而应更美观、交互更自然。D-18 的"像素级对齐"由此被本决策取代；帧对比脚本保留为参考工具，
不再作为验收门槛。

设计（`tui/`，ratatui/crossterm）：

1. **响应式布局**：宽终端（≥110 列）为"聊天主区 + 右侧栏圆角框"，窄终端自动堆叠为上框下聊。
   侧栏标签带计数徽标（任务/批准/会话/共享），选中页签用 `▍` + 强调色标出，标签放不下时换行。
2. **状态栏**：左侧品牌 + 会话，右侧胶囊：权限模式（全自动用强调底色）、待批准数、未完成任务数；
   暂停时插入警示胶囊。
3. **聊天**：活动行改为状态胶囊（成员 + 已用时间，如 `⠸ leader 00:12`）与右侧"最近活动"；
   对话区可滚动（PgUp/PgDn、Ctrl+U/D、鼠标滚轮、Ctrl+Home/End），带细滚动条与
   "已上翻 N 行"提示；内容不足时**底部对齐**（聊天从下往上生长）；
   流式预览带强调色竖条以区别于已归档消息。
4. **作曲家**：圆角边框输入框，边框在聚焦时变强调色；空输入显示占位提示；
   边框右上角显示 Leader 状态与模型（如 `○ Idle · Leader / test`）；
   新增 Ctrl+W / Alt+Backspace 删词、Ctrl/Alt+←→ 按词移动。
5. **表格**：暗色表头 + 分隔线、极淡斑马纹、选中行用强调色竖条 `▌` + 轻底色（不再是整块蓝底）、
   空状态居中提示（如 "— No tasks yet —"）；**列自适应**：窄栏按优先级依次丢弃次要列
   （如团队表的 Type/Workspace/Access），再按需收缩，避免列被硬裁。
6. **Esc 语义**：面板中按 Esc 先回到输入框，输入框中按 Esc 才是停止 Leader；页脚右侧显示当前焦点。
7. **鼠标**：滚轮滚动所在窗格；点击页签切换；点击表格行选中；点击聊天区回到输入框。
8. **视觉语言**：单一强调色（Codex 蓝）、灰阶层次（FG/GREY/NOTICE/DIM）、圆角只用于
   "框"（侧栏、作曲家、提示气泡），表格/状态栏保持扁平。toast 用圆角边框 + 图标。

未跟进 D-18 的对齐项：Textual 的滚动条字形、`▊▎` 选择器外观、页脚溢出滚动——这些是
Textual 的实现细节，新设计以 Ratatui 的能力与终端习惯为准。

## D-20 补充（用户反馈两处 bug + 打磨，2026-09-13）

> **本组第 1 条的 `ui::sidebar_width` 已被「D-20 补充四」（固定上下分区）取代，
> 代码中已无该函数；以下保留作历史记录。**

反馈：右侧栏宽度不随内容伸缩导致内容显示不全；侧栏顶部导航变成两行。

1. **侧栏宽度改为按内容计算**（`ui::sidebar_width`）：取当前面板的自然宽度——表格面板按
   表头与单元格的列宽合计（与渲染同一套 `col_widths`）、日志面板 84（正文按 80 换行）、
   设置面板取最长设置行 + 4——再夹逼到 `[40, 92]`、不超过终端 62%、并保证聊天区至少 44 列。
   结果：150 列里团队表 7 列全显（侧栏 92），120 列时自动只收次要列；切换面板时宽度随内容变化。
2. **页签栏永远一行**：放不下时以当前页签为中心向两侧展开，被裁掉的一侧显示 `‹`/`›` 标记
   （`★` 不再换行）。窄终端堆叠布局下侧栏占满宽度，页签栏自然完整。
3. **长表格可滚动**：表体按选中行开窗（`ui::table_start`，优先居中），超出时右侧显示细滚动条；
   鼠标滚轮悬停在侧栏上时等同 ↑/↓ 移动选中行（日志/设置仍走滚动）。
4. 鼠标点击表格行按「可见行 → 选中索引」换算（`App::select_row_visible`），与开窗一致。
5. 文档：README 与 USER-GUIDE 的 TUI 章节同步（侧栏按内容伸缩、页签窗口化、滚轮行为）。

## D-20 补充二（状态栏并入左列、侧栏满高、几何单一来源，2026-09-13）

用户反馈：把 `Pre-authorized` 从右上角移到左上角会话 id 之后；侧栏底部边框与输入框下底边对齐；
侧栏顶部边框放到终端第一行；并怀疑点击位置与显示位置错位（与右上角字段把侧栏下推一行有关）。

1. **状态栏并入聊天列**：宽布局下第一行左侧是状态行（`TeamAgents <会话> <权限模式> ⚠ 待批准 ▸ 未完成`），
   右侧就是侧栏框的顶边框——侧栏从终端第一行开始。堆叠布局仍保留整行状态栏（侧栏在它下面）。
   胶囊按优先级排列，放不下的直接丢弃（不再被裁半个字）。
2. **底部对齐**：作曲家把按键提示移到自己的**下边框**上（按宽度自适应只显示放得下的部分），
   于是聊天列的最后一行就是作曲家框的下边框，与侧栏框的下边框正好同一行。
3. **几何单一来源**：新增 `ui::geometry(app, area)`，渲染与鼠标命中都读它（`tabs_y`/`rows_y`），
   彻底消除"两处算行号"导致的点击错位。点击行号按可见窗口换算（`select_row_visible`）。
4. 新增 `tui/scripts/pty_click_check.py`：真终端里点击某一行，断言选中行就是被点击的那一行
   （含自带迷你 ANSI 屏模拟器，能重建最终画面）。

## D-20 补充三（状态栏精简：模式移到右下角、去掉 pane 说明、会话页签不再计数，2026-09-13）

用户要求：`Pre-authorized` 移到右下角并改用 pane 说明的格式（纯暗色文字，不再用灰底胶囊）；
删掉右下角的 pane 说明（多余）；会话页签后面不再显示会话数。

1. 状态行只保留 `TeamAgents <会话> ⚠ 待批准 ▸ 未完成`（模式胶囊移出）。
2. 页脚右下角显示权限模式：`Pre-authorized` 用暗灰（与原先 pane 说明同款式），
   `Full auto` 用强调色加粗（无背景色）作为风险提示；pane 说明删除（焦点已由侧栏边框颜色表达）。
3. 页签徽标只保留任务/批准/共享（待办类）计数，会话页签不再显示数量。

## D-20 补充四（英文说明、/settings 命令、固定上下分区，2026-09-13）

用户要求：TUI 里所有说明用英文（此前右栏按键说明仍是中文）；右栏去掉 Settings 页签，
改用 `/settings` slash command 查看并修改；右栏宽度伸缩会让 pane 按键位置漂移，
因此改为固定上下分区（上区就是原右栏内容）。

1. **英文说明**：修掉了两处缺英文映射导致回退中文的提示（审批面板与会话面板的 hint id 写短了）；
   新增回归测试 `ascii_frame_has_no_cjk_leaks`——英文模式下逐面板（含 `/settings` 浮层）
   渲染整帧并断言不含 CJK 字符，任何漏翻都会让测试失败。
2. **Settings 移出侧栏**：页签只剩六个（团队/任务/共享空间/批准/会话/日志）。在输入框输入
   `/settings` 回车打开居中浮层（圆角、强调色边框）：`↑↓` 移动、`Enter/Space` 切换、
   `Esc` 关闭；语言下拉与动效开关照旧，偏好仍写入 `ui.json`。浮层是模态的，吞掉键鼠输入。
   输入框占位提示改为 `Ask the Leader…  (/settings opens preferences)` 以便发现。
3. **固定上下分区**：取消"宽终端左右分栏"——状态行在最上，面板框占上方 2/5（整宽），
   聊天在下方，页脚在最后一行。pane 按键位置不再随宽度/内容变化，点击命中只依赖
   `ui::geometry` 的固定几何。内容自适应列、表体滚动、滚轮驱动选中行等能力保留。

## D-20 补充五（悬浮指示、斜杠命令体系、页签点击错位根因，2026-09-13）

用户反馈：状态行/页签点击 A 却切到 B；需要鼠标悬浮指示（黑底灰字 → 灰底白字）；
需要系统性的 slash command（输入 `/` 列出候选）；确认 Animations 开关是否有实际效果；
删除设置里与页脚重复的 `（Ctrl+F 切换）` 说明。

1. **页签点击错位根因**：渲染与命中各自算了一遍页签宽度，命中算法把徽标宽度多算了 2 列，
   于是带计数的页签之后都会偏移。现改为 `ui::tab_pieces` 单一来源（渲染与 `ui::tab_at`
   共用同一组 span 与宽度，含窗口化偏移），并用两个测试锁定：
   单元测试逐页签断言"点哪选哪"，PTY 检查在真终端点击 Log 页签断言面板确实切到 Log。
2. **悬浮指示**：`App::pointer` 记录鼠标位置，渲染时对指针下的**页签**与**表格行**应用
   `HOVER_BG`（#3a3a3a）+ 白字（即"灰底白字"）；移开即还原。鼠标移动事件（crossterm
   any-event 模式）驱动，未点击也会更新。
3. **斜杠命令**：`SLASH_COMMANDS` 注册表（`/help`、`/quit`、`/settings`）；输入框以 `/` 开头
   即弹出候选菜单（`/s` 之类前缀过滤），`↑↓` 选择、`Tab` 补全、`Enter` 执行、`Esc` 关闭菜单
   （不影响已输入文本）。`/help` 把键位与命令写进对话，`/quit` 直接退出，`/settings` 打开设置浮层。
   菜单自带英文说明，注册表可直接扩展。
4. **Animations 开关确有实效**：关闭后活动行的 spinner 从 Braille 动画帧变为静态 `●`
   （`activity_status` 的 animations 分支），新增单测断言两者不同且关闭时为 `●`，因此保留。
5. **删除重复说明**：设置浮层里的 `状态 / 权限模式` 行不再附带 `（Ctrl+F 切换）`（页脚已有键位），
   英文同理。

## D-20 补充六（悬浮范围修正、移除 Animations 开关，2026-09-13）

用户反馈：悬浮时**所有页签**同时变灰底白字（应只有指针所在的那个）；Animations 开关实际总是打开，
请从设置界面删除。

1. **悬浮范围 bug**：`tab_pieces` 用 `tab_cols`（整条页签栏的 x 范围）判断悬浮，于是指针落在
   页签行上就等于"命中所有页签"。现改为 `ui::tab_layout` 输出每个页签**自身的绝对列区间**，
   `hovered` 按区间逐块判定；渲染、悬浮、命中三者共用这一份布局（`tab_at` 也只是查这份布局）。
   单测同时断言：被指的页签是灰底白字，且相邻页签仍为原样式。
2. **删除 Animations 开关**：设置浮层只剩"界面语言"一行（Enter 打开语言下拉，Esc 关闭）；
   `App::animations` 恒为 true，偏好文件里的 `animations` 键保留写入以兼容旧文件但不再生效。
   活动行 spinner 始终动画。
3. 顺带修正浮层层次：语言下拉是最内层，Esc 先关闭下拉、再关闭浮层。
4. **浮层排版**：语言行下面只留一个空行（原来两行）；框高按 `info + 5` 精算。
5. **宽字符吃掉边框（ratatui 缓冲区 diff 的硬规则）**：`Buffer::diff` 对"紧跟宽字形之后的
   单元格"一律跳过，所以中文聊天行正好压到浮层左框线时，那条框线会被静默丢弃（人工核对
   150 列帧时发现第 21 行左框线消失）。修法：浮层左侧预留 1 列 gutter（先清空该列），
   内部文本区再比右框线少 1 列；新增回归测试逐行断言框线单元格存在。

## D-21 Rust 版全面审查与修复批次 ✅（2026-09-13）

对 `core/`、`engine/`、`tui/` 三个 crate 做了一轮对照 Python 基准的全面审查
（5 个只读审查域：core、engine 运行时、engine 工具/沙箱、tui、文档），随后四个修复批次
（core / engine-runtime / engine-tools / tui）与文档同步落地。发现与修复台账：

- `review/findings-rust-review-2026-09-13.md`（严重 2 · 高 11 · 中 22 · 低 17，含重复归并）
- `review/fix-notes-rust-review-2026-09-13.md`（逐条修复与关键测试）

主要修复（摘要，细节见台账）：

- **core**：事务内错误不再静默吞掉（`submit`/`emit` 等返回错误）、载入路径不再 panic
  （损坏 spec 报错、旧 limits 键按 Python 语义丢弃）、投递批次账本回写、`run_started`
  事件恢复生产、`wait_for_tasks`/`wake_info` 结果按 task_id 建键、`payload_hash` 与
  Python `json.dumps` 逐字节一致；新增跨版本合同方法 `expire_approval`、`approval_find_run`。
- **engine 运行时**：`limits.max_model_steps_per_turn` 真实约束模型请求数（超限 → `limit_reached`
  事件 + 回合 FAILED）、回合活动超时中断成员（消除幽灵写入）、once 批准执行后消费
  （EXPIRED 重请求、session 批准不消费、DENIED 阻断）、Codex reconcile 改用
  `thread/read {includeTurns:true}`、xhigh→max 归一化与重试、成员对话历史落盘重启装载、
  HTTP 重试只针对瞬时错误。
- **engine 工具/沙箱/配置**：shell 大输出不再死锁、超 200KB 落 `/artifacts/exec-*.log`
  且成员可经 `/artifacts/` 前缀读回、MCP stderr 直通不阻塞 + 子进程环境白名单、
  会话锁改 flock（kill -9 自动回收）、`guard_url` 与 Python 判定表逐条对齐（含 IPv6）、
  web 工具执行层 fail-closed、`[permissions]` 非法值报错、doctor 实跑 bwrap 与
  codex `generate-json-schema` 方法集合探针（D-3 兑现）。
- **tui**：窄终端浮层/菜单/toast 不再 panic（矩形夹取 + panic 后恢复终端）、几何单一来源
  （滚动后点击命中不再错位）、面板动作键拒绝 Ctrl/Alt 组合、`Ctrl+D`/`Ctrl+U` 任何焦点下滚动、
  开启 bracketed paste、中文光标按显示宽度定位、`/settings` 浮层只剩界面语言（Animations 已删）。
  集成期补：`Tab` 对任意页签都进面板（此前列表残留已删除的 settings 项、且共享/日志页签
  无法用键盘进入）。

验证（实跑）：core 38（17 unit + 21 integration）、engine 67（21 lib + 46 integration）、
tui 54（9 lib + 18 app + 27 render）全绿；PTY 冒烟与点击检查通过；真实 DeepSeek `--plain`
回合通过；`teamagents doctor` 通过（bwrap 实跑、codex schema 99 方法）。

**与 Python 的已知保留差异（如实记录，不粉饰）**：

1. DENIED 跨进程重启后重发同一操作会重新请求批准（engine 的 (run, op_hash)→approval 记忆表
   是进程内的；方向安全——只会多问，不会放行；完全对齐需 core 透出最新 DENIED 记录）。
2. Codex 审批等待保留 600s 有界超时（Python 无界），超时把该批准置 EXPIRED；
   `TEAMAGENTS_CODEX_APPROVAL_WAIT_S` 仅供测试覆盖。
3. Codex 进程组清理经系统 `kill` 二进制（未引入 libc 依赖）；`kill` 缺失时退化为只杀直接子进程。
4. 成员历史 JSON 无长度上限（`ponytail:` 已注明，可改为按窗口裁剪）。
5. 会话面板 size 有 30s TTL 缓存，最长滞后 30s。
6. web 工具策略层仍按 `web_` 前缀预授权（执行层已 fail-closed：未绑定必拒，但不会弹批准）。
7. `/artifacts/` 只挂进文件工具：`ls`/`glob` 与 bwrap 内 shell 看不到（与 Python 基准一致）。
8. MCP 的 http/sse 与 deepagents `general-purpose` 子代理仍未移植（既有台账项）。

## D-22 Python 原版移除、main 指向 Rust 实现（2026-09-13）

用户确认：`main` 用 Rust 版覆盖，仓库只保留 Rust 实现。

1. **移除 Python 原版**：删除 `src/teamagents/`、`tests/`、`pyproject.toml`、`uv.lock`、
   `dist/`（Python wheel/sdist）与 `examples/e2e_*.py`；同时清掉迁移期残留
   （`.pre-fix-backup/`、`conversation_history/`、`large_tool_results/`、`home/`、
   `tmp/probe_tui_mount.py`）。旧实现保留在 git 历史里：最后一个含 Python 的提交是
   `ba1caed`（`git show ba1caed:src/teamagents/control.py` 等仍可查阅）。
2. **测试自包含**：MCP 测试原先依赖 `.venv` 的 `mcp` 包与 `tests/mcp_echo_server.py`，
   现改用仓库内 Rust 二进制 `engine/src/bin/fake-mcp-server.rs`（支持 `--noisy-stderr`），
   `mcp_tools`/`mcp_stdio` 不再需要 Python 环境；`tui/scripts/*.py` 只是真终端测试工具。
3. **两个尾巴修复**（对应 D-21 之前审查报告"未修复"清单的最后两项）：
   - 聊天工具宣传改为按**解析后的 web 绑定**（`tools::web_tools`）：显式绑定名（如
     `anysearch`）也会向模型暴露 `web_search`/`web_fetch`，且只暴露实际绑定的那种
     （`ChatRunner::web_flags`）。
   - 日志面板 `↑↓` 真正生效：在"全部 → 各成员 → 全部"之间循环筛选
     （`App::cycle_log_member`），`Enter` 清除筛选，与面板提示及旧版语义一致。
4. **验收（实跑）**：core 38 / engine 67 / tui 55 全绿；PTY 冒烟与点击检查通过。
5. **分支状态**：`main` 已覆盖为 Rust 实现（快进，无历史丢失），`reconstruct` 分支随后删除，
   仓库只保留 `main`。

## D-23 Skills 分发机制：skill 工具 + 按成员注入（2026-09-13）

当前配套范围见 D-34。

背景：D-19 的"全量内容注入"简化在技能库规模（~200 个）下必然超 32KB/成员上限，
代码内已标注升级路径。用户要求 Leader 能查看全部技能、按需读取、并把特定技能
分发给特定成员。

决定（落回方案 §12.1/§12.2 原义，非新偏离）：

1. 新增内置绑定 `skills`：成员绑定后获得 `skill` 工具（search/read），注册根 =
   用户配置 `skills_paths`，只读、属于预授权"明确选择的 Skills 只读"。
2. 成员 `skills: [名称]`（TeamSpec 或 add_agent/update_agent patch）按名注入
   SKILL.md 全文到该成员系统提示词。同名覆盖顺序：
   用户级 → 项目级 → 成员级。
3. Leader 查看全部技能 = `skill search`（空查询列出全部，10 条封顶提示细化）；
   分发 = patch 成员 skills 字段，运行器按 config_revision 重建即生效。

来源约定（用户 2026-09-13）：TeamAgents 的唯一用户级注册根为 `~/.agents/skills`，
技能统一安装到该目录。`skill search/read` 覆盖注册根的检索与读取，科研检索时机由成员 instructions 指定。

证据：`engine::tools::tests::skill_tool_searches_and_reads_registry`、
`engine::session::tests::member_context_collects_skills_and_instruction_files`、
chat 工具载荷断言；core 38 / engine 78 / tui 55 全绿。

## D-24 Token 用量可见性（/status，2026-09-14）

用户确认补充（对照 Codex CLI /status，审查 A1）。ModelProfile 加可选 `context_window`；
chat.rs 按 thread 累计 OpenAI/Anthropic 两种 usage 形状；codex 成员解析
`thread/tokenUsage/updated`（自报窗口兜底）。会话内存态、不落盘（注释已留升级路径）。
出口：worker `usage` 方法、TUI `/status`、--plain `status`。
证据：chat/config/codex/worker/app 各层测试；engine 98 / tui 63 当时全绿。

## D-25 MCP 远程 HTTP 传输（2026-09-14）

方案 §12.1 本规划项落地。ToolBinding 加 `url`/`bearer_token_env_var`/`startup_timeout_s`/
`tool_timeout_s`（全 optional）；mcp.rs 重构为 Transport 枚举，`connect_http` 实现
streamable HTTP 最小语义（POST、JSON 与 SSE 两种响应、mcp-session-id 与协商后的
MCP-Protocol-Version 回带、多行 SSE 按事件拼接并匹配请求 ID、Bearer 仅来自
环境变量）；bound.rs 按 transport 分发，"sse" 明确报错改用 "http"。
天花板：不支持服务器主动推送（GET SSE 流）与会话终止 DELETE，遇到需要的服务再加。
证据：engine/tests/mcp_http.rs 三项 + bound.rs 单测。

## D-26 会话 fork/rewind：pi 式树状历史（2026-09-14）

用户指定参考 pi-mono 的树状会话结构。落法（TeamAgents 语境的最小正确版）：

1. 每个成员线程的历史从线性数组改为**追加式节点树 + leaf 指针**（chat.rs::ChatTree，
   `chat_tree.json` 与旧 chat_history.json 并存；旧文件惰性迁移为链）。模型调用时从 leaf
   回溯物化线性消息。
2. `rewind` = 移动 leaf 到指定节点（**包含该节点输入**）；`/rewind` 列出当前祖先链的用户输入点，
   `/rewind <n>` 按列表选择，已知节点 ID 可直接指定。被放弃的分支留在树里，但列表不遍历全部分支。
   Leader 有 QUEUED/RUNNING 回合时拒绝 rewind；只改模型记忆，不撤销团队事实、文件或已显示日志。
3. `fork` = 新会话 = 同 TeamSpec（含拓扑补丁后的活 spec）+ leader 对话树复制；**团队事实
   （任务/回合/事件/共享空间）不复制，文件改动不回滚**——rewind/fork 只管对话记忆。
   当前 `fork_session` 也不复制 D-29/D-30 的模型覆盖与会话 profile；后者可能使带自动 profile 的
   TeamSpec 无法在新会话打开。任意成员有 QUEUED/RUNNING 回合时拒绝 fork。
4. 检查点一致性：首次执行前固定旧线性历史的树迁移；checkpoint 记录 tree_base/tree_leaf
   及待追加节点，按「检查点追加日志 → 原子替换树 → 清除日志」提交。恢复可幂等补齐
   提交两侧的崩溃窗口。仅显式 rewind 的 rewind_epoch 变化使旧 checkpoint 作废；
   其他无法恢复的失配报 OUTCOME_UNKNOWN，禁止丢弃动作身份重新执行。codex 成员不支持 rewind（历史在
   app-server 侧，trait 默认返回不支持；其原生 /fork 未接线，需要再说）。
出口：worker `rewind_points`/`rewind`/`fork_session`、TUI `/rewind` `/fork`、--plain `rewind`。
证据：chat.rs 树单测两项、engine/tests/fork_rewind.rs、tui app_tests 两项。

2026-09-19 实施更新（D-32/D-35 范围内）：旧线性历史读取/结构错误不再降级为空对话；
树在持久化边界检查 ID、父节点、leaf 与摘要祖先关系，损坏恢复沿用
`CheckpointError / OUTCOME_UNKNOWN` 并保留原文件，回退列表显式传出错误。
祖先遍历改为线性，清空路径后压缩使用当前分支根；分支、线程与持久 JSON 格式不变。
新增八项回归及 DeepSeek 原生 1M 有效历史重建检查，见[完整性修复记录](../review/history-integrity-2026-09-19.md)。

## D-27 会话内切换模型/档位（/model，2026-09-14）

用户确认补充（对照 Codex CLI /model，审查 A6）；本轮追加供应商选择和交互式选择器。
会话级 ModelOverride（profile/model/effort），
不写回 TeamSpec（落盘由 D-29 追加）；runner 工厂构建时应用覆盖，设置后
drop_runner 使下一回合生效（在跑回合保持 runner 可达并标记配置过期；取消、批准与补充输入
仍能找到它，取消信号也直接撤销 TurnControl）。
出口：worker `model`/`set_model`、TUI `/model`（成员 → 供应商 → 模型 → 思考强度）、
原有手输参数与 --plain `model`。候选合并现有 config.toml 的 models 和供应商在线目录，按 provider 分组，
同名模型以 profile 名区分；选 profile 一并切换地址、协议、认证环境变量及窗口配置。
选择器支持搜索/粘贴、上下移动、Esc 返回、恢复默认，普通菜单显示完整命令，矮屏滚动到选中项。
用户确认两种候选同时提供：进入供应商时配置候选立即可用，后台调用其 models 接口；
OpenAI 兼容接口读取 data，Anthropic 支持 after_id 分页，同一地址/协议/认证的配置只请求一次。
在线结果以已有 profile 为连接配置，再覆盖模型 ID；worker 专门并发处理只读 discover_models，
不阻塞轮询、取消或关闭。失败保留配置候选并显示原因；迟到结果按会话和请求代次过滤。
请求有超时/页数/响应大小上限，不落盘缓存；退出并重进供应商可重新获取。
Codex 成员仅列 OpenAI 兼容配置，端点须支持 Responses API；重建连接时显式 thread/resume
并覆盖 model/modelProvider，后续 turn/start 也带 model。Anthropic 的档位传到 output_config.effort。
档位是否被具体模型接受仍由服务端校验，不承诺 provider 下所有模型能力相同。
证据：session/model_override/worker/codex_contract/chat_e2e/app/render 各层测试及真终端 /model 在线模型检查。

## D-28 上下文自动压缩：分层管线 + 读回指针（2026-09-14）

用户从调研候选中选定 ①+④（依据：arXiv 2508.21433 遮蔽≈摘要的效率实证、Codex/Claude Code
工业配方、Git Context Controller 的读回思想）。落法（只对 chat 运行时；codex 成员由
app-server 自行压缩，为已知天花板）：

1. **L0 写入时限流**：工具结果字符串 >50,000 字节时处理，保留头尾各最多 25,000 字符
   （chat.rs::cap_tool_output）；此步先于历史写入，被截去内容无法通过 read_history 恢复。
2. **L1 视图遮蔽**：发给模型的线稿（非 checkpoint/树）里，尾部 16k 字节预算之外的旧
   tool 输出替换为占位符（含 tool_call_id 指针）；最后一个 assistant 之后的未答工具结果
   永不遮蔽。
3. **L2 阈值摘要**：`usage.last_prompt > 0.9 × ModelProfile.context_window` 且回合内无
   待答工具调用时，调一次模型生成六段式交接摘要（目标/进展/文件/错误/任务/下一步），
   作为带 `skip_to` 的摘要节点提交到 ChatTree——materialize 跳过被覆盖区间，但**覆盖
   内容留在树里**（摘要保留已入树内容，但不能恢复 L0 已截去的内容）；连续失败 3 次熔断本 runner。
   关键不变量：压缩前先把未落树的尾部提交进树（覆盖的对象必须已在树中）。
4. **④ read_history 工具**：chat 运行时成员恒有（非能力绑定）；按 tool_call_id 查活
   历史→全树分支，取回被遮蔽/覆盖的已保存输出（受 L0 限制）。
   摘要请求和结果均附工具输出 ID 索引，从当前分支完整祖先链生成；连续压缩也保留旧索引，
   不依赖模型在摘要正文中复述 ID。摘要树与检查点写入共用 TurnControl，关闭会话后的迟到结果不能落盘。
出口：自动生效，无命令；触发信号复用 D-24 的 usage 采集。可调常量（50k/16k/256k/0.9/100k）
在 chat.rs 顶部，分别覆盖 L0 上限、L1 预算下限/L1 上限、L2 阈值和摘要输入上限，
均为 ponytail 注释的已知天花板。
证据：chat.rs 单测 4 项（截断/遮蔽/摘要节点+rewind/read_history）、chat_e2e.rs
compaction_triggers_on_threshold_and_read_history_recovers_output。

2026-09-19 实施更新（D-32/D-35 范围内）：L0 已改为只截减发送副本，完整工具结果保存在
私有检查点与树里；旧读回页的占位符使用原始来源及分页参数，见
[读回指针记录](../review/readback-pointer-2026-09-19.md)。固定 16,000 字节的 L1 在 1M 窗口
下过早遮蔽中等源码，主成员与私有子代理的实际请求回归均复现。沿用本条可调预算的方向，
将旧回执预算设为 `clamp(context_window / 4, 16_000, 256_000)` 字节，未配置时仍为 16,000；
这是有上限的保留启发式，不是 token 数保证。L2 估算、主请求、溢出恢复请求和私有子代理
使用同一窗口预算。当前原生窗口、模型步骤/时限、权限与持久记录格式均沿用原约束。
实现与真实对照见[上下文预算记录](../review/context-budget-2026-09-19.md)。

## D-29 /model 覆盖随会话落盘（2026-09-14）

用户明确要求：会话内调整的模型在重开该会话时保留，取代 D-27 的「不落盘、重开恢复默认」边界。
覆盖存 `sessions/<id>/model_overrides.json`（tmp+rename 原子写，每次 set 全量重写）；
打开会话时加载并按当时的 spec/catalog 重新校验——成员不存在、profile 消失、
Codex 成员选了非 OpenAI 兼容 profile、档位超出协议白名单的条目丢弃（配置可能已变），
不让陈旧覆盖阻断会话打开。恢复默认即删除对应条目。在线发现的模型本身仍不落盘：
重开后若该模型不在 config.toml，覆盖条目保留 model 名，连接复用所选 profile。
证据：model_override::model_overrides_survive_session_reopen（设置→重开生效→恢复默认→重开还原）、
stale_model_overrides_are_dropped_on_open（未知成员/未知 profile/越规条目全部丢弃）。

## D-30 成员↔profile 一一映射：add_agent 自动建会话级 profile（2026-09-15）

用户确认的理想行为：Leader 创建成员时系统创建对应 profile（默认与 Leader 当前模型相同，
也可按其要求选模型），之后用户用 /model 调整；同一会话内 profile 与成员一一映射，
不同会话各自一套。

落法（全部在 engine 产品层，core 协议不变）：

- 会话级 profile 存 `sessions/<id>/profiles.json`（tmp+rename 原子写），随会话持久；
  打开会话时并入 push 给 core 的 catalog，成员/补丁校验因此天然接受这些名字。
- 拦截点：`ToolGateway.call` 在 `apply_topology_patch` 提交前运行会话安装的
  topology_prepare 钩子（Runtime::set_topology_prepare；reject 不触发）。钩子里每个
  add_agent：model_profile 省略或指向未知名字 → 复制 Leader 当前生效 profile
  （含 /model 覆盖的模型与档位；未知非空值视为请求的模型 ID，沿用 Leader 连接），
  以成员 id 为名写入 profiles.json，并把 op 改写为该名字；指向已有 profile 则原样通过。
- 查找顺序：session_profiles 覆盖同名用户配置（runner 工厂、/model 报表与校验、
  discover_models 去重都走合并视图）。
- 边界：remove_agent 不清理对应 profile（无害残留；重做同名成员会被拒，id 不复用）；
  set_catalog 推送与补丁提交非原子，竞态下补丁校验失败会大声报错，Leader 重试即可
  （注释已标）。初始 TeamSpec（用户手写 YAML）不自动建 profile，仍须引用已配置名字。
证据：chat_e2e::review_add_agent_auto_creates_member_profile（省略/模型 ID 两种写法、
改写落 spec、profiles.json 内容、报表解析、重开后仍生效）。

2026-09-20 实施修复：仅实际 Leader 可进入准备钩子；自动创建不覆盖同名用户模型配置或已被
成员引用的会话 profile。候选先在局部准备，保存和 catalog 更新成功后才安装内存视图；
未使用的拒绝残留允许修正。原有跨存储非原子边界保留，见[组队校验记录](../review/topology-validation-2026-09-20.md)。

同日补齐提交边界：准备前复用 core 的身份/请求/版本校验；按 `patch_id` 批准的提案也使用相同
默认值。`Control::submit` 的 Rust 参数可同时接收原始动作和可信准备结果，原始动作记录去重回执，
准备后的操作在同一事务中校验和应用。JSON 协议和数据库结构不变，准备失败也保存拒绝回执。
旧版回执不迁移，跨存储非原子限制保留；见[请求与重放记录](../review/topology-requests-2026-09-20.md)。

## D-32 成熟度与稳定性改进授权（2026-09-15）

用户明确要求「按你建议的顺序对 TeamAgents 进行改进，核心目标是足够成熟、稳定」。
按已讨论的路线推进：文件读写与输出保存、界面响应及权限边界；流式响应、上下文与恢复；
交付验证和持久用量；结构化非交互 CLI、回归评测与发布检查。
此授权覆盖上述改进与相应文档更新。保留 Rust 三 crate、core 单事务权威、操作级批准、
成员隔离和已有会话兼容性；新行为使用已有依赖，提供可重跑的回归证据。
实施进度与验证记录见 `review/stability-2026-09-15.md`；未执行的真实模型评测不能记为通过。

实施补充（方案 §5.2/§5.3/§6.2/§7/§9 范围内）：任务类动作按严格类型校验，任务和回合引用按会话限定；
迟到结算复核承接者、任务状态和成员存在性，保留原结果不明回合的正常核对路径，不覆盖已结清/移交任务或成员新回合状态。
十五项 core 回归及一项普通/全自动 ChatRunner 参数修正、交付与重开检查见
[任务边界记录](../review/task-boundaries-2026-09-19.md)。沿用 SQLite 单事务权威，不新增架构偏离或真实服务验收结论。

同范围继续补齐受支持团队动作的外层参数校验及 `serve` 用户输入类型；共享读取按各空间游标分页，
条目统计使用实际聚合，求助任务与替代条目引用复核会话/访问边界。错误请求、游标故障回滚、
持久回执重放及生产入口修正路径见[动作请求记录](../review/action-requests-2026-09-19.md)。
沿用原追加式共享记录与现有数据库结构，没有增加跨存储事务或扩大真实供应商验收结论。

持久记录补充：任务/回合列表、事件/投递和批准 JSON 不再以默认值掩盖坏数据；
视图、调度、唤醒及批准过期错误传入现有事务，失败保留原记录并回滚状态。
十项 core 回归与生产 ChatRunner 启动拒绝、字段恢复后继续交付的检查见
[持久业务记录完整性](../review/stored-integrity-2026-09-19.md)。
没有迁移或自动修复机制，不改变权限裁剪、合法空值或原有会话身份，不增加真实服务验收结论。

运行时错误传播补充：回合开始与成员视图/唤醒读取在原 core 事务内完成，失败保留同一个排队意图；
成员返回后的状态读取失败保留原结果等待恢复，恢复核对的存储错误也继续重试。
`runtime_errors` 是进程内用户诊断，供 CLI/TUI 显示，不形成第二份业务权威。
`exec --json` 拒绝未受理输入的成功判定，有运行时错误时跳过交付检查。
实现、五项新增回归及未覆盖的故障组合见[运行时存储恢复记录](../review/runtime-storage-2026-09-19.md)；
保持数据库格式、权限边界及冷恢复依据，不新增真实服务验收结论。

运行中投递补充：核心的投递事务同时返回接收成员与当前权限投影，内部 `drain_mid_turn`
回复增加 `agent_id`；进程内客户端使用同一核心方法的类型化结果，避免排空后的读取故障丢失交接。
运行时周期派送并重试投影错误；原有投递确认与后端注入前授权复核保持。
五项新增行为检查包括真实进程的补充消息、成员通信及 SIGKILL 后重新排队故障恢复，
见[运行中消息记录](../review/mid-turn-storage-2026-09-19.md)。没有新增数据库结构、持久队列或权限偏离。

排队取消补充（方案 §6.3、§7、§9）：无外部回合 ID 的 QUEUED 在原事务中直接结清；
不调用成员执行器或读取其输入视图来确认未活动状态。整回合取消记录其未消费输入的失效原因，
任务取消仅移除目标任务的待投递就绪通知，保留其他消息和任务。批准/任务/回合/投递失败整体回滚，
旧 QUEUED 取消请求可在调度时收敛；外部回合继续等待停止确认。六项 core、两项 engine 新检查及
结果等待状态读取期间退出的补充证据见[排队取消记录](../review/queued-cancellation-2026-09-19.md)。
不改变数据库格式、等待回合的既有投递确认策略或外部副作用保证，不新增真实供应商验收。

plain 与终态恢复补充：行模式检查输入回执，读取/存储错误明确反馈并返回命令输入；
`status` 读取新增事件，后台重试保留原工作。Chat 中断接口保持已知终态，与既有 Codex 处理一致；
恢复已返回结果的回合时先核对历史 epoch、修复提交日志，再直接归档终态检查点，
避免取消标记与重新排队的时序覆盖结果。四项新增 engine 检查、历史日志重放扩展及范围见
[plain 与终态恢复记录](../review/repl-finalization-2026-09-19.md)。
保持方案 §9 的状态权威、停止确认与副作用边界，不新增数据库格式、后台守护进程或真实供应商结论。

归档通知与旧排队记录补充（方案 §9）：`finalize_run` 在原事务中返回 `applied` 和调度后的
`status`，运行时按该次提交发通知，无变更不发旧结果。恢复用成员检查点/外部回合 ID
识别已有执行的 QUEUED，整批恢复为 RUNNING 后再核对后端；保留取消和输入，不重记开始事件。
内部准备由进程内客户端调用、SQLite 单事务提交，失败整体回滚。首次用户输入及重开时的
全自动模式变更也先准备，避免其调度提前结清旧回合。数据库格式及权限规则保持，
九项新增检查及证据识别上限见[通知与恢复记录](../review/recovery-state-2026-09-19.md)。

执行证据与保留补充（同属方案 §9）：旧排队回合额外核对数据库 `run_started`，避免检查点丢失后
把已执行工作当成新意图。恢复和清理共用载荷/身份校验；历史清理保留未结清回合的开始事件，
包括仍可继续核对的 OUTCOME_UNKNOWN。候选读取、投递及分块事件删除在同一 savepoint 内，
失败整体回滚；数据库格式、脚本后端的稳定动作重放和原后端核对方式保持。
四项 core、一项 engine 新回归及 Codex 扩展见[恢复证据记录](../review/recovery-evidence-2026-09-19.md)。
开始事件不等于结果证据，全部执行依据丢失的识别上限与真实服务验收缺口保留。

成员记录补充（方案 §9.1/§13）：用户可按持久线程 ID 读取 Codex 原生对话与工具记录，
采用独立只读客户端和受限的旧格式文件回退；不复制外部历史、不恢复成员回合、不扩大模型权限。
保持 Codex 历史权威与原有 worker 后台读取方式。接口、路径/身份边界、失败回归和本机无模型
联调见[原生历史记录](../review/native-history-2026-09-20.md)；不新增架构偏离或真实供应商验收结论。

## D-31 codex 通道 session 级批准收紧为按 operation_hash 绑定（2026-09-15）

用户确认的收紧：core 的 session 级批准按 `operation_hash` 绑定单一操作（基准 §12.2），
而 codex app-server 侧的 `acceptForSession` 缓存覆盖后续更多同类操作，两侧语义不一致。
统一向 core 看齐：

- 决策为 session 时 runner 对 app-server 只回单次 `accept`（永不再发 `acceptForSession`），
  app-server 因此每次调用都来问；后续同 hash 操作由 engine 闭环的
  `session_grants`（core `session_approval_cache` 行 + 内存 revision 守卫双重条件）
  自动放行，不同操作重新产生 PENDING 行请用户决定。
- 批准 waiter 通道改传 core 原生决策词汇（once/session/deny），在应答点映射为线上词汇，
  使 session 决策能在应答前登记 per-hash grant（带与 ToolGateway 相同的 revision 复核）。
- operation_hash 的输入按 kind 投影判别字段（commandExecution→command/cwd、
  permissions→permissions/cwd；fileChange 与未知 kind 用整个 payload，fail-closed——
  fileChange 的线上参数没有判别字段（grantRoot 为 UNSTABLE 且通常缺失），
  宁可每次询问也不把整类文件改动并入一个 grant），
  因线上 params 携带每次调用唯一的 threadId/turnId/itemId/startedAtMs，整体 hash 永不相等。
- 回合已终态的在途批准即使命中 grant 也回 decline（拆除期不得执行）；同 hash 并发 waiter
  由 grant 一并释放（回 once、作废多余 PENDING 行）；set_mode 的 bump+clear 在 grants 锁内
  完成，消除"新 revision + 旧 grant"的观察窗口。
证据：codex::tests::session_decision_is_single_op_on_the_wire_and_cached_per_operation
（session 决策线上为 accept、同 hash 新 id 自动放行不挂 PENDING、终态 run decline、
同 hash 兄弟 waiter 被释放且多余 PENDING 作废、set_mode 后重新询问、deny 仍 decline）；
契约测试 codex_contract::codex_session_grant_auto_accepts_the_identical_operation
（fake app-server 两次同命令请求，两次线上应答均为 accept，第二次不产生 PENDING 行）。

## D-33 组队默认值：成员继承 Leader 工具、自动 Leader↔成员通道、成员之间只走共享空间（2026-09-15）

用户明确指示：

1. `add_agent` **省略** `tool_bindings` 时，新成员继承 Leader 的绑定；
2. `add_agent` 时自动建立 Leader↔新成员通道；
3. 成员之间的通话**只允许通过共享空间**进行，便于 Leader 查看。

背景：真实评测 `team-collab` 首轮 900 秒超时失败——Leader 建出的成员只有团队工具（收发消息/任务），
既改不了文件也跑不了命令，被派的任务永远完不成；同时 Leader 与成员之间没有任何通道，连"我卡住了"
都传不出来。

落法（与 D-30 同一个提交前钩子，core 协议不变）：

- `engine/src/session.rs::topology_prepare_hook` 对每个 add_agent：
  - 未给 `tool_bindings` → 复制 Leader 当前绑定；**显式 `[]` 仍表示"只要团队工具"**（保持可表达，
    文档要求写清两者的区别）；
  - 补两条 message 通道（`leader→成员`、`成员→leader`），已存在同向 message 通道时不重复添加。
    选择 message 而非 task：`TeamSpec::can_send` 只认 message 通道，task 通道只影响 `can_delegate`
    （Leader 委派本来就无需通道）。
- `core/src/control.rs` 的补丁校验在两处拒绝成员间通道（`add_channel` 与 `add_agent` 内嵌
  `channels`）：source 与 target 都不是 Leader 即拒绝，错误提示引导改用共享空间。
- 代价与升级路径：成员之间无法直接对话；确有跨后端小组需求时，需显式放宽该校验，并把该流量纳入
  观察者投影后再放开。

证据：

- `engine/tests/chat_e2e.rs::review_add_agent_inherits_leader_tools_and_gets_channels`
  （省略→继承 Leader 绑定、显式 `[]`→保持空、双向 message 通道存在、成员之间不可发消息）；
- `core/tests/engine.rs::topology_patch_add_and_stale_reject`（成员间 `add_channel` 被拒）；
- 真实评测：`review/eval/runs/2026-09-15-deepseek/`（含 `team-collab` 前后对比）。

2026-09-20 补齐广播语义：动态新增的非 Leader `broadcast` 无论 targets 如何都被拒绝，
避免只写 Leader 目标却获得全队发送权。Leader 广播和历史导入行为保持原约定；本地回归见
[组队校验记录](../review/topology-validation-2026-09-20.md)，没有追加真实模型验收。

## D-34 Skills 配套范围（2026-09-17）

用户确认配套范围为 `K-Dense-AI/scientific-agent-skills` 科学技能集合、`browser-use`、`find-skills`。

沿用 D-23 的唯一用户级注册根 `~/.agents/skills`，科学技能按集合内具体名称发现与分发。

核验（2026-09-17）：本机注册目录有 165 个 SKILL.md：科学集合 163 个、`browser-use` 1 个、
`find-skills` 1 个。科学集合与 `find-skills` 的来源由 `~/.agents/.skill-lock.json` 确认；
`browser-use` 在目录中独立安装。用户配置为 `skills_paths = ["~/.agents/skills"]`。

## D-35 优先复杂编码任务与长任务稳定性（2026-09-17）

用户在本轮后续方向中选择「复杂编码任务与长任务稳定性」。在 D-32 的成熟度改进范围内，
优先处理长上下文约束保留、模型请求预算、中断恢复，以及多文件仓库任务的独立验收。
成功标准以可复跑的行为检查、原有测试与用户文件保护、真实任务完成率为准，不能以工具数量
或模拟服务通过数代替真实效果。

评测扩展采用 Agent 执行结束后的独立评分，隐藏检查不复制到其执行工作区；待评分代码仍在
原有 Linux 沙箱内构建和运行。保持 Rust 三 crate、既有权限边界与会话兼容性。
本选择不自动授权跨平台、新的后台终端协议、插件市场或跨会话记忆等产品范围变化。

2026-09-19 验收补充（D-32/D-35 与方案 §11/T7/T19 范围内）：增加 Rust 的显式启用
Chat 模型矩阵入口，使用完整所选 profile 与有来源的原生窗口，隔离项目/配置/状态，
独立断言文件、同会话续接与重建后的历史；缺配置和失败均留证据，不能以跳过替代通过。
本批仅实跑 DeepSeek Flash，不扩大供应商验收结论，见[真实模型矩阵记录](../review/live-models-2026-09-19.md)。

## D-36 模型评测使用原生上下文长度（2026-09-17）

用户明确要求：「使用任何模型测试时请使用其原生上下文长度」。所有后续模型评测按该模型
原生长度配置，记录数值及来源，不得自行缩小真实模型的上下文窗口。长度未知时先核实，
不能用通用 16K/64K 值替代。DeepSeek Flash 的 1M 长度由本轮用户确认，评测配置为 1,000,000。

不合理的非原生窗口模型测试应作废并删除，不能改名为压力实验或历史证据保留。
上下文压缩算法的本地假服务回归属于确定性逻辑检查，不代表任何真实模型的上下文能力
或模型评测结果；其独立复现的代码缺陷与回归检查正常保留。
