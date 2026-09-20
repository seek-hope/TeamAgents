# Codex + DeepSeek 完成后冷恢复（2026-09-19）

测试入口：
[`live_codex_turn_and_cold_recovery_through_app_server`](../../../../engine/tests/live_codex.rs)。
实现及确定性回归见[恢复修复记录](../../../codex-recovery-2026-09-19.md)。

配置：本机 Codex **0.155.0**，`deepseek-flash` / `deepseek`，模型原生上下文
**1,000,000**，来源为用户确认的 [D-36](../../../../docs/DECISIONS.md)。
所有尝试都使用相同原生窗口；未运行五供应商矩阵或竞品对照。

任务固定：用 Codex 原生工具将 `recovered-once\n` 追加到 `proof.txt`，读回核实，
最终输出 `LIVE_RECOVERY_OK`。外部回合完成后，不归档本地 RUNNING 回合，关闭会话与运行器；
随后使用生产 `open_session` 重开，检查同一外部线程、COMPLETED 回合、SUCCEEDED 任务及原文件仍恰好一行。
这是完成后归档窗口的真实验证；实际 SIGKILL 与断线时序另用持久化假服务验证，不能混算。

| 尝试 | 结果 | 证据 |
|---|---|---|
| 外层工具沙箱内 | 240 秒超时，未通过 | [日志](sandbox-timeout.log)。Codex sandbox 报 bubblewrap mount-lock 只读错误，转为待批准；检查时 proof.txt 未生成 |
| 经权限流程在外层沙箱之外重跑，保留 Codex 自身工作区沙箱 | 通过，7,712 ms | [日志](first-success.log)、[结果](first-success.json) |
| 增补断线处理后的真实复跑 | 失败，Cargo 用时 10.87 秒 | [日志](completion-conflict.log)。文件已正确生成，但流式文本与历史文本分隔不同，重建完成申请时 action_id 的载荷校验拒绝 |
| 修复完成申请复用后的最终版本 | **通过，12,879 ms** | [日志](verified.log)、[结果](verified.json)；RUN COMPLETED、TASK SUCCEEDED、same_thread=true、proof_lines=1 |

各次运行横跨环境问题、缺陷发现与修复，属于开发验收过程；不是同版本独立性能采样，不报告统计成功率。
记录只含固定测试任务、时长、结果及线程/回合引用，不保存认证文件、密钥或完整外部私有历史。
临时工作目录、XDG 状态与 CODEX_HOME 随测试清理。

复跑（需要本机对应 Codex 配置和供应商凭据；该命令使用真实服务）：

```bash
TEAMAGENTS_LIVE_CODEX=1 \
TEAMAGENTS_LIVE_CODEX_CONFIG="$HOME/.codex/deepseek.config.toml" \
TEAMAGENTS_LIVE_CODEX_CONTEXT_WINDOW=1000000 \
cargo test --offline --locked --manifest-path engine/Cargo.toml --test live_codex -- --nocapture
```

最终结果文件的 `duration_ms` 包含真实回合及冷恢复，不是仅模型响应时间。
该验收不能证明所有外部接受/本地确认崩溃窗口均有 exactly-once，也不能证明整体编码能力已达到竞品水平。
