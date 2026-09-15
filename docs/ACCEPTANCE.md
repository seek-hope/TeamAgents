# 验收对照表（T1–T24）

基准：方案 §17；当前 Rust 实现于 **2026-09-15** 核对。
✅ = 所列路径有自动化证据，不代表场景中的每项发布条件均已证明；🔶 = 部分覆盖/存在已知缺口；⚠ = 尚未实现。
历史实测结果按当时日期保留，本次未重新运行真实模型/API 或 PTY 冒烟。

## 当前离线基线

在仓库根目录运行（本次 Rust 1.95.0，bubblewrap 与 python3 可用）：

```bash
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml
cargo test --offline --manifest-path tui/Cargo.toml
```

| crate | Cargo 报告通过 | 组成 / 实际执行范围 |
|---|---:|---|
| core | 50 | 20 库单测 + 30 集成测试 |
| engine | — | 全套离线集成与库测试；其中 `live_codex` 的 1 项未设开关即提前返回，真实服务需单独运行 |
| tui | 83 | 14 库单测 + 3 入口单测 + 37 app + 29 render |

**skip 不计入真实验收**：部分测试在缺依赖/开关时直接 `return`，Cargo 仍显示 passed。
真实 Codex 检查需显式运行以下命令；T7 的五家真实模型闭环没有可在导出密钥后统一运行的专用套件。

```bash
TEAMAGENTS_LIVE_CODEX=1 cargo test --manifest-path engine/Cargo.toml --test live_codex -- --nocapture
# 真终端检查另行运行，须先构建 engine 与 tui
python3 tui/scripts/pty_smoke.py
python3 tui/scripts/pty_click_check.py
```

## T1–T24 证据

| ID | 场景 | 状态 | 证据 / 说明 |
|---|---|---|---|
| T1 | Leader 委派给 B 并汇总 | ✅ | `engine/tests/scenarios.rs::t1_delegation_and_summary_full_lifecycle` |
| T2 | B/C 并行；慢任务不阻塞 Leader | ✅ | `scenarios.rs::t2_parallel_members_and_mid_run_supplement`（含执行中补充） |
| T3 | B/C 讨论；重复投递不重复注入；越界阻止 | ✅ | `scenarios.rs::t3_channel_enforcement_and_exactly_once_delivery` |
| T4 | 观察者只收到授权事件与载荷 | ✅ | `scenarios.rs::t4_observer_scoped_events_without_extra_rights` |
| T5 | 共享空间发布/发现/权限/引用 | ✅ | `scenarios.rs::t5_shared_space_permissions_and_discovery` |
| T6 | 信息隔离；新会话不继承 | 🔶 | `engine/tests/scenarios.rs` 的 T3/T4 覆盖通道与观察裁剪；`fork_rewind.rs::fork_carries_spec_and_leader_tree_but_not_team_facts` 覆盖分叉不复制团队事实；尚无覆盖本场景全部条件的专属用例 |
| T7 | 五家模型工具调用与续接；同队混用 | 🔶 | 真实 DeepSeek 单 Leader 回合实测（`--plain` 到 `goal_done`）；Anthropic 原生协议已实现（转换单测，缺密钥未实跑）；其余三家走 OpenAI 兼容路径未逐一实跑 |
| T8 | 恢复：杀进程后重建 | ✅ | `engine/tests/recovery.rs::t8_killed_turn_is_reconciled_and_stays_exactly_once`（kill -9 → 重启 requeue 重跑 → 取消收敛） |
| T9 | 单 Leader 可执行并继续对话 | ✅ | `scenarios.rs::t9_baseline_leader_alone_executes_and_keeps_talking` |
| 跨后端组队（Codex 成员） | `review/eval/tasks/team-codex/`：Leader（Chat）委派给 `runtime_kind: codex` 的成员，成员经 `codex_profile` 跑在 DeepSeek 后端（`codex app-server -c ...` 展开 `$CODEX_HOME/<name>.config.toml`，不使用官方订阅），真实运行 completed/exit 0/37.9s/验收通过（`review/eval/runs/2026-09-15-deepseek-codex/`）。顺带修：deltas 与 `item/completed` 的文本合并（原先词间空格 + 重复）、app-server 退出错误带 stderr 尾部 | 组合边界也验过：`team-codex-gate`（工作目录之外的写入 → app-server 请求批准 → 引擎 PENDING → 非交互 exit 3，写入从未发生、也无伪造记录）与 `team-codex-interrupt`（派给 Codex 成员的 sleep 被超时打断：run.txt 只有 started、两个回合都记为取消、无残留进程），证据 `review/eval/runs/2026-09-15-deepseek-codex-gates/`。
| T10 | 自然语言组队；非法结构被拒绝 | 🔶 | 核心校验与 `validate`；`chat_e2e.rs::review_add_agent_auto_creates_member_profile` 覆盖 D-30 自动 profile、`review_add_agent_inherits_leader_tools_and_gets_channels` 覆盖 D-33 默认值（省略 `tool_bindings` 继承 Leader 绑定、显式 `[]` 保持空、自动双向 message 通道、成员间通道被拒）。真实自然语言组队已跑：`team-collab` 首轮暴露"成员无执行工具 + 补丁形状靠猜"导致 900s 超时，修好工具契约与成员 `tools` 可见性后同一提示词 40s 通过（36 次工具调用、0 失败，见 `review/eval/runs/2026-09-15-deepseek/`）。未知 profile 在 add_agent 中按 D-30 解释为模型 ID，不保证远端模型存在 |
| T11 | 动态变更：成员只能提议、Leader 应用、边界生效 | ✅ | `engine/tests/topology.rs::t11_member_proposal_is_leader_decision`（提案→Leader 应用→生效，审计保留提案人） |
| T12 | 版本冲突不互相覆盖、不半应用 | ✅ | `topology.rs::t12_conflicting_patches_never_partially_apply` |
| T13 | 移除成员：停止后移除、任务移交、成果保留 | ✅ | `topology.rs::t13_removed_member_hands_tasks_to_leader_and_keeps_results` |
| T14 | 执行中补充；仅相关成员按边界调整 | ✅ | `scenarios.rs::t2_...`（supplement 到运行中的 Leader） |
| T15 | 工具批准：自动/越界暂停/其他成员继续/拒绝 | ✅ | 审批门单测 + Codex 批准 park/decide 用例；ChatRunner 端到端：`engine/tests/chat_e2e.rs::once_approval_is_consumed_and_the_turn_completes`（once 执行后消费）、`expired_once_approval_requires_a_new_request`（EXPIRED 重请求）、`denied_approval_blocks_the_operation`；Codex 超时闭环 `codex_contract.rs::codex_approval_timeout_expires_the_row` |
| T16 | 全自动只能用户开启；仍守系统权限与 ACL | ✅ | `scenarios.rs::full_auto_toggle_reaches_the_approval_gate`；仅 `actor=user` 可改模式（`core/src/control.rs` 校验） |
| T17 | Codex 成员：任务/进度/结果/批准/取消/恢复映射 | 🔶 | `engine/tests/codex_adapter.rs`（simple/approval/slow）；重启收敛 `codex_contract.rs::reconcile_reads_the_thread_history`、进程组清理 `closing_the_app_server_kills_its_process_group`、D-31 `codex_session_grant_auto_accepts_the_identical_operation`；真实 CLI `live_codex.rs` 本次未启用 |
| T18 | 工作目录 shared/isolated/worktree；脏输入不被忽略 | ✅ | `engine/src/workspace.rs` + 单测（worktree 生命周期/复用/合并/未合并拒绝清理/脏仓库回退 shared）；`tui` 会话删除守卫同源 |
| 多模态输入 | `view_image`（`files` 绑定）读取 png/jpeg/gif/webp（≤5 MiB，按魔数判类型，越界/超限/非图片都拒绝），结果只存路径引用，请求构建时按协议转成图片内容：chat completions → user 消息 `image_url`、Anthropic → `tool_result` image 块、Responses → `function_call_output` 的 `input_image`。回归 `chat_e2e::view_image_attaches_the_picture_to_the_next_request`、`chat::tests::image_references_become_protocol_image_parts`、`tools::tests::images_are_classified_by_magic_bytes_and_bounded`。真实视觉模型的端到端效果待有对应订阅后验收 |
| T19 | 工具生态：文件/Shell/搜索/抓取/MCP/Skills 真实任务 | 🔶 | files/shell/web_search/web_fetch；MCP stdio（`engine/tests/mcp_tools.rs` 启动本地测试服务器、`mcp_stdio.rs` 检查环境白名单/stderr）与 HTTP（`mcp_http.rs` 本地模拟服务）；Chat Skills/AGENTS.md 注入（`session.rs` 单测）；web fail-closed（`tools_sandbox.rs::web_tools_are_fail_closed_and_ordered_by_member_binding`）、长输出 artifacts（`long_shell_output_is_stored_as_a_readable_artifact`）；不等于所有工具已完成真实远端任务 |
| T20 | TUI：流式期间输入/导航/批准可用；窄屏、多行中文 | 🔶 | TUI 83 项单测 + TestBackend 帧；后台请求使用有界队列，停滞 worker 回归验证输入/退出仍响应。真实模型 + 真界面仍需 PTY/服务验收 |
| T21 | 崩溃去重：动作回执丢失仍只产生一次变更 | ✅ | `recovery.rs::t8_...` 断言重放步骤不重复产生副作用（shared 条目仍为 1 条） |
| T22 | 资源与失败：限流/超时/成员失败/无人就绪/超限 | ✅ | `recovery.rs::t22_goal_turn_budget_is_enforced`（LIMIT_REACHED）+ 取消/暂停场景 + 模型步数上限 `chat_e2e.rs::model_step_limit_reports_limit_reached`（超限 → `limit_reached` + FAILED）+ 活动超时中断 `timeout_interrupts_the_member_before_further_side_effects`、崩溃不误报超时 `a_crashed_member_is_not_reported_as_a_timeout` |BLOCKED 任务（成员回合被中断所致）由 Leader `cancel_task` 结清后重派，承接者的 `complete_task` 会被拒绝并提示该路径（回归 `core/tests/engine.rs::blocked_tasks_are_recoverable_by_the_leader`）；
| T23 | 权限执行：穿越/符号链接/Shell 越界/MCP 未授权 | 🔶 | `tools.rs` bwrap/路径/版本冲突回归，MCP workspace 模式目录与网络默认隔离，显式 host 才可离开沙箱；远端 HTTP 仍由服务授权，不能宣称覆盖服务端权限 |
| T24 | 会话复用；同名新成员不继承旧身份 | 🔶 | 复用的 `context_epoch` 机制在核心；fork 回归覆盖历史键重映射；仍缺专属同名成员复用场景 |

**尚未实现（⚠）**：Chat 成员私有子代理、Codex/Chat 五家真实服务的完整发布验收。
MCP HTTP（streamable，2025-06-18）：POST 响应中的 SSE、GET 主动推送流与 DELETE 会话终止均已实现——GET 流建立后服务器通知写日志，服务器发来的请求（如 sampling/roots）按规范回 JSON-RPC 错误而不是让服务器干等；关闭时 DELETE 结束会话；不支持推送的服务器回 405 时按无推送处理（`mcp_http.rs::http_push_stream_answers_requests_and_deletes_the_session`、`http_transport_tolerates_servers_without_push_or_delete`）。旧式独立 SSE 传输已从规范移除，绑定直接报错。
TUI 为 Rust 原生设计（D-20：固定分区、滚动、胶囊状态）。其余取舍见
`docs/DECISIONS.md`（D-17/D-19/D-20/D-21）。

## 当前实现与方案的已知差异

以下是代码现状记录，**不表示新偏离已获批准，也不将原方案的发布条件改为已完成**。

| 范围 | 当前行为与证据 |
|---|---|
| 模型流式与协议完整性（§11、T7） | `engine/src/stream.rs` 支持三种线上格式（chat completions / Anthropic Messages / OpenAI Responses）的 SSE 与单 JSON 响应，保留 thinking/signature 与 usage；`chat.rs` 为 Responses 做双向翻译（`instructions`、工具扁平化、`function_call`↔`tool_calls`），回归 `chat_e2e::responses_protocol_round_trips_a_tool_call` 与 `stream::tests::responses_stream_yields_text_calls_and_usage`。真实订阅闭环仍待各自凭据 |
| 动态变更安全边界（§8） | `core/src/control.rs::agent_has_live_run` 不把无 `external_turn_id` 的 WAITING_TASK/WAITING_APPROVAL 算作活动执行，因此 Chat 挂起时可应用 patch；有外部回合 ID 的 Codex 等待仍阻塞。现行修复证据：`core/tests/engine.rs::approval_parked_run_does_not_block_boundary`、`task_wait_parked_run_does_not_block_boundary` |
| 网关与 MCP 隔离（§12.2） | `BoundTools::load_in` 将 stdio MCP 默认放入成员 workspace bwrap（无网），`mcp_execution = "host"` 才显式使用宿主；绑定仍是授权边界 |
| 全自动与越界批准（§12.2） | 原生文件工具仍由 `tools.rs::resolve_in_root` 限定路径，Shell 始终走 `shell_run_with_control` / `bwrap_argv`；full_auto 只跳过批准门，没有扩大文件根或取消原生 Shell 沙箱 |
| 多文件原子编辑 | `edit_files`（`files` 绑定）：先对每个文件做唯一匹配 + `expected_sha256` 校验，全部通过才在排序后的路径锁下逐个原子写入；任一失败则一个字节都不落盘，同一文件一次只允许一条编辑。回归 `tools_sandbox.rs::batch_edits_are_all_or_nothing` |
| 共享目录并发写（§12.3） | `tools.rs::workspace_executor_with_control` 提供进程内路径锁、SHA-256 CAS 与原子替换；`tools.rs::with_path_lock` 追加跨进程建议锁（锁文件在会话状态目录 `sessions/<id>/locks/`，不落项目目录，`File::try_lock` 争用等待上限 10s，FS 不支持时退化为进程内互斥），回归 `tools::tests::path_lock_serializes_two_writers`。外部编辑器（不走本工具的写）仍建议使用 worktree |
| Skills 后端范围（§10、§12.1） | `session.rs::make_runner_factory` 在 Codex 分支提前返回；`member_context` 和 `BoundTools` 的 Skills/指令注入仅用于 Chat 成员 |
| TUI 完整视图与响应（§13） | 日志页签实时显示成员工具活动（`push:"tool"` → `App::on_tool`，失败标 `✗`，尊重成员过滤，环形缓冲 2000 行，回归 `tui::tests::tool_activity_lands_in_the_log_panel`）；六个管理页签；`/settings` 仅切语言，`/model` 单独选模型。成员记录通过日志事件筛选，没有完整私有对话树浏览器；控制请求已通过有界异步队列，停滞 worker 有超时和过期响应保护 | 改动审查弹层（团队/日志页签选中成员按 `v`，显示其最近一次编辑的 diff，回归 `tui::tests::review_overlay_shows_the_last_edit_diff`）； 结果不明的回合在团队页签标出并可按 `c` 结清（`cancel_run` 的 acknowledgement，回归 `tui::tests::unknown_outcome_runs_are_visible_and_acknowledgeable`，`exec` 结果行带 `outcome_unknown` 列表、回归 `cli::exec_tests`）； 计划状态条（面板框下方独立一行：`计划 done/total · 成员 · 进行中的项`，数据来自 `update_plan` 的 push 与 `state.plans`，回归 `tui::tests::plan_status_strip_tracks_the_selected_member`）；
| 回退节点（D-26） | `chat.rs::ChatTree::rewind_to` 将 leaf 设为目标节点，包含该条输入；TUI 文案已改为“保留该条输入，移开后续对话”。`rewind_points` 只列当前祖先链，旧分支需已知节点 ID 才能访问 |
| 分叉与会话模型（D-26/D-29/D-30） | `fork_session` 复制 Leader 树、TeamSpec、会话 profiles/overrides，并在打开失败时保留源会话；团队任务/运行事实仍重新开始。真实失败注入仍需补充 |
| 工具输出读回（D-28） | 工具完整结果先写入私有历史，模型上下文仅使用有界 head/tail；`read_history` 支持分页取回完整结果。 |
| 配置校验与导出（§5.2、§14） | `cli.rs::validate_spec` 已合并 TeamSpec 所在目录的受信任项目配置；任务依赖/委派权限在动作提交时校验。仍没有专用 TeamSpec 导出 CLI，部分通用 payload 未覆盖完整 schema |
| 成员计划（update_plan） | 成员记录自己的分步计划（`members/<id>/plan.json`，每轮以 `<plan>` 块回灌；`plan_updated` 事件也进 hooks），TUI 以独立状态条展示；真实运行：模型三次更新计划并把两项都做完（`review/eval/runs/2026-09-15-deepseek-plan/`，回归 `chat::tests::plan_round_trips_into_the_prompt_block`） |
| 事件钩子 | `[hooks] notify = [argv]`：`tool_call`、`run_*` 终态、`team_action` 三类事件把 JSON 写到钩子 stdin（事件名作最后一个参数），主机权限运行、10 秒超时、失败只写 stderr 且不影响回合。回归 `chat_e2e::configured_hooks_see_tool_calls_and_turn_end`、`hooks::tests::hooks_receive_the_event_name_and_json_on_stdin` | `[hooks]`/`[retention]` 只在用户配置生效，项目配置里的相应段落会被忽略并提示（克隆的仓库不能装钩子），回归 `config::tests::every_user_config_field_is_accepted`、`user_hooks_and_retention_survive_loading_and_project_ones_are_ignored`；
 成员级中断的服务链另有独立任务 `resume-task-recovery`：dev 的回合被超时打断 → `task_blocked`（reason=external turn outcome could not be confirmed）→ Leader `cancel_task` → 重派一个『不需要等待』的任务 → dev `complete_task` → Leader 复核 + 结清两个未知 run + `signal_done`，`run.txt` 恰好一行 `started`（副作用无重复），证据 `review/eval/runs/2026-09-15-deepseek-task-recovery/`。| 中断后继续（resume） | 阶段 1 故意超时中断、阶段 2 `exec --resume` 继续同一会话：副作用恰好一次（`progress.txt` 只有一行 `alpha done`）、工作继续完成、`goal_done`（`review/eval/runs/2026-09-15-deepseek-resume/`）。配套修复：`cancel_run` 可结清 OUTCOME_UNKNOWN 回合（`acknowledged_outcome_unknown`），失败回执把 `result` 作为 `detail` 回传（否则模型看不到阻塞项与 run id），回归 `core/tests/engine.rs::acknowledging_an_unknown_run_unblocks_completion`、`chat::tests::refused_tool_receipts_keep_their_detail` | 该链路在 Codex 后端也验过（`resume-codex-recovery`：两个回合都落 OUTCOME_UNKNOWN → task_blocked → Leader cancel_task + 重派 → codex-dev 完成 → 结清两个 run → goal_done，证据 `review/eval/runs/2026-09-15-deepseek-codex-resume/`）。
| 结构化执行与评测 | `teamagents exec --json` 输出稳定 JSONL（`session`/`tool`/`event`/`result`）：`tool` 行给出每次工具调用的名称、参数摘要（≤500 字符）与成功/失败，`result` 行给出退出码、`duration_ms`、各成员 `usage` 与验收命令结果（另写入会话目录 `verification.json`）。工具活动经 `Notify::set_tool_sink` 从 ChatRunner 直达（回归 `chat_e2e::tool_activity_reaches_the_automation_sink`）；回合停在待批准时立即以退出码 3 结束（`cli.rs::exec_outcome` + `exec_tests::parked_approval_reports_approval_required_not_timeout`），不再等到超时报 124。固定任务集 `review/eval/tasks/<id>/` + `review/eval/run.sh`；DeepSeek 真实跑 3/3 completed、验收全通过、16–24s、原始 JSONL 见 `review/eval/runs/2026-09-15-deepseek/`。多供应商矩阵、多成员协作任务与被中断恢复仍未验收 |
| 沙箱内构建工具链 | `$HOME` 不可见时成员仍能真的构建：`tools.rs::toolchain_mounts` 把 `RUSTUP_HOME` 与 `CARGO_HOME` 的 `bin`/`registry`/`git` 只读镜像到沙箱 `/tmp/.teamagents-toolchain/` 并注入环境变量（`credentials.toml`/`config.toml` 不挂载，令牌不进沙箱）；回归 `tools::tests::sandbox_builds_with_the_host_toolchain`（在沙箱里跑 `cargo test --offline`）。只有 Rust 已覆盖，nvm/pyenv 等 HOME 级工具链仍不可见 |
| Shell 续用状态 | 成员的 `cd`/`export` 跨命令保留（状态在会话目录 `members/<id>/shell/`，不进项目目录，随会话持久；命令输出带 `[cwd: …]`），被中断的命令不更新状态（临时文件 + rename）。回归 `tools_sandbox.rs::persistent_shell_keeps_cd_and_exports_between_commands`、`an_interrupted_command_does_not_advance_the_shell_state` |
| Shell 长输出与制品 | 有界预览 200KB 落 `artifacts/exec-*.log`，单个制品上限 64 MiB，超过部分丢弃并在输出中标注（`tools.rs::OutputSink` + `shell_artifact_stops_at_the_size_cap`）；整目录预算 512 MiB，新建制品时按 mtime 删最旧的 `exec-*.log`（`prune_artifacts` + `artifacts_are_pruned_to_the_directory_budget`）。历史检查点/对话树仍无配额 |
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
