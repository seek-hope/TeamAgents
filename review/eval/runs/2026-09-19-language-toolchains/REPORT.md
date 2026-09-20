# 语言工具链与交付：本地证据（2026-09-19）

问题、实现和边界见[修复记录](../../../language-toolchains-2026-09-19.md)。本目录不包含真实模型响应或用户数据。

| 阶段 | 日志 | 结果 |
|---|---|---|
| 原版 Shell 探针 | `baseline.log` | Python venv 成功；Node 包含四个 npm 运行时文件 |
| 首次回归 | `red.log` | 1 通过、4 失败；Node 一项含测试自身的 npm JSON 格式错误 |
| 修正探针 | `red-corrected.log` | Python 通过；Shell/MCP HOME、旧默认值和 npm 包清单检查失败 |
| 初次修复定向 | `green.log` | 新增 5 项及既有工具/MCP 23 项通过 |
| 首次完整检查 | `make-check.log` | 旧 MCP 测试仍要求 HOME 等于项目根，断言失败 |
| 更新行为断言 | `make-check-final.log` | 完整检查通过，81.107 秒 |
| 实际 tarball 检查 | `delivery-green.log` | 5/5，4.771 秒；列出 tarball 八个文件，无缓存或日志 |
| 最终完整检查 | `make-check-verified.log` | 退出 0，77.342 秒；core 90、engine 334、TUI 104；另 3 ignored |

阶段命令和退出码在对应 `*-result.json`。`test-counts.json` 从最终完整日志逐测试二进制统计。
`product-and-tests.patch` 只含本批两个生产文件及新增测试文件，相对 `review/tmp/language-toolchains-20260919/before/`
的原始脏工作树；不是相对 Git HEAD 的全部改动。原始日志保留，测试错误不计作产品缺陷。

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test toolchain_projects -- --nocapture
make check
# 在本目录核对归档：
sha256sum -c SHA256SUMS --quiet
```

Python/Node 使用系统工具和本地依赖，真实 bubblewrap 隔离，未联网下载、未调用模型、未新增 PTY 或竞品成绩。
Python venv/wheel 原版已能完成，本批补充验收；产品修复是默认 HOME 位置及旧 Shell 状态兼容。
