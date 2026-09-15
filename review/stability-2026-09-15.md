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
