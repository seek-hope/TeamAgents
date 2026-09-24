# 开发与维护

本项目保持 `core`、`engine`、`tui` 三个 Rust crate。产品约束见 [实施方案](archive/TeamAgents-Implementation-Plan.zh-CN.md)，
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

`make check` 依次检查格式、全部目标的 Clippy、三个 crate 的测试、Git 子模块配置与仓库卫生（拒绝已跟踪的编译缓存 / Python 缓存 / SQLite 临时文件，并检查 `install.sh` 语法）。
Clippy 警告视为错误。CI 使用相同的 Make 目标，仅允许联网下载锁定的依赖；发行工作流使用相同的 Rust 版本。
Cargo 报告 passed 可能包含缺少依赖时提前返回的测试，不能代替真实隔离或真实模型验收。

### 评测与探针入口

产品之外还有几个独立入口：故障/开销探针、评测组 A/B/C 的驱动，以及真终端驱动。它们都不进
`make check`，复跑时请选用**新的证据目录**，结果与限制写进有日期的报告：

```bash
# 故障探针：SQLite/制品原子边界、runner 与 daemon 崩溃恢复、存储失败停机、I/O 取消
cargo build --offline --locked --manifest-path engine/Cargo.toml --example probe
cargo build --offline --locked --manifest-path tui/Cargo.toml --example probe
engine/target/debug/examples/probe suite review/tmp/probe-new
python3 tui/scripts/pty_probe.py            # 真终端里驱动同一协议

# 评测组驱动（真实模型；A = 直驱参考，B = 持久化单实例，C = B + 协作面）
cargo build --offline --manifest-path engine/Cargo.toml --example eval_group_a
engine/target/debug/examples/eval_group_a --task "..." --workdir /tmp/t --trace /tmp/t-trace
cargo build --offline --manifest-path engine/Cargo.toml --example eval_group_b   # 需 engine/target/debug/teamagents
engine/target/debug/examples/eval_group_b --task "..." --workdir /tmp/t --trace /tmp/t-trace --full-auto
engine/target/debug/examples/eval_groups_abc --group A --task-file t.md --workdir /tmp/t --trace /tmp/t-trace --state /tmp/t/state

# 负载与验收探针
engine/target/debug/examples/load_probe DIR [--steps N] [--payload BYTES]
engine/target/debug/examples/accept_probe --evidence DIR --workspace DIR --lead KEY --worker KEY
```

故障注入与多实例的确定性覆盖用测试文件：`cargo test --manifest-path engine/Cargo.toml --test v2_driver`
（崩溃复用结果、磁盘满、取消、必需检查）、`--test jobs_runner`（runner/daemon 崩溃、重复 GO）、
`--test v2_supervisor`（多实例调度与回收）、`--test v2_daemon`（握手、水位续读）。

局部开发仍直接使用 Cargo，缩短反馈时间（下表即当前全部测试入口）：

```bash
# core：v2 控制面、内核、持久化
cargo test --offline --locked --manifest-path core/Cargo.toml --lib   # v2 控制面单测（A01–A36 的多数引用）
cargo test --offline --locked --manifest-path core/Cargo.toml --test v2_invariants
cargo test --offline --locked --manifest-path core/Cargo.toml --test kernel_properties
# engine：v2 相位机/多实例/daemon/job、假服务与 CLI
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_driver
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_supervisor
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_daemon
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_mcp
cargo test --offline --locked --manifest-path engine/Cargo.toml --test v2_spawn_failure
cargo test --offline --locked --manifest-path engine/Cargo.toml --test jobs_runner
cargo test --offline --locked --manifest-path engine/Cargo.toml --test providers_fake
cargo test --offline --locked --manifest-path engine/Cargo.toml --test providers_stall
cargo test --offline --locked --manifest-path engine/Cargo.toml --test eval_group_a
cargo test --offline --locked --manifest-path engine/Cargo.toml --test cli
cargo test --offline --locked --manifest-path engine/Cargo.toml --test install
# tui：会话界面状态与渲染
cargo test --offline --locked --manifest-path tui/Cargo.toml --test v2app_tests
```

## 形式化验证（TLA+ / Kani）

`verification/` 是与实现同源的形式化材料：`tla/V2*.tla` + `MC*.cfg` 是 v2 控制面、制品、等待、
任务、压缩、daemon 协议与必需检查轮次的 TLA+ 规格；`kani/` 是直接编译 `core/src/kernel/types.rs`
的证明 crate。结论、证据与**未证明清单**见 [验证报告](../verification/REPORT.md)，性质 ↔ 代码 ↔ 验收编号的映射见
[验证说明](../verification/README.md)。

```bash
make verify-tools       # 下载并校验固定版本 tla2tools.jar（TLC v1.7.1，SHA-256 固定）
make verify-model       # 控制面小配置穷举
make verify-model-all   # 七个模块的小配置穷举（秒级）
make verify-model-wide  # 控制面宽配置（数亿状态，耗时较长）
make verify-kani        # 分页算术的 Kani 证明（需 Kani 工具链）
cargo test --offline --locked --manifest-path core/Cargo.toml --test v2_invariants
```

这些目标不进 `make check`（需要 Java / Kani，首次还要下载 TLC）。TLC 的 `verification/tla/states/`
与 Kani 的 `target/` 都是可再生成的中间产物，已被 `.gitignore` 忽略，不入库。
模型只覆盖协议层性质：不是精化证明，活性依赖弱公平假设，穷举都有界；改动 v2 命令集、相位机或
分页/裁剪逻辑时，须同步规格与 `v2_invariants`，并重跑对应目标。

## 改动应放在哪里

| 变更 | 所属位置与约束 | 优先回归 |
|---|---|---|
| 团队动作、任务、授权与调度 | `core/src/v2/control.rs` 的 `Control::submit` 单事务路径；身份、操作号与权限版本一律由控制面生成，不取自模型或客户端字段 | `core` 库单测（`core/src/v2/control.rs`）、`core/tests/v2_invariants.rs`、`engine/tests/v2_supervisor.rs` |
| 持久化与事务边界 | `core/src/v2/store.rs`（每会话单库，WAL + 显式 `synchronous=FULL`）；进程内调用经 `engine/src/v2/storage.rs` 的有界单写线程串行化 | `core` 库单测、`engine/tests/v2_driver.rs`、`core/tests/v2_invariants.rs` |
| 回合相位机与工具执行 | `engine/src/v2/driver.rs`：模型/工具等待在事务外，状态迁移只经 `Control::submit`；文件/Shell/web 工具在 `engine/src/tools.rs`，Shell 命令由 `engine/src/jobs` 的 runner 进程执行；用户钩子在 `engine/src/hooks.rs`（`pre_tool` 拦截 + `notify` 事件） | `engine/tests/v2_driver.rs`、`engine/tests/jobs_runner.rs`、`engine/tests/providers_fake.rs`、`providers_stall.rs` |
| 多实例协作面 | `engine/src/v2/supervisor.rs`：每个 ACTIVE 实例一个相位机；`spawn`/`delegate`/`send`/`wait` 的授权与派发线性化点在 `core/src/v2/control.rs` | `engine/tests/v2_supervisor.rs`、`core/tests/v2_invariants.rs` |
| 会话 daemon 与客户端 | `engine/src/v2/daemon.rs`（每状态根一个 Unix socket JSON-lines 服务）、`engine/src/v2/exec.rs`（无头客户端）、`tui/src/daemon_client.rs`（断线按事件水位续读） | `engine/tests/v2_daemon.rs`、`engine/tests/cli.rs`、`make pty` |
| 信息权限与共享空间 | `core/src/v2/control.rs` 的可见性/投递判定与 `core/src/kernel/*` 的上下文视图；`audience` 可见不等于 `push` 注入 | `core` 库单测（可见性/投递/引用）、`core/tests/v2_invariants.rs` |
| MCP、Skills 与工具绑定 | `engine/src/bound.rs`（绑定即授权）、`engine/src/mcp.rs`（stdio + streamable HTTP） | `engine/tests/v2_mcp.rs`、`engine/tests/v2_spawn_failure.rs` |
| 工作区策略（§12.3） | `engine/src/workspace.rs`：共享/隔离/git worktree；`spawn` 的 `workspace` 参数解析在 `driver::prepare_spawn_workspace`，策略记录写在 `<instances_dir>/<id>/workspace.json`，回收在 supervisor 的终止路径 | `engine/src/workspace.rs` 单测、`engine/tests/v2_driver.rs::spawn_resolves_the_requested_workspace_policy`、`engine/tests/v2_supervisor.rs::terminating_an_instance_retires_its_workspace` |
| 供应商适配 | `engine/src/providers/*`：一次传输尝试、只做失败分类，重试归运行时；配置与目录在 `engine/src/config.rs`（用户目录的解析、`[hooks]`/`[retention]` 校验） | `engine/tests/providers_fake.rs`、`engine/tests/providers_stall.rs`、`engine/src/config.rs` 单测 |
| 模型调用与内核 | `core/src/kernel/*`（无 I/O 的请求/响应/观察转换）、`engine/src/reference.rs`（评测组 A 直驱参考循环） | `core/tests/kernel_properties.rs`、`engine/tests/eval_group_a.rs` |
| 会话界面与真终端 | `tui/src/v2app.rs`（状态与按键）、`tui/src/v2ui.rs`（渲染，`geometry()` 同时供鼠标命中）、`tui/src/wrap.rs` | `tui/tests/v2app_tests.rs`、`make pty` |
| 安装、自检与发布 | `install.sh`、`engine/src/cli.rs` 的 `init`/`doctor`、`.github/workflows/release.yml` | `engine/tests/install.rs`、`engine/tests/cli.rs`、发行制品冒烟 |

表中 Rust 路径均相对各 crate 的 `src/`。TUI 只经 daemon socket（`tui/src/daemon_client.rs`）访问引擎，
不直接读取数据库，也不在本进程执行任何东西。回调与队列类型在所属模块命名，避免跨文件复制复杂签名。
抽取模块应围绕独立职责和实际变更频率；不为消除单个 lint 创建只有一处使用的框架。

## 稳定的回归测试

集成测试优先在子进程上用 `Command::env` 设置隔离的 `XDG_CONFIG_HOME`/`XDG_STATE_HOME`
（见 `engine/tests/cli.rs`）。确需改当前进程环境时只改本测试用到的变量，并在同一测试内恢复；
引擎库单测沿用已有的 `crate::env_lock()` 约定。生产会话的测试项目必须与配置/状态目录分开，
例如同一临时根下使用 `project/`、`config/`、`state/` 三个子目录；
不要把包含 XDG 状态的整个临时根或 `/tmp` 作为共享工作根。

假服务应先读请求再回响应，避免响应先于 pending request 注册。并发断言优先使用屏障或通道，
关闭并等待所有线程、子进程后再销毁环境；不要依赖测试名称顺序或本机用户配置。

覆盖面按层划分：`v2_driver` 负责崩溃后复用已知结果而非重做副作用、磁盘满停机与恢复、取消与必需检查；
`jobs_runner` 负责 runner/daemon 分别崩溃、重复 GO 与服务跨退出存活；`v2_supervisor` 负责多实例调度；
`v2_daemon` 负责握手、水位续读与第二 daemon 拒绝；`v2_mcp` 负责绑定、批准与取消；
`providers_fake`/`providers_stall` 负责半条流、失联与截断重试；`v2_invariants` 把形式化规格的
不变量映射回当前命令集。以上均使用本地夹具与假服务，证据和边界见
[验收对照表](ACCEPTANCE.md)，不计作真实模型或真实供应商验收。

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

## 固定任务与评分

任务集与运行器在 `review/eval/r2-p6/`：每个任务 `tasks/<id>/` 提供 `prompt.md`、`checks.txt` 与可选
`fixture/`；`run.py` 为每个 trial 建全新的工作目录与状态目录，trial 结束后在**同一目录**按 `checks.txt`
逐条判分，结果写 `runs/<日期>/results.jsonl`。分组定义、冻结参数与限制见
[评测说明](../review/eval/README.md)。

- 判分只在 trial 工作目录内执行，不读取当前工作树；任务输入与判分脚本在跑前冻结（`manifest*.json` 记哈希）。
- 大型任务的构建输出可能占数 GB：先 `df -h /tmp .` 核对文件系统，把输出目录放在被忽略的
  `review/tmp/` 或独立磁盘，并让 `TMPDIR` 指向同一磁盘。
- 已有证据不自动清理；运行期间不修改输入、判分或候选，不因换存储位置把历史失败改记成功。
- 与竞品 CLI 对照时：若它能读工作区外的文件，仅把测试放远不足够——要在独立文件系统视图里只挂载公开输入，
  用无模型探针确认隐藏材料不可读，同时确认工作区可写、对方沙箱仍生效。发现已污染就终止、保留轨迹、
  换全新输入重跑，不续接被污染的上下文。
- 更早实现的固定任务运行器与评分器（含隐藏测试注入）在 `review/eval/archive/`，它们的原始结果仍在
  `review/eval/runs/`。

## 真实模型验证

v2 的真实模型证据来自三个显式入口，都不进 `make check`（需要凭据与原生上下文配置）：

```bash
# 评测组 A/B/C 固定任务对照（预登记口径与冻结参数见 review/eval/r2-p6/design.md）
python3 review/eval/r2-p6/run.py --phase pilot  --out review/eval/r2-p6/runs/<新日期>
python3 review/eval/r2-p6/run.py --phase formal --out review/eval/r2-p6/runs/<新日期>
# 单次无头回合：自动拉起 daemon，只报结果（--json 给机器可读摘要）
engine/target/debug/teamagents exec --json --timeout 180 "1+1=?"
# 直驱参考循环（评测组 A 入口，同一 kernel/工具/配置）
engine/target/debug/examples/eval_group_a --task "..." --workdir /tmp/t --trace /tmp/t-trace
```

- 必须使用模型的原生上下文长度并在报告里记录数值与来源（D-36）；DeepSeek Flash 按用户确认的 1M。
- 每个 trial 使用全新工作目录与状态目录；结果写 `runs/<日期>/results.jsonl`，逐 trial 的会话库与
  产物留在 `runs/<日期>/{state,work}/`。trial 的编译缓存与 SQLite 临时文件**不入库**：
  `.gitignore` 忽略 `target/` 与 `*.sqlite-wal|shm`，`make hygiene` 会拒绝误提交。
- 结论只按预登记口径给出；样本不足、区间含 0 或方差过大时写「未证实」，不写等效也不写收益。
- 更早的评测入口（真实模型矩阵、Codex 恢复检查、历史浏览协议、旧的固定任务运行器）与其证据在
  `review/archive/` 与 `review/eval/archive/`，不再是可复跑的当前入口。

真实模型运行的操作备忘（中断后任务可能落 `BLOCKED`、RT-06 批准随回合终态过期、重任务要尽早落盘）
见仓库根目录 [AGENTS.md](../AGENTS.md)。