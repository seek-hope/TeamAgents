# 实施决策记录（偏离与澄清）

基准文档：`TeamAgents-Implementation-Plan.zh-CN.md`。**任何偏离方案的做法，先与用户确认再实现。**
本文件只记录已确认的决策；未确认的候选方案写在对话里，不写进代码。

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

## D-10 放宽执行上限、删除无效配置 ✅（用户确认 2026-09-12）

用户认为原默认值过于保守，逐项给出新默认值：

- `TeamSpec.limits`：`max_parallel_workers` 4→**8**、`max_members` 16→**20**、
  `max_turns_per_goal` 200→**1000**、`max_model_steps_per_turn` 50→**200**、
  `turn_active_timeout_s` 900→**1200**、`cancel_confirm_timeout_s` 30→**60**。
- `ModelProfile.max_retries` 2→**5**（模型请求的 SDK 级重试）。
- 沙箱命令输出上限 100 KB→**200 KB**（超出仍转存 artifact 文件）。
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
     不引入 libc 绑定。
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
