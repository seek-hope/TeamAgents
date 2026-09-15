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
