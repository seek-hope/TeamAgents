# 大窗口上下文预算真实对照记录（2026-09-19）

本批验证 Chat L1 旧工具结果遮蔽预算从固定 16,000 字节调整为
`clamp(context_window / 4, 16,000, 256,000)` 字节。未配置窗口仍使用 16,000；DeepSeek
Flash 的原生 1,000,000 窗口对应 250,000 字节。生产改动只在 `engine/src/chat.rs`。

本地实际请求回归先证明了固定预算在 1M 配置下会过早遮蔽约 20KB 的已读源码；主成员和私有子代理
的请求回归在调整前失败、调整后通过。UTF-8 字节边界、256KB 上限、私有历史不变、压缩阈值使用
同一发送视图也有回归覆盖。

真实对照各启动一个新的独立会话，使用 DeepSeek Flash 原生 1M、high、请求超时 120 秒、重试 5 次、
任务时限 1200 秒；任务、评分器和隐藏检查保持原样。两次运行都没有形成可评分的 `result`：

| 版本 | 会话 | 有效 JSONL | 工具调用 | 结果 |
|---|---|---:|---:|---|
| 固定 16KB 对照 | `proj_0b2a549bf184` | 401 | 391 | 因共享 `/tmp` 配额耗尽后的环境不稳定而停止；无 `result` |
| 大窗口调整 | `proj_f1723e4d00c0` | 78 | 75 | `cargo test` 链接时收到 `Disk quota exceeded (os error 122)`；无 `result` |

两版都没有进入隐藏评分。直接配额错误仅见于调整版；对照版在共享环境故障后被人工停止。
`/tmp` 总容量为 16GB，故障后的检查曾显示约 13GB 已用，不是故障瞬间的配额测量。随后只删除了这两
份工作目录中被评分器忽略的临时 `target/` 构建目录，源码、日志、JSONL 和错误证据均保留。
这批结果不支持调整版优于或劣于对照版的结论，也不计入真实模型完成率。

本地验证：

- `cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e`：48 passed。
- `cargo test --offline --locked --manifest-path engine/Cargo.toml --lib chat::tests`：23 passed。
- `make check`：core 84 passed；engine 307 passed、2 ignored；tui 104 passed。

原始日志、配置哈希、运行器哈希、上下文预算实现/测试补丁与指标在本目录保存；
未归档模型产出的最终允许源码补丁。模型配置文件未归档，凭据未写入
仓库；JSONL 在归档前检查了当前环境中的 API 密钥/令牌值。
