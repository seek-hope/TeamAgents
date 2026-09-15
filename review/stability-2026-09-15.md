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

私有 Chat 子代理、五家真实供应商、磁盘配额和独立 SSE 推送仍未纳入本轮；真实模型评测需配置凭据后单独执行。

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

- 五家真实供应商矩阵、`review/eval/tasks.jsonl` 固定任务集的真实模型指标：需要凭据与时间，尚未执行。
- 磁盘配额：现在只有单个制品 64 MiB 上限，没有会话级总量治理。
- 跨进程并发写同一文件：仍有进程内路径锁 + SHA-256 CAS，跨进程需 worktree 或外部锁。
- Chat 成员私有子代理、独立 SSE 推送（GET/DELETE）未实现。
