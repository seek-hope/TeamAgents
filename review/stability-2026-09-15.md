# 成熟度与稳定性改进（2026-09-15）

授权：D-32。基线：core 50、engine 140（其中 live_codex 未启用时提前返回）、tui 80。

## 实施顺序

- [x] 有界分页读文件、唯一匹配编辑、原子写入与版本冲突检测、完整 shell 输出保存。
- [x] TUI 后台请求与过期响应保护；MCP 权限和生命周期边界。
- [x] 流式响应、上下文预检及溢出恢复、完整会话 fork、取消与恢复验证。
- [x] 编码交付验证、持久用量与耗时；协作重叠修改仍通过 worktree/Leader 串行安排。
- [x] 结构化非交互 CLI 与 CI 检查；`exec --json` 可被固定任务评测消费。

## 验证

实施后补充实际命令与结果；模拟服务测试和真实模型评测分开记录。`exec --json` 的退出码和验收命令结果可供 CI 直接消费。

## 已验证

- `cargo test --offline --manifest-path core/Cargo.toml`：50 项通过。
- `cargo test --offline --manifest-path engine/Cargo.toml`：156 项通过（含本轮新增
  `exec_tests::parked_approval_reports_approval_required_not_timeout`、
  `tools::tests::shell_artifact_stops_at_the_size_cap`）。
- `cargo test --offline --manifest-path tui/Cargo.toml`：83 项通过。
- `git diff --check`：通过。

## 保留边界

私有 Chat 子代理、五家真实供应商、磁盘配额和独立 SSE 推送仍未纳入本轮；真实模型评测见第三批（DeepSeek）。

## 发布前硬化（第二轮，2026-09-15）

只读复核 + 真实运行后修掉两处会直接误导使用者的问题：

1. **`exec --json` 卡在待批准直到超时**（`engine/src/cli.rs`）：非交互方式没有人能回应批准，
   原先会一直轮询到 `--timeout`（默认 1200s）再报 `timeout`/124，与文档写的“3 需要批准”不符。
   现在检测到待批准即结束本轮并返回 3；运行仍留在会话里，可用 `--resume` 在 TUI 批准后继续。
   回归：`cli::exec_tests::parked_approval_reports_approval_required_not_timeout`。
2. **Shell 制品无磁盘上限**（`engine/src/tools.rs::OutputSink`）：输出预览内存有界（200KB），
   但 `artifacts/exec-*.log` 会一直追加，一条失控命令即可写满磁盘。现在单个制品封顶 64 MiB，
   超出部分丢弃并在返回文本里标注 `artifact truncated at 64 MiB`。
   回归：`tools::tests::shell_artifact_stops_at_the_size_cap`。

### 真实运行证据（不是模拟）

使用本机 `DEEPSEEK_API_KEY` 的默认单成员团队，隔离状态目录 `/tmp/ta-live/state*`：

```bash
XDG_STATE_HOME=/tmp/ta-live/state2 engine/target/debug/teamagents exec --json \
  --cwd /tmp/ta-live/proj2 --full-auto --timeout 840 \
  --check 'python3 -c "import hello; assert hello.add(2,3)==5 and hello.add(-1,1)==0"' \
  --check 'test -s result.txt' - < /tmp/ta-live/prompt.txt
# 15.3s，status=completed，exit=0，两项验收均 ok（hello.py/result.txt 真实落盘）

# 同一任务改成要求 shell 带 network=true（默认策略不允许 → 需要批准）：
# 修复前：60s 后 exit=124，事件 approval_requested → run_cancelled（会话 proj_d0e1216de052）
# 修复后：1.6s 内 exit=3，status=approval_required（会话 proj_1b0d50411654）
```

实跑原始 JSONL 与 stdout 保留在 `/tmp/ta-live/out*.jsonl`；文档与 `docs/ACCEPTANCE.md`
按上述结果更新（默认团队 = `files`+`shell`+`web`，未含 codex 成员，多供应商矩阵仍未验收）。

## 已知剩余缺口

- 五家真实供应商矩阵、其它供应商的真实模型指标：需要凭据与时间，尚未执行（DeepSeek 已跑，见第三批）。
- 磁盘配额：现在只有单个制品 64 MiB 上限，没有会话级总量治理。
- 跨进程并发写同一文件：仍有进程内路径锁 + SHA-256 CAS，跨进程需 worktree 或外部锁。
- Chat 成员私有子代理、独立 SSE 推送（GET/DELETE）未实现。

## 第三批：沙箱工具链、exec 用量与真实评测（2026-09-15）

1. **沙箱里根本没法构建项目**（`engine/src/tools.rs`）：`$HOME` 在沙箱中不可见，而 rustup
   shim 需要 `RUSTUP_HOME` 才能选工具链、cargo 需要 registry/git 缓存才能离线构建，所以成员
   改完代码既不能编译也不能跑测试——"自己验证自己的改动"在这条路径上是假的。现在把
   `RUSTUP_HOME` 与 `CARGO_HOME` 的 `bin`/`registry`/`git` 只读镜像到沙箱
   `/tmp/.teamagents-toolchain/`（`/tmp` 是沙箱内 tmpfs，能容纳挂载点，`/home` 仍然不可见），
   并注入 `RUSTUP_HOME`/`CARGO_HOME` 与 `$CARGO_HOME/bin` 到 PATH。`credentials.toml` 与
   `config.toml` 不挂载，注册表令牌不会进入沙箱。
   回归：`tools::tests::sandbox_builds_with_the_host_toolchain`（把缓存目录换成空目录即失败，
   证明这条断言有牙齿）。
2. **`exec --json` 看不到用量**：result 行现在带 `duration_ms` 与 `usage`（各成员
   prompt/completion/total 与未知调用计数），与 TUI `/status` 同一份账本，评测和 CI 不必再猜 token。
3. **评测集从空壳变成可复跑**：`review/eval/tasks/<id>/{prompt.md,checks.txt,fixture/}` +
   `review/eval/run.sh`，三个任务各有真实验收（`cargo test`、configparser 逐段断言、大输出取值）。
   旧的 `tasks.jsonl` 里 `test -d .` 这类恒真检查已删除。
4. **`exec --json` 看不到"做了什么"**：只有 run 级事件时，自动化无法审计成员改了哪个文件、跑了
   哪条命令。现在 ChatRunner 在工具结果落地处把 `{tool, call_id, ok, error, arguments(≤500 字符)}`
   推给 `Notify::set_tool_sink`，`exec` 以 `type:"tool"` 行实时输出；TUI 仍走原来的文本流，
   互不影响。回归 `chat_e2e::tool_activity_reaches_the_automation_sink`。

### 本轮真实运行

```bash
cargo build --offline --manifest-path engine/Cargo.toml
review/eval/run.sh --timeout 600          # deepseek-flash，默认单成员团队
```

| 任务 | status | exit | 秒 | tokens(prompt/completion) | 验收 |
|---|---|---|---|---|---|
| edit-integrity | completed | 0 | 24.0 | 38530/3866 | 全部通过 |
| long-output | completed | 0 | 16.1 | 29753/2385 | 全部通过 |
| rust-fix | completed | 0 | 21.4 | 41421/3342 | 全部通过 |

原始 JSONL（含 `tool` 行）与逐任务复核：`review/eval/runs/2026-09-15-deepseek/`。

## 仍然没有做的

- 供应商矩阵：本机只有 DeepSeek 与 OpenAI 两个密钥，本轮只跑了默认 DeepSeek；其它供应商需要凭据。
- 多成员协作任务、被中断后的恢复、批准回路：评测集尚未覆盖。
- `tool` 行在"成员私有上下文"面上是新增暴露：它含工具名与参数摘要（写文件内容会被截到 500
  字符），派发敏感读任务时要意识到这条日志会被 CI 保存。

## 第四批：组队链路是断的（2026-09-15）

新增 `team-collab` 评测任务（两个独立子任务，要求 Leader 自己组队并行完成）后，第一次真实运行
**900 秒超时失败**（`review/eval/runs/` 的 REPORT 记录了两个版本的对比）：

1. **新成员没有执行工具**：`apply_topology_patch` 的 add_agent 省略 `tool_bindings` 时得到空
   绑定，成员只能收发消息/任务，改不了文件也跑不了命令，两个任务永远完不成，Leader 一直等
   （`wait_for_tasks` → 超时）。这是团队能力的主干道断点。
2. **补丁形状靠猜**：工具说明里没有 operation 形状，Leader 连续 5 次提交失败（`missing agent`、
   把 `patch_id` 当幂等键 → `unknown patch`、漏 `runtime_kind`），最后才试对。
3. **缺 `base_revision` 的报错误导**：省略时提示 "base_revision None is stale"，没说当前版本是几。
4. **成员能力不可见**：`<team>` 上下文只有 id/name/role/status，Leader 看不到成员有没有工具。

本轮只动"契约清晰度与可见性"，不放宽任何权限：工具说明写清 operation 形状、`base_revision`、
"不写 tool_bindings 就没有执行工具、没有 channel 就发不出消息"；`core/src/views.rs` 在
`relevant_topology.members[]` 里带上 `tools`；`core/src/control.rs` 对缺失 `base_revision` 的
补丁直接报出当前版本号（回归断言加在 `core/tests/engine.rs::topology_patch_add_and_stale_reject`）。
修完同一提示词：36 次工具调用、0 失败、40 秒通过（`review/eval/runs/2026-09-15-deepseek/`）。

仍待确认的默认值（属策略变更，等用户点头再落码）：add_agent 省略 `tool_bindings` 时是否继承
Leader 的绑定（D-30 已为 model_profile 开了同类先例）；是否自动为"Leader↔新成员"建通道。

顺带把两条"猜错就被拒、但不告诉你对的是什么"的报错改成自愈式（`core/src/control.rs`）：
`send_message` 被拒时列出当前可达成员；未知 shared space 时列出该成员可用的空间 id（只列它
自己有权使用的，不泄漏别的空间名）。回归 `core/tests/engine.rs::refused_messages_and_spaces_name_the_valid_options`。

## 第五批：组队默认值（D-33，用户确认后落地）

按用户 2026-09-15 的指示改默认值，并加了成员间通信约束：

1. `add_agent` **省略** `tool_bindings` → 继承 Leader 当前绑定（显式 `[]` 仍表示"只要团队工具"，
   保留可表达性）；这样"建出来啥也干不了"的静默失败不再可能出现。
2. `add_agent` 自动补 `leader→成员` 与 `成员→leader` 两条 **message** 通道（`can_send` 只认
   message；task 通道只影响 `can_delegate`，Leader 委派本来就无需通道），已存在时不重复。
3. 成员之间**不允许**直接通道：`core/src/control.rs` 在两处（`add_channel`、`add_agent` 内嵌
   `channels`）拒绝 source 与 target 都不是 Leader 的通道，错误提示引导改用共享空间。

回归：`chat_e2e::review_add_agent_inherits_leader_tools_and_gets_channels`、
`core/tests/engine.rs::topology_patch_add_and_stale_reject`（成员间通道被拒）。
真实复跑：`review/eval/runs/2026-09-15-deepseek-d33/`（team-collab 35s、33 次工具调用、0 失败、验收通过）。
决策记录：`docs/DECISIONS.md` D-33。

## 第六批：磁盘与并发（用户批准的优先级 ①②）

1. **制品目录无上限**（`engine/src/tools.rs::prune_artifacts`）：单制品封顶 64 MiB 只挡住了单个
   文件，长会话里每个超大命令都会留一个文件。现在新建制品时按 mtime 删最旧的 `exec-*.log`，
   直到目录总量回到 512 MiB 以内；只动自己写的 `exec-*.log`，删文件失败不影响命令结果
   （清理是尽力而为），被删引用的读回会正常报"文件不存在"。
   回归：`tools::tests::artifacts_are_pruned_to_the_directory_budget`。
2. **跨进程写同一文件**（`engine/src/tools.rs::with_path_lock`）：原来只有进程内互斥 + SHA-256
   CAS，另一个 teamagents 进程（或另一个终端的 `--resume`）写同一路径时看不到锁。现在每次
   写/编辑前先用 `File::try_lock` 抢该路径的建议锁（锁文件命名取自目标绝对路径的 SHA-256，
   放在 `sessions/<id>/locks/`，**不写进被编辑的项目目录**），争用时最多等 10 秒再报
   `another teamagents process is writing this file; retry`；文件系统不支持建议锁时退化为
   原行为，不因加锁失败拒绝写入。
   回归：`tools::tests::path_lock_serializes_two_writers`（两个写者严格串行、项目目录里没有
   锁文件残留）。

仍未做：历史检查点/对话树/team.db 的会话级总量配额（当前只有 artifacts 目录有预算）。

## 第七批：TUI 里能看见成员在做什么（用户批准的优先级 ③）

`exec --json` 的 `tool` 行只解决了自动化侧；界面侧原来只能看到 Leader 的流式文本，成员的
工具调用完全不可见。本轮把同一份活动接到界面上：

- `engine/src/worker.rs` 给 serve 协议新增 `push:"tool"`（agent_id/tool/ok/arguments）；
  TUI 的 `Push` 结构相应扩展，`main.rs` 分发到 `App::on_tool`。
- `tui/src/app.rs::on_tool` 把活动写成日志行（`·` 成功 / `✗` 失败，参数截断 120 字符），
  并尊重成员过滤；日志面板改为环形缓冲（`MAX_LOG_LINES = 2000`），长会话不再无限增长。
- 回归：`tui/tests::tool_activity_lands_in_the_log_panel`（含失败标记、参数可见、过滤生效）、
  `log_panel_keeps_only_the_newest_lines`。

真实端到端核对（真模型 + 真 serve 通道）：起 `teamagents serve`、发 `new_session` +
`user_message("用 shell 工具运行 ls -a 并简单报告")`，stdout 里拿到
`push:"tool" leader shell ok=true {"command":"ls -a"}` 与 `signal_done`，说明 TUI 走的就是这条数据。

仍未做：批准回路与被中断恢复的评测任务（优先级 ④）。

## 第八批：批准回路与被中断恢复的真实评测（用户批准的优先级 ④）

评测集补两个"停在安全边界"的任务，并给 runner 加了三个可选覆盖文件（`mode.txt`、
`timeout.txt`、`expect.txt`，见 `review/eval/README.md`）：

- `approval-gate`：提示词要求用 `network=true` 的 shell 联网。结果 status=approval_required、
  exit=3（2.1 秒返回，不是等超时），没有任何联网命令执行，验收命令另外断言没有伪造的 `200`。
- `interrupted-recovery`：提示词要求原样运行 `sh -c 'echo started > run.txt; sleep 60; echo done > run.txt'`，
  任务超时 45 秒。结果 exit=124，`run.txt` 只有 `started`，事件是
  `run_started → run_cancelled(CANCEL_REQUESTED)`，沙箱里的 sleep 随 bwrap 被杀（无残留进程）。

证据：`review/eval/runs/2026-09-15-deepseek-gates/`（原始 JSONL + 说明）。
这一批没有改产品代码，只补可复跑的验证与记录；优先级列表 ①–④ 至此全部有真实运行证据。

## 第九批：补齐第三种线上格式 OpenAI Responses（2026-09-15）

用户指出主流的模型 API 格式只有三种（responses / anthropic / chat-completion），三种都兼容后剩下
的就是各家订阅。核对结果：chat completions（`openai`/`deepseek`）与 Anthropic Messages 已有，
**Responses 完全没有实现**，于是补齐：

- `engine/src/stream.rs`：`Mode { Chat, Anthropic, Responses }`，新增 Responses 的 SSE 解码
  （`response.output_text.delta` 才 emit、推理增量不外发、`response.output_item.done` 收
  function_call、`response.completed` 收 usage 与最终 output、`response.failed` 直接报错、
  未完成的流一律视为错误不执行）；单 JSON 响应同样支持。
- `engine/src/chat.rs::chat_responses`：`POST {base_url}/responses`，`stream:true`、`store:false`；
  双向翻译 `to_responses_input`（system→`instructions`、assistant 文本→`output_text`、
  `tool_calls`→`function_call`、tool 结果→`function_call_output`）与 `from_responses_output`
  （`output` 里的 message/function_call → 引擎内部的 assistant/tool_calls）；
  `max_tokens`→`max_output_tokens`、`reasoning_effort`→`reasoning.effort`。
- 回归：`stream::tests::responses_stream_yields_text_calls_and_usage`（文本/私有推理/工具/usage、
  截断流报错、失败事件报错）、`chat_e2e::responses_protocol_round_trips_a_tool_call`
  （假 Responses 服务：工具调用→执行→结果按 `function_call_output` 回传→第二轮文本收尾，
  并断言 `instructions`、工具扁平化、`stream/store`）。
- 文档：USER-GUIDE 新增四种协议对照表（含端点与典型用途）、`examples/config.toml` 顶部说明、
  ACCEPTANCE 协议行、`core/src/models.rs` 的 protocol 注释。

未做：真实 Responses 订阅（OpenAI 官方/中转）的端到端验收——本机没有可用订阅，只有本地
faithful 假服务；Anthropic 官方订阅同样待凭据。

## 第十批：会话保留策略（用户批准的优先级 ①）

磁盘占用现在有两层：制品目录自动预算（第六批）与会话级保留。

- `engine/src/sessions.rs::prune_archived(days, base, dry_run)`：清理"最后更新超过 N 天"的
  **归档会话**，复用 `delete_session` 的既有保护（运行中的会话、带未合并 worktree 成果的会话
  跳过并报告原因），逐会话收集错误而不是中断整轮。
- 入口：`teamagents sessions prune --days 30 [--dry-run]`；用户配置 `[retention] archived_days = 30`
  则每次打开会话顺手清理（默认不写=不删）。配置字段加在 `core::models::UserConfig.retention`。
- 回归：`sessions::tests::retention_removes_only_old_archived_sessions`（只删超期归档、
  dry-run 不删、活跃会话目录不在遍历范围）；CLI 手工核对：dry-run 报告"将删除 proj_old（45 天前）"，
  实际执行后只剩未超期的 proj_fresh。
- 有意不做：回合检查点 / 对话树 / `team.db` 不自动删。核对过 `chat_e2e::
  review_completed_checkpoint_restores_reply_without_another_model_call` 这条路径：崩溃窗口里
  "已完成回合"仍可能被 reconcile 重新执行并用检查点里的回复收尾；删掉旧检查点会退化成重新调用
  模型并可能重复投递——代价高于省下的磁盘（文档 §3.3 已写明这条边界）。

## 第十一批：团队面板的"最近活动"列（用户批准的优先级 ③）

日志页签能看工具流水，但团队页签还得手动翻。现在团队面板多一列：每个成员最后一次工具调用
与距今时间（失败带 `✗`），来源与日志页签同一份 `push:"tool"` 数据。

- `tui/src/app.rs`：`tool_activity: HashMap<agent, {tool, ok, at}>` 在 `on_tool` 时更新，
  `team_rows()` 追加"最近活动"列（切换会话时清空）。
- `tui/src/i18n.rs`：表头与英文翻译各加一项。
- 回归：`tui::tests::team_panel_shows_each_members_last_tool`（未跑过是 `-`、成功/失败格式、
  行内列数与表头一致）。

## 第十二批：多模态看图和读图（用户批准的优先级 ②）

Codex/Claude Code 能看图，而 TeamAgents 的 `read_file` 只读 UTF-8 文本——UI 截图、流程图这类
任务直接受限。本轮补 `view_image`：

- `engine/src/tools.rs`：`view_image`（`files` 绑定）读 png/jpeg/gif/webp，按**魔数**判类型
  （标签错比拒绝更糟），单张 ≤5 MiB；结果只返回 `{image, media_type, bytes}` 引用，
  **base64 不落历史/检查点**，读时再加载并复核类型与路径（`load_image_reference` 复用同一套 root 校验）。
- `engine/src/chat.rs`：请求构建时把引用转成各协议的图片内容——chat completions 追加一条
  `image_url` 的 user 消息、Anthropic 放进 `tool_result` 的 image 块、Responses 放进
  `function_call_output` 的 `input_image`；`gateway.rs` 的 bound_tool 白名单加 `view_image`。
  另补零依赖的 base64（RFC 4648）。
- 回归：`tools::tests::images_are_classified_by_magic_bytes_and_bounded`（魔数/超限/非图片/穿越拒绝）、
  `chat::tests::base64_matches_the_rfc_vectors`、`chat::tests::image_references_become_protocol_image_parts`
  （两种协议的图片块形状 + 普通文本结果不受影响）、`chat_e2e::view_image_attaches_the_picture_to_the_next_request`
  （真实工作区文件 + 真实 executor，断言第二轮请求带 `data:image/png;base64,`）。
- 已知边界：图片会一直留在上下文里（每次请求都会重发），模型侧是否具备视觉能力取决于所选模型；
  真实视觉模型的端到端效果待有对应订阅后验收。

## 第十三批：持久 Shell 续用状态（用户批准的优先级 1）

沙箱是每条命令一个新进程，`cd`/`export` 用不上，日常手感与 Claude Code 差一截。做法不是常驻
shell 进程（取消/超时语义会被打乱），而是“状态随命令走”：

- `engine/src/tools.rs`：沙箱新增一个 rw 绑定（成员自己的 `members/<id>/shell/` → 沙箱内
  `/tmp/.teamagents-shell`，挂载点在私有 tmpfs 下，`/home` 依旧不可见）；命令被包一层
  前置 `. state.sh`（恢复 cwd 与导出变量）与后置捕获（`printf 'cd %q' "$PWD"` + `export -p`
  写入临时文件后 rename，再写一个 cwd 单文件）。输出前缀 `[cwd: …]` 告诉模型下一条命令的家。
- 取消/超时语义不变：杀掉 bwrap 即可，因为捕获没跑，状态就停在最后一条完成的命令上；
  半截写入也被 rename 挡住。
- 接线：`session.rs::member_executor_factory` 给每个成员传自己的 shell 目录
  （`member_executor_with_control`/`workspace_executor_with_control` 多一个可选参数，
  旧签名保留给测试与 MCP 复用，MCP 传 None）。
- 工具说明补一句：cwd 与导出变量会保留、输出以 `[cwd: …]` 开头。
- 回归：`tools_sandbox.rs::persistent_shell_keeps_cd_and_exports_between_commands`
  （cd/export 跨命令生效、无状态调用不受影响、**项目目录里没有任何状态文件**）、
  `an_interrupted_command_does_not_advance_the_shell_state`（中途取消后仍停在最后完成的目录）。

## 第十四批：多文件原子编辑（用户批准的优先级 2）

`edit_file` 一次只能改一个文件，重构（改名/改签名跨文件）要么来回多次、要么中途失败留下半成品。
新增 `edit_files`：

- 两阶段：先对每条 `{path, old_string, new_string, expected_sha256?}` 做唯一匹配与版本校验
  （不写盘），全部通过后再按**排序后的路径**逐个原子写入（排序是为了两个成员之间不死锁），
  任一条失败则一个字节都不落盘；同一文件一次只允许一条编辑（否则后一条会基于旧内容验证）。
- 返回各文件的 diff；`gateway` 的原生工具白名单加 `edit_files`；工具说明写清"全部校验通过才落盘"。
- 回归：`tools_sandbox.rs::batch_edits_are_all_or_nothing`（一条不匹配 → 两个文件都没变；
  全部匹配 → 两个文件都改并返回 diff；同文件两条 → 拒绝且文件不变）。

### 全量复跑（持久 Shell 之后）

`review/eval/runs/2026-09-15-deepseek-shellstate/`：6 个任务全部符合预期（4 completed + 2 个
刻意的安全边界，即 approval-gate exit 3、interrupted-recovery exit 124），0 个失败工具调用；
其中 `rust-fix`/`long-output` 真实用到 shell，证明状态捕获包装没有改变命令语义与退出码。

## 第十五批：MCP GET 推送流与 DELETE 会话终止（用户批准的优先级 3）

规范里最后两块没实现的东西补齐（POST 的 SSE 早已支持）：

- `engine/src/mcp.rs`：初始化并发出 `notifications/initialized` 之后，客户端开一条 **GET SSE 流**
  （带会话 id / 协议版本 / bearer；`timeout_connect=2s`、`timeout_read=1s`，读超时同时充当停止信号轮询）；
  服务器通知写一行 stderr，服务器发来的**请求**按规范回 `-32601 client does not support <method>`，
  避免服务器永久等待一个客户端并不具备的能力。`session`/`protocol` 改为 `Arc<Mutex<..>>` 以便推送线程复用。
- `close()` 先停推送线程（最多等 3 秒，超时则不 join 以免卡住退出），再带会话 id 发 **DELETE**；
  405 视为"服务器不支持"静默通过，其它错误只写 stderr。stdio 的关闭语义不变。
- 服务器回 405（不支持推送/DELETE）时按无推送处理，连接与调用照常。
- 回归：`mcp_http.rs::http_push_stream_answers_requests_and_deletes_the_session`（GET 带会话 id、
  被推送的 sampling 请求收到错误回复、关闭时 DELETE 带上会话 id）、
  `http_transport_tolerates_servers_without_push_or_delete`（405 不破坏任何功能）；
  原有 HTTP 用例（POST SSE、session id、token、malformed JSON）继续通过。

## 第十六批：Codex 成员的真实跨后端评测（用户指定：不用官方订阅）

补上唯一没有真实运行覆盖的主干路径：Leader（Chat）与 Codex 成员协作。用户要求 Codex 成员走
`codex --profile deepseek` 而不是官方订阅，实测发现两件事：

1. **当前 CLI 拒绝 `codex --profile X app-server`**（`--profile` 只适用于 runtime 命令与
   `codex mcp`），所以"profile"落法是：`model_profile.codex_profile = "<name>"` →
   引擎读 `$CODEX_HOME/<name>.config.toml` 并展开成 `-c key=value`（嵌套表→点号键）传给
   app-server。profile 文件缺失/为空会明确报错。这样成员跑在 DeepSeek provider 上，完全不碰
   官方订阅。回归：`session::tests::codex_profile_layers_into_config_overrides`、
   `codex::tests::app_server_args_reach_the_subcommand`。
2. **回复与摘要的词间空格/重复**：`item/agentMessage/delta` 是连续片段、`item/completed` 又给
   同一段完整文本，原先 `pieces.join(" ")` 得到 "I 'll  start  by ..."，摘要里同一段还出现两遍。
   现在按 run 累积单个文本缓冲（deltas 直接拼接；item 文本只在不是尾部时追加），回复、
   `complete_task` 摘要与外部进度都取自它；错误信息仍走原 progress 列表。
   回归：`codex::tests::streamed_deltas_and_the_completed_item_make_one_clean_reply`。
3. 顺手：`codex app-server exited` 现在带最后 3 行 stderr（坏参数与崩溃不再无法区分——本轮就是
   靠这个定位到 `--profile` 不可用的）。

真实评测：`review/eval/runs/2026-09-15-deepseek-codex/`（completed、exit 0、37.9s、验收通过；
Leader 复现失败 → assign_task → codex-dev 修 `mul` → Leader 复跑 `cargo test` → signal_done）。

## 第十七批：事件钩子（用户批准的优先级 2）

Codex 有 `notify`、Claude Code 有 hooks，TeamAgents 之前没有任何外部集成点。

- `engine/src/hooks.rs`：`[hooks] notify = [argv]`，事件发生时把 `{event, session_id, payload}` 写到
  钩子 stdin，事件名作为最后一个参数；每次事件一个分离线程，10 秒未结束杀掉，失败只写 stderr，
  **不阻塞回合**。钩子是用户自己写的程序，在主机上以用户权限运行（不进沙箱）。
- 事件来源：`Notify` 新增独立的事件 sink（`set_event_sink`），与 UI 的 tool/stream sink 互不干扰
  ——TUI/exec 可以替换 tool sink，钩子照常触发。`ChatRunner` 在每个工具结果处发 `tool_call`；
  `Runtime::finalize` 发 `run_completed`/`run_failed`/`run_cancelled`/`run_paused`；
  `Runtime::submit` 对团队动作发 `team_action`（含 assign_task/complete_task/signal_done 的受理结果）。
- 配置：`core::models::UserConfig.hooks.notify`（`[hooks]` 段，默认空=不启用）。
- 回归：`hooks::tests::hooks_receive_the_event_name_and_json_on_stdin`（argv/stdin 内容、未配置时不启用）、
  `chat_e2e::configured_hooks_see_tool_calls_and_turn_end`（真实会话里钩子收到 tool_call 与 run_completed）。

## 第十八批：会话数据库保留（用户批准的优先级 4）

`team.db` 的投递账本与事件流随会话长期增长（一条长会话几十万行不稀奇）。新增
`Store::prune_history(session_id, days, dry_run)`：

- 只删 **已受理（applied）** 且超过 N 天的投递；**未受理的投递与其依赖的事件一定保留**，
  所以崩溃重放语义不变。事件用「没有未受理投递引用它」作为删除条件，逐条判定。
- 有删除就 VACUUM（`execute_batch`，不在事务里）；dry-run 只统计。
- 入口：`teamagents sessions prune --days N --history-days M [--dry-run]`（对全部会话逐个处理，
  运行中的会话跳过并报原因）、以及 `[retention] history_days = M`（打开会话时清理该会话，此时持有会话锁）。
- 回归：`storage::tests::history_pruning_keeps_pending_deliveries_and_their_events`
  （dry-run 不删；已受理投递+旧事件被删；未受理投递的旧事件保留；近期行保留）。
  真实核对：拿一份 completed 会话库把时间戳前移 3 天后 `--history-days 2`，dry-run 报
  "4 条投递、15 个事件"，实跑后 VACUUM 完成、库缩到 0.3 MB。
- 有意保留：事件流同时是 TUI 日志与审计原料，所以默认不清理、天数由用户给。

## 第十九批：TUI 改动审查弹层（用户批准的优先级 3）

工具的 diff 之前只出现在工具结果文本里，界面上没有专门视图。现在：

- 工具活动载荷增加有界的 `result`（≤2000 字符）：`chat.rs` 在 `tool_call` 事件里带上，
  `worker.rs` 的 push 与 `exec --json` 的 `tool` 行一并透出（自动化侧也能看到改动内容）。
- TUI：`App::record_review` 记录 `edit_file`/`edit_files`/`write_file` 的结果；团队页签（或日志页签）
  选中成员按 `v` 打开弹层 `render_review_overlay`（`+` 绿 / `-` 红 / `edited …` 高亮标题行），
  `Esc` 关闭、`Ctrl+U/Ctrl+D`、`↑/↓` 滚动；没有记录时提示"还没有可审查的改动"。
- 回归：`tui::tests::review_overlay_shows_the_last_edit_diff`（无改动时打不开、记录 diff、
  面板键路径 `v` 打开、Esc 关闭）。
- 边界（`ponytail:` 注释已标）：每个成员只保留最近一次编辑批次，不是完整历史；要看历史仍有
  日志页签与工具结果。

## 第二十批：成员计划（update_plan）+ 上区计划状态组件（用户要求的下一轮）

- `update_plan(items)`：所有 Chat 成员可用（运行时工具，不需要绑定）。计划存
  `members/<id>/plan.json`（原子写），每轮以 `<plan>` 块回灌到系统提示（`[x]/[~]/[ ]` 标记），
  更新时发 `plan_updated` 事件（hooks 可见）与 `push:"plan"`（UI 实时）；worker 的 `state` 回复
  里也带上 `plans`，所以重连后立刻能看到。
- 定位说明：计划是成员的**工作记忆**（像 Codex 的 update_plan），不是团队任务语义——"谁欠谁什么"
  仍由 core 的任务图负责，两者不混。
- TUI：面板框下方新增**独立一行状态组件**（`ui::geometry` 里预留 `plan` 矩形，只在有成员有
  计划时占用，并从对话区高度里扣）：`计划 1/3 · leader  修 mul`；显示谁的计划取决于面板高亮
  （否则是当前有回合的成员，再否则 Leader）。
- 回归：`chat::tests::plan_round_trips_into_the_prompt_block`（读写、`<plan>` 块、模型每轮可见）、
  `tui::tests::plan_status_strip_tracks_the_selected_member`（进度/当前项、有才占行、行从对话区扣、全部完成）。
- 真实运行：`review/eval/tasks/plan-use`（新增）——模型三次 `update_plan`，两项都标 done，任务通过，
  证据 `review/eval/runs/2026-09-15-deepseek-plan/`。

## 第二十一批：Codex 成员 × 批准回路 / 被中断恢复（用户要求的组合评测）

- `team-codex-gate`：让 Codex 成员做**工作目录之外**的写入 → Codex 沙箱（workspace-write）只能
  申请批准 → app-server 权限请求 → 引擎按 D-31 落 PENDING 批准、回合停在 WAITING_APPROVAL →
  非交互 exec 以 3 结束（14s，不是等超时）。外部可见后果：目标文件不存在、工作区无伪造成功记录。
  过程发现并记录：第一版让 Codex 成员跑 `curl` 却真的成功了，因为用户 Codex 配置里
  `[sandbox_workspace_write] network_access = true`——Codex 成员的联网走它自己的沙箱，不经过
  团队批准门；任务因此改成"写工作目录之外"。
- `team-codex-interrupt`：把 `sleep 60` 命令派给 Codex 成员，任务超时 45s → exit 124；
  `run.txt` 只有 started、两个回合都记为 CANCELLED、`ps` 无残留 `sleep`。
- 证据：`review/eval/runs/2026-09-15-deepseek-codex-gates/`。仍未覆盖：中断后 `--resume` 重放。

## 第二十二批：中断后 resume 的两阶段评测（连出两个真缺陷）

评测 runner 新增 `resume.md` 支持（阶段 1 故意超时 → 阶段 2 `exec --resume` 继续），任务
`resume-continue`：阶段 1 修 alpha + 追加一次 `alpha done` + `sleep 90` 被超时打断；阶段 2 修 beta
并要求确认 `alpha done` 只有一行。**第一次跑发现两个缺陷**：

1. **OUTCOME_UNKNOWN 回合无法结清**（`core/src/control.rs`）：`signal_done` 的完成检查把
   "outcome-unknown operations" 当阻塞，但没有任何动作能清掉它 —— 一旦某回合在命令中途被中断，
   该会话再也无法完成目标。现在 `cancel_run` 接受 OUTCOME_UNKNOWN：置 CANCELLED、事件带
   `acknowledged_outcome_unknown: true`（人工确认），已是终态的回合仍拒绝；阻塞信息直接写出
   "用 cancel_run 结清哪个 run"。
2. **失败回执丢掉详情**（`engine/src/chat.rs::tool_result_content`）：阻塞项在回执的 `result` 里，
   而工具结果只回传 `error`，模型只能猜 run id（实测连猜 5 个全错、白烧 20 万 token）。
   现在失败且 `result` 非空时以 `detail` 一并回传。
3. 评测 runner：`resume.md` + `expect-resume.txt`，汇总表只统计阶段 2。

修复后同一任务：阶段 2 completed / exit 0，`progress.txt` 恰好一行 `alpha done`（**无重复副作用**），
`goal_done`，证据 `review/eval/runs/2026-09-15-deepseek-resume/`。回归：
`core/tests/engine.rs::acknowledging_an_unknown_run_unblocks_completion`、
`chat::tests::refused_tool_receipts_keep_their_detail`。

## 第二十三批：结果不明回合的界面与自动化出口

上一批修了"OUTCOME_UNKNOWN 挡死 signal_done 且无法结清"；这一批把它补到人能看到、能操作的地方：

- **TUI**：团队页签的状态列在有未知回合时显示 `结果不明（c 结清）`（英文 `unknown (c to ack)`），
  选中按 `c` 提交 `cancel_run`（actor=user，core 本来也允许用户结清），回执 `acknowledged` 时提示
  已结清、否则显示错误；没有未知回合时提示"该成员没有结果不明的回合"。
- **`exec --json`**：结果行新增 `outcome_unknown: [run_id…]`。之所以要它：CI/脚本会看到
  `status:"failed"`，但原因（未知回合需要人工确认）只在运行状态里，给出 id 才能一键结清。
- 回归：`tui::tests::unknown_outcome_runs_are_visible_and_acknowledgeable`（状态列标记、
  面板键 `c` 产出 AcknowledgeRun、无未知回合时的提示）、`cli::exec_tests` 增补 `unknown_run_ids`。

## 第二十四批：计划清单弹层（p）

状态条只显示"进度 + 当前进行中"的一项；要看整个清单，现在在团队页签选中成员按 `p` 弹出
与"改动审查"同一个弹层（三态标记 `[x]/[~]/[ ]`，Esc 关闭、Ctrl+U/Ctrl+D 与 ↑/↓ 滚动）。
没有计划的成员会提示"还没有计划"。团队页签的按键提示同步更新，并补了英文翻译
（渲染测试 `ascii_frame_has_no_cjk_leaks` 就是靠这条翻译漏检抓出来的）。
回归：`tui::tests::plan_overlay_shows_the_whole_list`。

## 第二十五批：BLOCKED 任务的自救路径写进回执与文档

起因：仓库备忘里曾写"BLOCKED 任务任何 Agent 都无法结清、唯一路径是用户侧 CANCEL_TASK"。
用 `core/tests/engine.rs::blocked_tasks_are_recoverable_by_the_leader` 把事实钉下来：承接者确实
不能再 `complete_task`，但 **Leader 可以 `cancel_task` 结清**，然后重新派一次（新任务 id）。

- `core/src/control.rs`：承接者对 BLOCKED 任务的 `complete_task` 拒绝信息现在直接写明
  "ask the Leader to cancel_task <id> and assign the work again"（自愈式报错，和第二十二批同一套路）。
- `engine/src/chat.rs`：`cancel_task` 的工具说明补上"BLOCKED 任务只能这样清掉，清掉后重新派新任务"。
- 文档：USER-GUIDE 故障处理新增一行（Leader 结清 + 重派，用户也可面板按 `c`）；ACCEPTANCE 对应
  补一句。仓库 AGENTS.md 里的备忘本身已是正确版本（Leader 可用 cancel_task），无需改。

## 第二十六批：成员级中断 → 任务 BLOCKED → Leader 自救（真实评测）

新增两阶段任务 `resume-task-recovery`（leader + dev 两个 Chat 成员的 TeamSpec）：阶段 1 把
`sleep 90` 的命令派给 dev 并被会话超时打断；阶段 2 resume 后**全模型驱动**地完成恢复：

`task_blocked(external turn outcome could not be confirmed)` → Leader `cancel_task` →
重派一个不需要等待的补救任务 → dev `echo done >> run.txt` + `complete_task` → Leader 自己 `cat` 复核 →
`signal_done` 先被拒（还有未知 run）→ `cancel_run` 结清两个 → `signal_done` 通过 → `goal_done`。
验收：`run.txt` 恰好一行 `started` + 一行 `done`（被中断命令的副作用**没有重复**）。
用时 64.6s，证据 `review/eval/runs/2026-09-15-deepseek-task-recovery/`。

顺带修复：阻塞信息里 run id 原先是 `成员:run_id` 的写法，模型照抄整串去 `cancel_run`（实测被拒两次），
现在改成 `run_id of 成员`（`core/tests/engine.rs::acknowledging_an_unknown_run_unblocks_completion`
补了断言）。

至此四条恢复路径都有真实运行证据：回合内重启（第七批）、批准回路与被中断恢复（第八批）、
中断后 `--resume`（第二十二批）、成员级中断 + 任务自救（本批）。

## 第二十七批：两个"配了也不生效"的配置缺陷（真实运行才发现）

起因是想给钩子补一条真实运行证据（此前只有假模型测试）。用最小运行一验，钩子根本没触发。
顺着查出来**两个真缺陷**：

1. `engine/src/config.rs::parse_user_config` 只挑 `models/tools/skills_paths/instruction_files`
   四个顶层键，`[retention]` 与 `[hooks]` 被静默丢掉 → 这两项在配置文件里配了也不生效
   （第十批的 retention、第十七批的 hooks 都受此影响；CLI 的 `sessions prune` 因为不走配置所以当时看着是好的）。
2. 更隐蔽的一处：`load_user_config_for`（`open_session` 真正用的加载器）自己重建 catalog，
   **压根没搬运这两个键**。现在按用户配置优先搬运，并且**项目配置里的这两段被有意忽略**
   （钩子会执行命令、保留策略会删数据，克隆下来的仓库不该有这种权力）。
3. 防回归：`config::tests::every_user_config_field_is_accepted` 用 `UserConfig::default()` 的
   序列化键集合对 `CATALOG_KEYS` 做双向断言——以后再往 UserConfig 加字段却忘了加进筛选列表，
   测试立刻失败（这正是本次缺陷的成因）。另有 `user_hooks_and_retention_survive_loading_and_project_ones_are_ignored`。

真实运行验证（两条都复跑过）：
- 钩子：`XDG_CONFIG_HOME=<含 [hooks] 的配置> teamagents exec --json --cwd ... "只回答：你好"` →
  `/tmp/ta-hook-log2.txt` 收到 `team_action`、`tool_call`、`run_completed` 三个事件与 JSON 载荷。
- 保留：把一份归档会话的 mtime 前移 40 天、配置 `[retention] archived_days = 30`，
  打开会话后该归档目录被清掉。

## 第二十八批：Codex 后端上的成员级中断恢复（真实评测）

把第二十六批的链路在 Codex 成员上复跑（`resume-codex-recovery`）：resume 时 codex-dev 与 leader
两个回合都落 `OUTCOME_UNKNOWN` → `task_blocked` → Leader `cancel_task` + 重派收尾任务 →
codex-dev `task_completed` → Leader 复验 + `cancel_run` 结清两个未知 run → `signal_done` → `goal_done`；
`run.txt` 恰好一行 `started` + 一行 `done`（副作用无重复），58.7s，证据
`review/eval/runs/2026-09-15-deepseek-codex-resume/`。

至此**两个后端**（Chat / Codex）的"成员级中断 → 任务自救 → 目标完成"都有真实运行证据。

## 第二十九批：pre_tool 策略钩子（执行前可拦截）

`[hooks] notify` 只能"事后知道"；现在补上 **`pre_tool`**：原生工具执行前用同一份 JSON 询问用户的
策略脚本，`exit 0` 放行、`exit 2` 拒绝（stderr 第一行作为原因回给模型，回执里带
`denied_by: pre_tool_hook`）、其它情况（非 0/2、启动失败、超时 10 秒）放行并打日志——写坏的钩子
不该让团队停工。被拒的调用**不会到达执行器**。

- `core::models::Hooks.pre_tool`（仍只在用户配置生效，项目配置里会被忽略）；
- `engine/src/hooks.rs::Hooks::deny_reason`：阻塞式判定；stderr 用共享缓冲读，**只 join 至多 1 秒**
  （否则钩子里再起一个持有管道的孙进程，会把一次调用拖到子进程生命周期那么长——测试第一版就踩了这个）；
- `gateway.rs`：`ToolGateway` 多一个 `hooks` 字段，`call` 在 executor 之前过闸；
  `runtime.rs::set_hooks` 由 session 注入（与 notify sink 同一个 Hooks 对象）。
- 回归：`hooks::tests::pre_tool_policy_decides_by_exit_code`（0/2/7/挂死四态 + notify-only 不算策略）、
  `gateway::tests::pre_tool_hook_denies_before_the_executor_runs`（执行器一次都没被调用）。
- 真实运行验证：配置 `pre_tool` 拒绝一切原生工具后，让模型 `write_file` 创建文件 →
  工具回执 `ok=False`、原因 `denied by pre_tool hook: policy: file writes are not allowed…`，
  工作目录**保持为空**（写入从未发生）。

## 第三十批：精简要每轮都付的固定提示开销

系统提示里的"Team tools available"清单原本是**每个工具一行描述**——而同样的描述已经随请求的
function schemas 发了一遍，等于每轮重复。改成只列工具名（描述仍随 schemas 走）后，实测
系统提示 3227 → 471 字符（`chat::tests::prompt_overhead_stays_lean` 把上限钉在 6000 字符，
防止它再长回来）。

- 全套 13 个任务复跑无回归（`review/eval/runs/2026-09-15-deepseek-lean/`，验收全过，
  含必须读懂工具契约的 team-collab / team-codex）。可比对照：plan-use 的 prompt token
  77,540 → 67,413（−13%），team-collab 116,651 → 113,917（−2.3%）；差值不大是因为历史本身占大头，
  固定开销这一项降了 85%。

## 第三十一批：团队成员上下文用量可见

压缩是自动的，但用户此前只能在 `/status` 里翻账本。现在 worker 把用量快照并入 `state` 回复，
团队页签的模型列在有 `context_window` 时追加百分比（`deepseek-flash 43%`，≥80% 用警示色）——
某个成员快触发压缩这件事变成一眼可见。回归 `tui::tests::team_panel_shows_context_usage_per_member`
（有窗口才显示、无窗口/无用量保持原样、高占用带警示色）。

## 第三十二批：文档基线与 doctor 钩子自检

- `AGENTS.md` 的"快速命令"补上 `sessions prune --history-days`、`review/eval/run.sh`，并把基线数字
  从 core 50 / engine 140 / tui 80 更新为 **54 / 187 / 91**（这份文件是每个 agent 的入口，过时数字
  会误导后续判断）。
- `teamagents doctor` 新增两项检查：`[hooks]` 里配置的 notify/pre_tool 程序是否存在（含 PATH 里的
  相对名）且可执行，以及是否配置了 retention（打印天数）。理由：钩子配错此前只在事件发生时才在
  stderr 露一行，极易漏看。回归：`engine/tests/cli.rs::doctor_probes_isolation_codex_and_config_errors`
  增补 hooks/retention 断言。
