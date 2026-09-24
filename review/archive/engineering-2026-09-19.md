# 工程化改进记录（2026-09-19）

承接 `440e610` 的文档、死代码与临时文件清理。本轮落实用户要求的可维护性改进，保持 Rust 三 crate、
现有产品行为、权限边界和会话格式。

## 交付

- `rust-toolchain.toml` 固定 Rust 1.95.0、rustfmt、Clippy；CI 与发行工作流从该文件读取版本。
  `rustfmt.toml` 统一格式；纯格式调整单独提交为 `3128c25`，方便审查后续逻辑差异。
- `Makefile` 提供 `check`、`fmt`、`lint`、`test`、`build`、`pty`、`hygiene`。
  本机默认 `--offline --locked`，CI 使用相同目标并保留 `--locked`；Clippy 覆盖所有目标，警告视为错误。
- 将重复的复杂回调/队列签名命名为所属模块的类型；清理无效循环、重复表达式、无用克隆与冗余转换。
  锁文件显式 `.truncate(false)`，保留原语义。5 处既有多参数接口采用有原因的函数级 `expect`，
  不在 crate 根屏蔽告警；已满足条件时的多余 expect 同样由严格检查发现。
- 统一 engine 集成测试的 `TestEnv`：锁覆盖完整测试生命周期，配置与状态目录独立，额外变量统一设置，
  退出恢复原环境（包括未设置、非 Unicode 值）并清理目录。移除各测试文件重复实现的锁，
  为原来未持锁的场景/拓扑/Codex 测试补齐生命周期约束。
- 新增 `test_environment` 回归，验证重复设置后仍恢复最初值、panic 展开后的目录清理、锁中毒后下一测试可继续。
- `make pty` 使用独立 XDG 目录和无凭据的本地占位配置；鼠标检查的 TeamSpec 改用临时目录，避免覆写仓内 fixture。
- `.gitignore` 防止重新收进 `review/tmp` 临时产物；卫生检查拒绝已跟踪 Python 缓存并检查无配置 gitlink。
- [开发与维护](../docs/DEVELOPMENT.md) 记录模块职责、检查入口、测试隔离、lint 例外、依赖/工具链升级和发布流程；
  README、AGENTS、验收基线同步更新。

## 本地验证

```bash
make fmt-check
make lint
make test
make hygiene
make pty
```

- 格式检查和三个 crate 全部目标的 Clippy（`-D warnings`）通过。
- 完整回归：core **63**、engine **223**、TUI **91** 项通过；engine 另有 1 项显式 ignored。
- 真终端两项通过：启动、粘贴、消息回显、面板切换、终端恢复/退出；普通与滚动后行点击、页签点击。
- 工作流 YAML 与其中的 Bash 语法检查通过，工具链提取命令实际输出 `version=1.95.0`。
- 当前 README/AGENTS/docs 中本地 Markdown 文件链接检查通过（不包含外部 URL 与锚点）。

本次没有调用真实模型。CI 的 bubblewrap 与真实服务跳过边界仍见 [ACCEPTANCE](../docs/ACCEPTANCE.md)。
现有 engine 库单测仍使用 `crate::env_lock()`；本轮统一恢复/清理对象覆盖的是集成测试，未声称所有历史测试设施均已重写。
