# Rust 更新缺陷修复与 /model 扩展

对应用户本轮“修复这些缺陷”，及追加的模型选择器、slash 菜单末项问题。修改基于
`a415099babfb80e4a39682a3c321e6bf84c27023`，未提交 git commit。

## 缺陷与回归证据

| 问题 | 修复 | 可复跑的回归检查 |
|---|---|---|
| P1 树历史惰性迁移重复/遗漏回合，叶节点失配使已提交动作重放 | 首次执行前固定树；检查点保存追加日志，恢复原子替换树前后的窗口；只用显式 rewind_epoch 作废检查点，其余冲突停止执行 | `chat_e2e::review_completed_turns_survive_tree_migration_and_restart`、`review_tree_commit_recovers_on_both_sides_of_rename`、`review_crash_after_committed_chat_action_must_not_replay_it` |
| P1 /model 摘除活动 runner 后取消失效 | 活动 runner 保持可达并标记过期；取消先撤销 RunSlot 的 TurnControl | `chat_e2e::review_model_override_preserves_cancellation_and_applies_next_turn`（真实 bwrap/shell，取消后无迟到写入）、`model_override::drop_runner_keeps_an_inflight_turn_alive` |
| P1 会话关闭并释放锁后，迟到压缩仍写树 | 树提交、恢复和检查点共用执行守卫 | `chat_e2e::review_late_compaction_cannot_write_after_session_close` |
| P2 压缩后缺少可发现的工具输出 ID | 摘要输入和结果保留 ID 索引；从当前分支完整祖先链生成，连续压缩仍能读回旧结果 | `chat_e2e::compaction_triggers_on_threshold_and_read_history_recovers_output`（两次压缩，模型从请求中找到 ID 再读回） |
| P2 MCP HTTP 缺少协商版本头 | initialize 响应的 protocolVersion 用于后续 POST（含 initialized） | `mcp_http::http_transport_binds_and_calls_tools`（服务端返回不同版本以排除硬编码） |
| P2 SSE 按行解析导致合法多行 JSON 丢失/串响应 | 按空行分事件、拼接 data 字段，支持 LF/CRLF/CR；响应匹配请求 ID，忽略通知和其他 ID | 同上（多行响应前后夹通知及其他 ID） |
| P2 Skills 的 YAML 多行描述不可搜索 | 复用 serde_yaml 解析 frontmatter，兼容折叠、字面量和引号；读取与输出长度仍受限 | `tools::tests::skill_tool_searches_and_reads_registry` |

## 追加的 /model 与 TUI 修复

- `/model` 打开“成员 → 供应商 → 模型 → 思考强度”选择器，Leader 优先，团队其他成员均可选择。
  支持文字搜索、粘贴、方向键、Enter、Esc 返回，以及恢复默认；模型名旁显示配置名以区分同名模型。
- 候选合并 config.toml 的 models 配置和供应商在线目录，按 provider 分组；选择 profile 会同时更换模型、
  endpoint、protocol、认证环境变量、生成选项和窗口大小。保留原有手输模型名的命令。
  当前回合继续，下一回合采用新配置；模型覆盖同步显示到成员表格与 Leader 输入框标题。
- Worker `model` 返回模型候选与成员当前覆盖；`set_model` 增加可选 profile；未知配置或档位拒绝修改。
  大写档位归一化，Anthropic 使用 output_config.effort，普通 Chat 使用 reasoning_effort。
- 进入供应商时后台 discover_models 请求在线目录：OpenAI 兼容的 GET models、Anthropic 的
  GET /v1/models 与 after_id 分页；复用认证环境变量，同一连接只请求一次，去重已配置及重复模型。
  在线候选使用已有 profile 加模型 ID 覆盖，按“在线”标记；配置候选立即可选，失败保留并显示原因。
  worker 网络请求单独线程运行；TUI 会话/代次校验丢弃迟到回复，合并不会移动已有选中项。
  最长 10 秒获取预算，单次 HTTP 上限 8 秒、每页 2 MB、最多 20 页；禁用重定向以免转发认证头。
- Codex 成员重建连接会显式恢复已有线程并传入 model/modelProvider，turn/start 也传 model。
  选择自定义 endpoint 时使用独立 provider 配置 ID，认证只引用环境变量。
- slash 菜单原本有 7 项但最多画 6 项：普通高度现在完整显示，矮屏滚动至选中项；
  最后一项继续按 ↓ 保持选中，测试同时检查实际高亮位置。

回归检查：`worker_protocol::worker_set_model_switches_and_clears_overrides`、
`model_override::set_model_override_applies_reports_and_clears`、
`session::tests::session_override_rewrites_chat_profile_and_codex_options`、
`codex_contract::model_switch_resumes_codex_thread_with_selected_provider_and_model`、
`app_tests::model_picker_selects_members_profiles_effort_and_restores_defaults`、
`render_tests::model_picker_keeps_selected_model_visible_and_renders_both_languages`、
`render_tests::slash_command_menu_lists_navigates_and_runs`。
动态获取另有 `session::tests::model_discovery_uses_auth_pagination_and_keeps_configured_models_on_failure`、
`worker_protocol::model_discovery_does_not_block_worker_requests`、
`app_tests::dynamic_models_merge_without_moving_selection_or_reviving_closed_pickers`。

## 验证

```bash
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml
cargo test --offline --manifest-path tui/Cargo.toml
# 集中验证恢复/跨供应商切换/连续压缩
cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e
# 真终端测试须先有 debug 二进制；使用不连真实供应商的本地配置
XDG_CONFIG_HOME=/tmp/teamagents-fix-pty-config XDG_STATE_HOME=/tmp/teamagents-fix-pty-smoke python3 tui/scripts/pty_smoke.py
XDG_CONFIG_HOME=/tmp/teamagents-fix-pty-config XDG_STATE_HOME=/tmp/teamagents-fix-pty-click python3 tui/scripts/pty_click_check.py
python3 /tmp/teamagents-model-pty-check.py
git diff --check
```

结果：core **43**、engine **116**、TUI **70** 通过；新增 10 项测试并增强已有用例。
两项仓库 PTY 脚本通过；临时 PTY 探针还走通了 `/` 末项 → `/model` → 供应商/模型/档位 →
在线目录 HTTP → 选择配置中不存在的模型 → worker 回执 → 重开选择器核对当前值。
临时配置和探针均在 `/tmp`；持续回归证据以上述 Rust 测试为准。
执行日志位于 `/tmp/teamagents-{core,engine,tui}-final.log`、`/tmp/teamagents-chat-final.log`、
`/tmp/teamagents-pty-{smoke,click,model}.log`。

一次完整 engine 检查遇到既有 `process_leaks` 脚本启动的 `Text file busy`；随后相同完整命令通过，
没有放宽断言或修改该用例。未调用真实付费模型；Codex 以本机生成的 app-server schema 和协议替身验证。

## 边界

- 不增加依赖，不修改 SQLite/TeamSpec 数据结构；新增检查点字段有 serde 默认值。旧树/检查点
  若已不一致且无法可靠恢复，将报 OUTCOME_UNKNOWN，避免重复副作用；不会自动修复已经丢失的历史。
- 模型在线发现限已配置供应商提供的 models 列表接口，复用对应 profile 的连接与生成选项；
  不新增 OAuth 登录或模型能力数据库。在线目录不落盘。
  （会话覆盖原不落盘；2026-09-14 起按 D-29 随会话保存，重开保留。）
- Codex 成员只列 OpenAI 兼容配置，所选端点须支持 Responses API；不把 Codex 运行时转换成 Chat 运行时。
  具体模型是否接受某个档位仍由服务端校验。MCP 未追加 GET 推送/DELETE 会话或旧式独立 SSE 传输。
- 工具输出 ID 索引目前完整放入上下文，若索引本身占据大量窗口再分页；代码保留了 ponytail 注释。

交互参考：[pi 模型选择器源码](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/modes/interactive/components/model-selector.ts)、
[Codex 模型/推理选择事件](https://github.com/openai/codex/blob/main/codex-rs/tui/src/app_event.rs)。
协议依据：[Anthropic effort](https://platform.claude.com/docs/en/build-with-claude/effort)、
[MCP 版本头](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#protocol-version-header)、
[SSE 事件解析](https://html.spec.whatwg.org/multipage/server-sent-events.html#interpreting-an-event-stream)；
模型目录：[OpenAI List models](https://developers.openai.com/api/reference/resources/models/methods/list)、
[Anthropic List models](https://platform.claude.com/docs/en/api/models/list)。
Codex 字段以本机 `codex app-server generate-json-schema --out /tmp/teamagents-codex-schema` 的
ThreadResumeParams/TurnStartParams 为核对依据。
