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
