# Rust 追加审查修复台账（2026-09-13）

基准提交：`7ab510b4fac08277c63b4bed01017543639db433`。修复本轮确认的 6 项问题
（5 项 P1、1 项 P2），新增 10 项回归检查；不引入新依赖、不改变 SQLite schema。
本批为现有方案的缺陷修复，未新增方案偏离。

## 修复与证据

除特别标注外，测试均在 `engine/tests/chat_e2e.rs`。

| 问题 | 修复 | 回归测试 |
|---|---|---|
| P1 文件工具经悬空符号链接越界创建文件 | 路径解析拒绝无法解析的链接；读写工具通过 Linux `/proc/self/fd` 固定父目录，验证实际文件后才截断，创建使用 `create_new` | `tools_sandbox.rs::file_tools_reject_dangling_links_and_keep_in_root_links_working`：叶子/父目录悬空链接均被拒绝，目录外无新文件，目录内有效链接与普通嵌套文件仍可读写 |
| P1 Chat 崩溃恢复重复提交团队动作 | 执行前持久化模型工具调用及 ID，每个结果落盘；恢复按原 ID 查询核心回执；外部调用结果不明时停止自动重放 | `review_crash_after_committed_chat_action_must_not_replay_it`：真实 worker 杀进程并重启，含回执未记录到检查点的窗口，共享条目始终只有 1 条；`review_unknown_external_effect_is_not_replayed_after_crash`：真实 shell 已写一次后杀进程，恢复为 OUTCOME_UNKNOWN，无新模型调用；`review_completed_checkpoint_restores_reply_without_another_model_call`：已保存的最终答复直接恢复 |
| P1 会话关闭后旧回合继续执行 | 共享取消控制保护工具入口、检查点与重试；关闭时先撤销执行资格、关闭 MCP、等待活动工具退出并回收运行线程，再释放会话锁；停止迟到的通知与归档 | `review_closed_session_must_not_execute_late_tool_call`：延迟模型响应后重新取得会话锁，无迟到文件或事件，随后可恢复；`review_close_must_stop_running_shell_before_unlocking`：正在执行的 shell 也不能在关闭后继续写入 |
| P1 回合超时未停止在途 shell | 取消信号传入 shell 轮询，终止并回收 bwrap；确认工具退出后再将回合记为失败，无法确认则为 OUTCOME_UNKNOWN | `review_turn_timeout_must_stop_running_shell`：真实 bwrap 对照成功，回合 1 秒超时，shell 第 2 秒的写入未发生 |
| P1 成员配置 APPLIED 后仍沿用旧运行器/执行器 | state 暴露成员配置版本，运行器与执行器按 `config_revision` 重建；保留成员历史，执行层再次校验工具绑定 | `review_topology_update_must_rebuild_runner`：下一请求使用新模型、新指令及新工具列表；`review_executor_refreshes_workspace_and_revoked_bindings`：shared 切 isolated 后写入新根目录，撤销 files 后拒绝伪造文件调用 |
| P2 已绑定 MCP 工具未传给模型 | 将已加载绑定的名称纳入工具允许集合，保留其 schema 与说明 | `review_bound_mcp_must_be_advertised_to_model`：本地 stdio MCP 直接调用成功，模型请求含该工具，模型发起调用并收到 ping 结果 |

检查点使用写临时文件、`sync_all`、原子重命名和父目录同步；失败向上传播。
投递携带 event/delivery ID，检查点记录实际注入的投递，恢复时去重且只确认已注入的部分。
模型响应已保存但运行结果尚未归档时，直接恢复答复，避免额外请求。

兼容范围：旧版 `chat_history.json` 保持原格式；新增 `members/<成员>/turns/<回合>.json`。
旧版遗留的 RUNNING Chat 回合没有检查点时无法证明安全重放，收敛为 OUTCOME_UNKNOWN。
文件、shell、MCP 等外部调用没有持久化结果时同样如此，不能推断为失败并自动重试。
检查点暂存完整历史；历史压缩沿用既有保留差异，代码已标注升级条件。

## 可复跑验证

仓库根目录执行：

```bash
cargo test --offline --manifest-path core/Cargo.toml
cargo test --offline --manifest-path engine/Cargo.toml
cargo test --offline --manifest-path tui/Cargo.toml

# 仅本批新增检查：9 项 Chat + 1 项文件沙箱
cargo test --offline --manifest-path engine/Cargo.toml --test chat_e2e review_
cargo test --offline --manifest-path engine/Cargo.toml --test tools_sandbox file_tools_reject_dangling_links_and_keep_in_root_links_working
git diff --check
```

结果：core 38、engine 77（21 lib + 56 integration）、tui 55 项通过。
全部新增模型交互使用本地 HTTP 服务；bwrap、worker 进程和 MCP stdio 服务实际运行。
本批未调用真实模型供应商或启用 `TEAMAGENTS_LIVE_CODEX`，不将默认 live 测试门禁通过当成真实服务验收。

真终端验证使用独立 XDG 目录和本地不可达模型地址，避免读取个人模型配置：

```bash
mkdir -p /tmp/teamagents-fix-pty-config/teamagents
cat > /tmp/teamagents-fix-pty-config/teamagents/config.toml <<'EOF'
[models.leader_main]
provider = 'openai'
protocol = 'openai'
model = 'review-local-only'
base_url = 'http://127.0.0.1:9'
max_retries = 0
timeout = 2
EOF
XDG_CONFIG_HOME=/tmp/teamagents-fix-pty-config XDG_STATE_HOME=/tmp/teamagents-fix-pty-smoke python3 tui/scripts/pty_smoke.py
XDG_CONFIG_HOME=/tmp/teamagents-fix-pty-config XDG_STATE_HOME=/tmp/teamagents-fix-pty-click python3 tui/scripts/pty_click_check.py
```

两项均通过：启动、粘贴、发送、面板切换、批准跳转、终端恢复与退出；普通/滚动后的行点击、页签点击。
本机完整日志：`/tmp/teamagents-core-fix-test.log`、`/tmp/teamagents-engine-final.log`、
`/tmp/teamagents-tui-final.log`、`/tmp/teamagents-fix-pty-smoke.log`、`/tmp/teamagents-fix-pty-click.log`。
