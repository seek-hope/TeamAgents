# Chat 冷恢复与归档：离线证据（2026-09-19）

详细问题、实现与测试见[修复记录](../../../chat-cold-recovery-2026-09-19.md)。
此目录没有真实模型响应或用户数据，全部 HTTP 请求来自本地协议夹具。

| 阶段 | 日志 | 结果 |
|---|---|---|
| 首次探针 | `initial.log` | 1 通过、2 失败；误判工具结果形状，不计产品缺陷 |
| 修正后的原版 SIGKILL 基线 | `baseline.log` | 3/3：结果与补充、待批准、未知外部副作用 |
| F-1 原版归档失败 | `finalization-red.log` | 失败：归档失败后再次请求模型 |
| Runtime 修复 | `finalization-green.log` | 6/6，含原 T8/T22 |
| 扩充边界 | `expanded-green.log`、`expanded-green-2.log`、`paused-diagnostic.log` | 一次测试类型编译错误；一次误用 settle；诊断说明暂停通知应保留为 QUEUED |
| 修正边界断言 | `expanded-green-3.log` | 8/8 |
| F-2 Runtime 修复后冷恢复 | `failed-cold-red.log` | 失败：模型错误未持久化，归档失败再 SIGKILL 后再次请求模型 |
| 最终定向 | `recovery-green.log` | 9/9，新增 7 项 |
| 完整工程检查 | `make-check.log` | 退出 0，77.852 秒；core 90、engine 329、TUI 104，engine 另 3 ignored |

各阶段命令与退出码保存在对应 `*-result.json`。初始探针/编译错误原样保留，与两条产品行为失败分开。
`test-counts.json` 从完整日志逐测试二进制统计。未启动 `live_codex` 真实开关，
`live_models` 的真实入口仍 ignored；无 PTY 和真实模型新增成绩。

复跑：

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery -- --nocapture
make check
```

在本目录校验归档：

```bash
sha256sum -c SHA256SUMS --quiet
```

`product-and-tests.patch` 仅包含 `engine/src/runtime.rs`、`engine/src/chat.rs`、
`engine/tests/recovery.rs` 本批增量，基于 `review/tmp/chat-cold-recovery-20260919/before/`
的原始脏工作树，不是相对 Git HEAD 的全量补丁。独立副本重放后逐文件与最终源码哈希一致。
manifest 记录增量来源、文档前后哈希、依赖及工具链；没有提交、重置或覆盖旧评测。
