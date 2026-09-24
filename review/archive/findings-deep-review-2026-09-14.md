# TeamAgents 深度审查报告（2026-09-14）

审查方式：5 路并行（core / engine 运行时 / engine 工具权限安全 / tui / Codex CLI 对比），
其中 engine 运行时切片由主会话完成（子代理两次 429 失败）。审查基于**工作区当前状态**
（含未提交的 skills 机制改动）。基线：core 38 / engine 78 / tui 55 全绿。
探针：/tmp/probe_core.py、/tmp/tui_probe、/tmp/ta-probe*、/tmp/ta-review-*。

级别定义：P0 致命 / P1 必须修 / P2 应尽快修 / P3 可缓。

---

## 一、P1（必须修，共 5 项）

### P1-1 views.rs:56 — 观察者 payload_scope 拼写错误时裁剪 fail-open，全量 payload 泄露
`scope_payload` 的 `_ =>` 分支对任何未识别字符串返回完整 payload；`TeamSpec::validate`
不校验 `payload_scope` 取值。探针：`payload_scope: "bogus-typo"` 的观察者收到含
`"summary":"TOP-SECRET-SUMMARY"` 的完整载荷。
**修**：`_ =>` 改为按 `"status"` 裁剪（fail-closed）+ validate 白名单校验。

### P1-2 tools.rs:966 — web_fetch SSRF 守卫只守第一跳，重定向绕过
`guard_url` 只校验初始 URL，ureq 默认跟随 10 次重定向，后续跳不检查
`is_private_addr` → `302 → http://169.254.169.254/` 可读云元数据。探针证实跟随链无再校验。
**修**：`redirects(0)` 自建 agent，手工跟随并对每跳 `guard_url`。

### P1-3 config.rs:191-195 — 项目配置 instruction_files/skills_paths 无信任门，任意本地文件注入全员提示词
`[tools]` 有 `trust_project_tools` 门，但项目 config.toml 可直接写
`instruction_files = ["~/.aws/credentials"]`；文件前 8000 字符进每个成员 system prompt
→ 发往模型 API 且落盘 checkpoint。继承自 Python 原版的设计缺口。
**修**：项目来源的两项同样走信任门，或只接受项目目录内相对路径。

### P1-4 tui/app.rs:451 — 流式缓冲按字节截断，中文长回复 panic 整个 TUI
`buf.drain(..cut)` 的 cut 是字节偏移，中文 3 字节/字必然截在 UTF-8 序列中间。
Leader 回复超 ~10.7k 汉字即崩。探针：`on_delta("汉".repeat(12000))` → PANIC。
**修**：截断前回退到字符边界（`floor_char_boundary`，Rust 1.80+）。

### P1-5 tui/main.rs:280 — 事件轮询游标退化为 0，从未打开日志面板时每 250ms 拉全量事件
`after = app.cursor.min(app.log_cursor)`，`log_cursor` 不点日志页永远是 0 →
全量事件每 250ms 传输+克隆，随会话长度平方级增长。
**修**：日志页未激活时用 `after = app.cursor`。

---

## 二、P2（应尽快修，共 10 项）

| # | 位置 | 问题 | 一句话修复 |
|---|------|------|-----------|
| P2-1 | control.rs:327 | `complete_task` 不强制 run_id → 回执假成功、任务落 BLOCKED 卡死 | validate 加 run_id 必填 |
| P2-2 | control.rs:1300 | `cancel_task` 不覆盖 QUEUED 回合，取消后任务仍被执行 | 取消路径同时置 QUEUED 回合 cancel_requested |
| P2-3 | control.rs:396+1796 | WAITING_BOUNDARY 补丁不可拒绝 + 批准挂起回合算 live → 拓扑变更无限卡死 | 允许拒绝 boundary 补丁或排除批准挂起回合 |
| P2-4 | codex.rs:130-139 | app-server initialize 失败泄漏子进程+两个读线程（无 Drop，线程持 Arc），ensure_server 重试再漏 | start 错误路径先 close() 再返回 Err |
| P2-5 | mcp.rs:80-86 | MCP initialize 超时后子进程与读线程双泄漏（同类） | 同上 |
| P2-6 | tools.rs:344 | `skill` 工具经符号链接越出 skills_paths 根（read/search 均受影响；member_context 同构） | 收录前 canonicalize + starts_with 校验 |
| P2-7 | main.rs:88-106 | 从当前目录找 TUI 二进制并执行 → 他人仓库 cwd 下的本地代码执行 | 搜索根去掉 cwd |
| P2-8 | session.rs:380 | 每次成员工具调用拉全量会话状态（序列化 1000 事件+占 core 全局锁） | core 加 `agent_config_revision` 轻量端点 |
| P2-9 | runtime.rs:317+384 | run_loop 空闲 20Hz 全量 `core.state()`（与 P2-8 同根因：state() 太重） | 轻量摘要端点或空闲降频 |
| P2-10 | tui/app.rs:371 | 批准到达的 toast+响铃运行中永不触发（读旧快照判断） | 对传入的新快照判断 |
| P2-11 | tui/app.rs:904 | 任务重排后点击命中错行（命中端用缓存旧索引，渲染端用 key 解析当前位置）→ 可能取消错任务 | 命中端先按 key 解析当前索引 |
| P2-12 | tui/worker.rs:93 | 引擎断连无界面反馈，UI 线程同步调用最长冻结 120s；两条断连提示 i18n 文案零引用 | 轮询失败计数提示 + submit 挪后台线程 |
| P2-13 | tui/app.rs:1494 | 共享空间面板溢出一屏后无任何途径滚动 | shared_rows 补稳定 key 进 panel_row_keys |

（编号沿用子代理原始发现，顺序不表优先级。）

## 三、P3（可缓，选要）

- storage.rs:444 动作去重只查 action_id 不比对 payload_hash（同 id 不同 payload 拿旧回执）
- control.rs:851 `read_shared` limit 接受负数（SQLite LIMIT -1 = 无限制）
- control.rs:65 `pbool` 空数组/空对象判真（`"reject": []` 意外拒绝补丁）
- control.rs 调度路径 N+1（per-agent 全表扫）；≤20 成员无害
- server.rs:205 `expire_approval` 端点直写库不走 Control（无审计事件、可 strand 回合）
- storage.rs:848/246 两处 panic 路径（同事务内不可达 / 刻意 fail-fast）
- control.rs:1944 `begin_run_inner` 对非 QUEUED 重复调用无守卫
- tools.rs:698 shell timeout 传 u64::MAX 溢出 panic（catch_unwind 兜住但自杀回合）
- sessions.rs:268 session_id 无格式校验 → 目录穿越（`delete_session("../victim")` 成功）
- tools.rs:644 save_artifact 文件名秒级时间戳+pid，同秒碰撞互截
- tools.rs:377 skill_index 每次调用全量重建（规模小可缓）
- tools.rs:165 glob_walk 跟随符号链接目录（TOCTOU 竞争窗口，只读低危）
- 技能优先级两通道不一致：注入侧 member-first，skill 工具侧 user-first
- web_fetch 大页面 10MB 上限硬错误而非截断；guard_url DNS rebinding TOCTOU
- gateway.rs:94 `PermissionPolicy.require_approval` 死配置面
- codex.rs:180 call 写失败 pending 槽泄漏
- tui：i18n `{error}`/`{v0}` 占位不一致 1 处；slash 菜单 Esc 状态不复位；chat/log 缓冲无上限；
  render_chat live_area 宽度越界 1 列（当前靠 wrap 宽度掩盖）；死代码 4 处；main.rs 每 120ms 无条件全帧重绘

## 四、已排查证伪/确认无问题（抽样）

- SQL 注入面无问题：全部绑定参数，format! 拼接仅占位符与自有枚举（storage.rs:610 有注释）
- `resolve_in_root`+`open_member_file` 对 `../`/绝对路径/悬空链接/TOCTOU 换链均有防护
- shell 仅 bwrap、缺失即报错不降级；env 白名单不含密钥；network=true 必须批准且绑定精确参数哈希
- web/MCP 执行层确为 fail-closed；MCP 子进程 env 白名单+shellshock 过滤
- run_id 进 checkpoint 路径前有字符集校验；runtime catch_unwind + 取消确认超时（RT-06）
- full-auto 只认用户配置，项目文件无法开启；`[permissions]` 类型错即报错
- 等待中的回合不占并发额度（is_active 仅 Queued|Running）；模型 HTTP 有 profile.timeout 兜底
- bound.call 的 expect 不可达（names 只含 MCP 工具项）
- 仓库与 git 历史无硬编码密钥；依赖树极小（engine 直连 10 个）

## 五、与 Codex CLI 的差距（功能缺口）

### A. 方案外新能力（Codex 有，TeamAgents 无）

| # | 缺失项 | 重要性 | 说明 |
|---|--------|--------|------|
| A1 | Token 用量/成本可见性 | 高 | 全仓库无 usage 追踪；团队把消耗乘以成员数，目前完全黑盒。Codex：`/status`、`/usage`、`limit_tokens` 预算 |
| A2 | 上下文自动压缩 | 高 | 成员历史每回合全量快照无上限（chat.rs:277 ponytail 注释、D-21），长会话确定溢出模型窗口。Codex：auto compact + `/compact` |
| A3 | 命令级审批规则（prefix/pattern） | 高 | 批准绑死精确 operation_hash，"会话内批准"只重放完全相同参数。Codex：execpolicy `.rules` prefix_rule |
| A4 | 会话 fork/回滚 | 中 | 无 fork/undo，worktree 合并是唯一回滚。Codex：`/fork`、`/diff`、review 面板 |
| A5 | 非交互 exec 模式 | 中 | `--plain` 无结构化事件输出/退出语义契约。Codex：`codex exec --json` |
| A6 | 会话内切换模型/档位 | 中 | 模型绑死 profile，Codex 成员 effort 固定。Codex：`/model`、`/fast` |
| A7 | 通知钩子 | 低 | 批准积压时有用。Codex：`notify = [...]` |
| A8 | 网络逐域名过滤 | 低 | shell 网络 allow/deny 二元（方案 §12.2 明确不承诺） |
| A9 | 后台终端任务 | 低 | shell 同步+超时，长输出落 artifacts |
| A10 | 历史搜索/记忆 | 低 | 输入历史 500 条，无会话内容搜索 |

### B. 方案已规划未实现

| # | 项 | 重要性 | 依据 |
|---|----|--------|------|
| B1 | MCP HTTP/SSE 远程传输 | 中 | 方案 §12.1 明确写了；mcp.rs 仅 connect_stdio；远程 MCP 生态整体不可用 |
| B2 | 五家模型契约测试缺口 | 中 | T7🔶 缺密钥 skip（ACCEPTANCE.md） |
| B3 | deepagents 图框架/通用子代理 | 低 | RECONSTRUCT 标有意简化；注意 Codex 已内置 subagents，但校验拓扑/ACL/观察者语义仍是 TeamAgents 差异化 |
| B4 | MCP 超时/凭据注入可配置 | 低 | mcp.rs 硬编码 60s/120s |

## 六、总体评价

- **健壮性**：core 单事务边界清晰、存储枚举 fail-closed、panic 面有 catch_unwind 兜底；
  剩余风险集中在状态机边角（QUEUED 取消、无 run 完成、boundary 补丁僵持）与进程泄漏（MCP/codex initialize 失败路径）。
- **安全性**：沙箱/批准链/密钥处理设计扎实（fail-closed、bwrap 不降级、env 白名单、历史无密钥）；
  实质性缺口 5 处：观察者 scope fail-open、SSRF 重定向、项目配置注入、skill 符号链接、cwd 执行二进制。
- **效率**：依赖树与线程模型干净；系统性问题一个——`core.state()` 全量序列化被 run_loop(20Hz)、
  每次工具调用、TUI 轮询三处高频调用，叠加事件/历史无界增长，长会话确定性劣化。
- **对 Codex CLI**：团队编排语义（校验拓扑/信息权限/崩溃恢复）是独有差异化；
  最大的现实差距是 A1（token 黑盒）、A2（上下文无压缩）、A3（审批粒度），建议按此序补齐。
