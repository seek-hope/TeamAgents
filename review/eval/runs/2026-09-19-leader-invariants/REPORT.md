# Leader 校验与恢复保护：离线证据（2026-09-19）

本批是确定性回归与工程检查，没有调用真实模型服务。
问题、实现和 8 项新增测试见[修复记录](../../../leader-invariants-2026-09-19.md)。

## 结果

| 阶段 | 证据 | 结果 |
|---|---|---|
| 原版本导入/保存/持久读取 | `red-cli.log`、`red-core-validation.log` | 重复 Leader 被接受；共 3 项新测试行为失败 |
| 原版本动态变更 | `red3-core-patches.log` | 新增/提升第二 Leader、切换 Codex Leader 均返回 APPLIED；3 项失败 |
| 原版本会话恢复 | `red2-session.log` | 非法重复 Leader 可进入恢复；1 项失败 |
| 仅修核心校验后的恢复 | `partial-session.log` | 无初始配置的拒绝通过，但显式初始配置仍绕过旧恢复分支；1 项失败 |
| 最终定向回归 | `green-core.log`、`green-engine-entrypoints.log` | core 90 通过；CLI 5、session_boot 3 通过 |
| 完整工程检查 | `make-check.log`、`make-check-result.json` | 退出 0；89.558 秒；core 90、engine 322、TUI 104；engine 另 3 ignored |

`partial-core.log` 为加入最后一个元数据测试前的核心检查，89 项通过。
`red-core-patches.log`、`red-session.log`、`red2-core-patches.log` 是测试代码的编译失败，
已保留以说明过程，不用它们证明产品行为。最终新增 8 项不等于 8 项都预先取得失败；
元数据存在性测试随修复增加，其余 7 项有行为失败证据。

## 复跑

在当前源码树的仓库根目录：

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml
cargo test --offline --locked --manifest-path engine/Cargo.toml --test cli --test session_boot
make check
```

在本归档目录检查完整性：

```bash
sha256sum -c SHA256SUMS --quiet
```

`test-counts.json` 从完整日志逐测试二进制汇总；Cargo passed 不等于真实服务验收。
既有 `live_codex` 未启用，`live_models` 的真实入口保持 ignored；另外两项 ignored 属于评分器。
没有重复 PTY，也没有新增真实模型、真实组队成功率或竞品比较。

## 来源

`manifest.json` 给出 Git HEAD、各阶段命令/退出码、依赖与工具链哈希、7 个生产/测试文件
及文档的前后哈希。工作树本来已有多批改动，没有提交、重置或改写旧评测记录。

`product-and-tests.patch` 以 `review/tmp/leader-invariants-20260919/before/` 为基线，
仅包含本批增量；该基线不是干净 Git HEAD，不能把补丁当作整个工程的重建包。
补丁在独立的基线副本上应用后，逐文件哈希与被检查的最终版本一致。
SHA 清单覆盖归档中的原始日志、阶段结果、报告、manifest 与补丁，不含清单自身。
