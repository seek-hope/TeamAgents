# 开发与维护

本项目保持 `core`、`engine`、`tui` 三个 Rust crate。产品约束见 [实施方案](../TeamAgents-Implementation-Plan.zh-CN.md)，
已确认决策见 [DECISIONS](DECISIONS.md)，真实服务验收边界见 [ACCEPTANCE](ACCEPTANCE.md)。

## 环境与统一入口

在仓库根目录工作。`rust-toolchain.toml` 固定 Rust 版本、Clippy 与 rustfmt；本机和两个 GitHub
工作流读取同一份版本配置。通过 rustup 安装 Rust 后，首次运行 Cargo 会准备所需工具链。
Linux 隔离检查还需要可工作的 bubblewrap；真终端检查需要 Python 3。

```bash
make check CARGO_FLAGS=--locked   # 首次允许下载依赖，保持锁文件不变
make check                        # 后续默认 --offline --locked
make fmt                          # 应用统一格式
make build                        # 构建 CLI 与 TUI，使用原有 target 路径
make pty                          # 独立配置/状态、无模型凭据的真终端检查
```

`make check` 依次检查格式、全部目标的 Clippy、三个 crate 的测试、Git 子模块配置、已跟踪缓存与安装脚本语法。
Clippy 警告视为错误。CI 使用相同的 Make 目标，仅允许联网下载锁定的依赖；发行工作流使用相同的 Rust 版本。
Cargo 报告 passed 可能包含缺少依赖时提前返回的测试，不能代替真实隔离或真实模型验收。

局部开发仍直接使用 Cargo，缩短反馈时间：

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test model_override
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e worker_environment
cargo test --offline --locked --manifest-path tui/Cargo.toml --test render_tests
```

## 改动应放在哪里

| 变更 | 所属位置与约束 | 优先回归 |
|---|---|---|
| 团队动作、任务状态、调度、权限 | `core/control.rs` 经 `Control::submit` 的单事务路径；`storage.rs` 持久化、`views.rs` 可见性 | `core/tests/engine.rs`、`engine/tests/scenarios.rs` |
| 成员执行、取消、恢复 | `engine/runtime.rs`；具体 Chat/Codex 协议留在各自适配器 | `recovery`、`chat_e2e`、`codex_contract` |
| 模型配置、会话启动 | `engine/config.rs`、`session.rs`、`sessions.rs` | `cli`、`session_boot`、`model_override` |
| 工具执行、隔离、批准 | `engine/tools.rs`、`gateway.rs`；已绑定 MCP 经 `bound.rs`/`mcp.rs` | `tools_sandbox`、`mcp_*`、批准回归 |
| TUI 操作、显示、鼠标 | `tui/app.rs` 状态与动作、`ui.rs` 渲染；布局和点击复用 `geometry` | `app_tests`、`render_tests`、`make pty` |
| 安装与发布 | Rust `init`、`install.sh`、`.github/workflows/release.yml` | `engine/tests/install.rs`、`cli`、发行制品冒烟 |

表中 Rust 路径均相对各 crate 的 `src/`。TUI 通过 worker 协议访问引擎，不直接读取数据库。
回调与队列类型在所属模块命名，例如 `TopologyPrepare`、`RunnerCache`、`PendingReplies`，避免跨文件复制复杂签名。
抽取模块应围绕独立职责和实际变更频率；不为消除单个 lint 创建只有一处使用的框架。

## 稳定的回归测试

优先在子进程上通过 `Command::env` 设置测试环境。必须改当前进程环境的 engine 集成测试，
使用 `engine/tests/support/env.rs` 的 `TestEnv`：

```rust
let mut env = support::isolated_state_home("descriptive-test-name");
env.set("TA_TEST_OPTION", "value");
let project = env.join("project");
// Create the runtime after env; close it and join workers before env is dropped.
```

该对象持有测试二进制内的互斥锁，设置独立的 XDG 配置/状态目录，退出时恢复环境变量并清理目录，
包括 panic 展开路径。必须保存返回值到局部变量；不要写 `let _ = ...` 或在辅助函数内提前丢弃。
额外环境变量用 `env.set`，不要另写 `std::env::set_var`；需要返回运行时的辅助函数应一并返回该对象。
engine 库单测修改进程环境时继续遵循现有 `crate::env_lock()` 约定。

假服务应先读请求再回响应，避免响应先于 pending request 注册。并发断言优先使用屏障或通道，
关闭并等待所有线程、子进程后再销毁环境；不要依赖测试名称顺序或本机用户配置。
`test_environment` 回归验证变量恢复、非 Unicode 值、重复设置、panic 清理以及锁中毒后的复用。

## 代码检查与评审

- 统一使用根目录的 `rustfmt.toml`；大批格式调整与行为变更分别提交，便于查看实际逻辑差异。
- 不在 crate 根关闭 lint。确有必要保留的现有参数较多的接口，用函数级 `#[expect(..., reason = "...")]`
  解释原因；失去触发条件的 expect 也会在严格检查中报错。
- 文件打开明确选择保留、追加或截断；锁文件保持原有 inode 和内容，不因清理 lint 改成截断。
- 新逻辑以正常、失败和恢复行为验证；纯搬移或格式化复用既有回归，不增加只镜像实现的测试。
- `review/tmp/` 是被忽略的探针与临时制品目录。有效结论放到有日期的 `review/*.md`，正式评测证据
  放在 `review/eval/runs/`；不再提交 Python 缓存、临时数据库、嵌套 Git 仓库或整份旧源码备份。

## 依赖、工具链与发行

三个 Cargo.lock 各自提交，常规检查和发行都加 `--locked`。新增依赖先检查标准库和已有依赖能否满足需求；
升级依赖时只更新相关锁文件，并重跑受影响的接口检查及 `make check`。
升级 Rust 只修改 `rust-toolchain.toml` 的版本，再运行 `make fmt`、`make check` 与 `make pty`；CI/发行自动读取新版本。

发布前同步三个 crate 的版本及其锁文件、更新发行说明、通过 CI，再推送 `vX.Y.Z` 标签。
发布后从公开地址校验 SHA-256，验证安装、`init` 保留配置和实际 TUI 启动。
真实模型评测需明确配置凭据与模型原生上下文，单独记录；维护性回归不能宣称扩大供应商兼容范围。
