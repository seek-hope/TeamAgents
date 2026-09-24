# TeamAgents 仓库约定（对本仓库工作的所有 Agent 生效）

## 基准与偏离

- 设计与验收基线见 `docs/DESIGN.zh-CN.md`（含 Q1–Q19 需求、架构与协议约束、A01–A36 验收矩阵与完成定义）；
  逐项证据与已知缺口见 `docs/ACCEPTANCE.md`，当前生效的决策与仍适用的早期约定见 `docs/DECISIONS.md`。
- **任何与已确认设计不同的实现（更简单或更好的方案）必须先告知用户并得到确认，才可写进代码。**
  已确认的偏离记录在 `docs/DECISIONS.md`；未确认的只讨论，不落码。
- 本仓库是独立的 Rust 项目，实现代码统一使用 Rust（core / engine / tui 三个 crate）；
  `tui/scripts/*.py` 及部分 Rust 测试中的 Python 假服务只用于测试。
- 更早实现与其迁移期的材料（旧实施计划、旧指南、阶段记录、决策史、审查批次）在 `docs/archive/` 与
  `review/archive/`，只作追溯，不作为当前口径。

## 快速命令

统一开发入口与维护约定见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。
工具链固定在 `rust-toolchain.toml`；提交前运行 `make check`（默认离线），格式修复用 `make fmt`，
隔离配置的真终端检查用 `make pty`。首次下载依赖可用 `make check CARGO_FLAGS=--locked`。

```bash
cargo test --offline --manifest-path core/Cargo.toml    # 权威核心（v2 控制面 + kernel + 规格对应）
cargo test --offline --manifest-path engine/Cargo.toml  # 引擎（v2 相位机/多实例/daemon/job/MCP）
cargo test --offline --manifest-path tui/Cargo.toml     # TUI 逻辑 + TestBackend 帧
engine/target/debug/teamagents {init,doctor,daemon,exec,version}  # CLI 入口
engine/target/debug/teamagents exec --json --timeout 180 "…"      # 无头回合（必要时自动拉起 daemon）
make pty                                 # 真终端冒烟（tui/scripts/pty_v2_smoke.py）
make verify-model-all                    # TLA+ 小配置穷举（七个模块；见 verification/README.md）
make verify-kani                         # Kani 证明（分页算术）
python3 review/eval/r2-p6/run.py --phase pilot --out <新的日期目录>   # 真实模型 A/B/C 对照（需凭据）
```

- 基线（2026-09-25）：`make check` 全绿——core 91 / engine 136 / tui 29；真实评测的原始 JSONL 在
  `review/eval/runs/`，逐项证据在 `docs/ACCEPTANCE.md`。

- 当前基线与跳过项统一见 `docs/ACCEPTANCE.md`；Cargo 的通过数不等于真实服务验收通过数。
  决策记录 `docs/DECISIONS.md`。
- 模型评测一律使用该模型的原生上下文长度，并记录数值及来源；不得自行缩小窗口进行真实模型测试。
  DeepSeek Flash 按用户确认的 1M 配置。不合理的非原生窗口测试应作废并删除，不得改名为压力实验保留。

## 架构速览（改代码前先读这几行）

以下描述当前代码。

- 唯一团队事务入口：`core/src/v2/control.rs::Control::submit`（ingest→validate→reduce→persist，
  单会话单 SQLite 事务；身份、操作号与权限版本由控制面生成，不取自模型或客户端字段）
- 权威状态：`core/src/v2/store.rs`（WAL + 显式 `synchronous=FULL`，格式印记拒绝外来/错版库）；
  进程内调用经 `engine/src/v2/storage.rs` 的有界单写线程串行化
- 执行：`engine/src/v2/driver.rs`（相位机，模型/工具等待在事务外）与 `engine/src/v2/supervisor.rs`
  （每状态根一个协调者，驱动全部 ACTIVE 实例）；Shell 命令由 `engine/src/jobs` 的 runner 进程执行
- 工具与绑定：`engine/src/tools.rs`（文件/Shell/web）、`engine/src/bound.rs`（绑定即授权）、
  `engine/src/mcp.rs`（stdio + streamable HTTP）；用户钩子在 `engine/src/hooks.rs`（`[hooks]`
  `pre_tool` 拦截 + `notify` 事件）
- 信息权限：`core/src/v2/control.rs` 的可见性/投递判定与 `core/src/kernel/*` 的上下文视图
  （`audience` 可见 ≠ `push` 注入；观察者按 scope 裁剪载荷）
- 产品层：`engine/src/v2/daemon.rs`（每状态根一个 Unix socket JSON-lines 服务）、`engine/src/v2/exec.rs`
  （无头客户端）、`engine/src/cli.rs`、`tui/src/daemon_client.rs`；渲染与鼠标命中共用
  `tui/src/v2ui.rs::geometry`
- 工作区策略：`engine/src/workspace.rs`（共享/隔离/git worktree，§12.3）由 `spawn` 工具的 `workspace`
  参数选择；`driver::prepare_spawn_workspace` 在启动子实例前解析策略并写 `<instances_dir>/<id>/workspace.json`，
  supervisor 在实例 `TERMINATED` 时据此回收（有未提交/未合并结果时拒绝删除并报告）。

## 代码审查与证据（review/*）

- 现行证据：`docs/ACCEPTANCE.md`（A01–A36 逐项）、`verification/REPORT.md`（形式化验证结论、证据与
  未证明清单）、`review/README.md` 索引的当前材料。
- 日期化的历史批次（审查发现、修复台账、迁移阶段、更早实现的评测）在 `review/archive/`：可追溯当时的
  结论，但不描述当前代码。
- 只读审查不得修改被审文件；结论必须带可复跑的命令或探针（探针放 /tmp 或 `review/tmp/`），
  报"证伪"前先排除探针自身误差。

## 团队运行操作备忘（实测）

- **任务可能停在 `BLOCKED`**：必需检查耗尽、成员异常或取消时任务会落 `BLOCKED`，它会阻止目标完成。
  用户可在 TUI 任务面板选中后按 `c` 取消（无相关活动回合时直接落 `CANCELLED`）；重派任务请用新任务。
- **避免打断成员回合**：把任务拆小、验收写清。打断后按持久化位置分类恢复——已知结果复用、在途丢失
  记 `OUTCOME_UNKNOWN`，绝不猜测重放副作用。
- **批准**：批准绑定具体操作与参数散列，`once` 用后即失效；操作结清或实例终止时其 PENDING 批准自动
  过期，个别残留可在批准面板按 `d` 拒绝。
- **传输故障**：响应头之后的读超时/流截断在未送出可见文本时按 `max_retries` 在回合内重试；已送出文本
  或协议错误立即失败，原因写进回执（`transient retries exhausted` / `permanent model error` /
  `context overflow`）。
- **目标时限与预算**：目标可带 `deadline` 与用量上限；过期后不再开始新请求，用量结算如实记账。
- **重任务要"尽早落盘"**：回合受预算与时长约束，读很多再写报告的任务应在中途先落产物，避免回合结束时
  只留下未提交的结论。

## 代码风格

- Lazy-first：标准库 > 已有依赖 > 新依赖；抽象与脚手架以当前需求为限。
- 每个非平凡逻辑留一个可运行的检查（acceptance 测试或 `__main__` 自检）；删除代码优于新增代码。
- CI 与本机共用 Make 目标；Clippy 对全部目标按 `-D warnings` 检查，不在 crate 根统一关闭告警。
- engine 集成测试优先用子进程 `Command::env` 隔离 `XDG_CONFIG_HOME`/`XDG_STATE_HOME`（见
  `engine/tests/cli.rs`）；确需改进程环境时只改本测试用到的变量并在同一测试内恢复。
- 给「已知天花板」的简化留 `ponytail:` 注释（写明升级路径）。
- 文档、提交信息与面向用户的输出用中文；代码标识与注释用英文。
- 密钥只从环境变量/本机凭据读取，禁止写入仓库、提示词、事件或日志。
- Skills 约定：唯一用户级注册根为 `~/.agents/skills`，配套范围为
  `K-Dense-AI/scientific-agent-skills` 科学技能集合、`browser-use`、`find-skills`。
  配套技能安装到该目录，通过 `skill search/read` 按需检索和读取。来源与范围见 `docs/DECISIONS.md` 中的 D-34。
