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

## D-7 Python 要求提升到 `>=3.12` ✅（用户已确认 2026-09-12）

- 方案 §3 原文为 Python 3.11+；用户要求提升到 **3.12+**，避免偏旧的 Python 行为。
- 方案正文已同步（§3 现为 Python 3.12+）；实现侧同步：`pyproject.toml` 的
  `requires-python`、锁文件重解析、README 与本文档；
  本机开发/验收用 3.13.15，测试套件（65 确定性 + 12 实跑）在 3.13 上全绿。

## D-8 默认模型统一为 `deepseek-flash` ✅（用户已确认 2026-09-12）

- 用户要求：TeamAgents 的默认模型配置全部使用 `deepseek-flash`。
- 落地：`examples/config.toml` 与用户 `~/.config/teamagents/config.toml` 的
  `leader_main` / `research` / `coding` 全部指向 `deepseek-flash`
  （provider=deepseek，`generation_options = { reasoning_effort = "high" }`）；
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
