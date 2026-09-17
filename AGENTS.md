# TeamAgents 仓库约定（对本仓库工作的所有 Agent 生效）

## 基准与偏离

- `TeamAgents-Implementation-Plan.zh-CN.md` 是产品与实现的基准（P0–P7、T1–T24、DP-1..12）。
- **任何与方案不同的实现（更简单或更好的方案）必须先告知用户并得到确认，才可写进代码。**
  已确认的偏离记录在 `docs/DECISIONS.md`；未确认的只讨论，不落码。
- 本仓库是独立的 Rust 项目，实现代码统一使用 Rust（core / engine / tui 三个 crate）；
  `tui/scripts/*.py` 及部分 Rust 测试中的 Python 假服务只用于测试。

## 快速命令

```bash
cargo test --offline --manifest-path core/Cargo.toml    # 权威核心
cargo test --offline --manifest-path engine/Cargo.toml  # 引擎
cargo test --offline --manifest-path tui/Cargo.toml     # TUI 逻辑 + TestBackend 帧
engine/target/debug/teamagents {doctor,validate,sessions,version,--plain}   # 入口
engine/target/debug/teamagents sessions prune --days 30 [--history-days 30] [--dry-run]
python3 tui/scripts/pty_smoke.py         # 真终端冒烟
python3 tui/scripts/pty_click_check.py   # 真终端点击命中检查
review/eval/run.sh [--only ID] [--timeout SEC]   # 固定任务集的真实模型评测（需凭据）
```

- 基线（2026-09-15）：core 54 / engine 187 / tui 91 全绿；真实评测证据与逐批记录见
  `review/stability-2026-09-15.md`，跑过的原始 JSONL 在 `review/eval/runs/`。

- 当前基线与跳过项统一见 `docs/ACCEPTANCE.md`；Cargo 的通过数不等于真实服务验收通过数。
  决策记录 `docs/DECISIONS.md`。
- 模型评测一律使用该模型的原生上下文长度，并记录数值及来源；不得自行缩小窗口进行真实模型测试。
  DeepSeek Flash 按用户确认的 1M 配置。不合理的非原生窗口测试应作废并删除，不得改名为压力实验保留。

## 架构速览（改代码前先读这 6 行）

- 唯一团队事务入口：`core/src/control.rs::Control::submit`（ingest→validate→reduce→schedule→persist，
  单个 SQLite 事务；错误向上传播）
- 权威状态：`core/src/storage.rs`（SQLite，WAL，动作去重回执、事件序列、投递批次账本、`expire_approval`）
- 执行：`engine/src/runtime.rs`（线程化回合循环；`QUEUED` TurnRun = 持久化执行意图；超时会中断成员）
- 团队动作与原生执行工具入口：`engine/src/gateway.rs::ToolGateway`（批准/全自动）；
  已绑定 MCP 由 `ChatRunner` 经 `BoundTools::call` 直接调用，受绑定集合与 TurnControl 约束。
- 信息权限：`core/src/views.rs`（`audience` 可见 ≠ `push` 注入；观察者按 scope 裁剪载荷）
- 产品层：`engine/src/{session,worker,cli}.rs`（会话服务、`serve` 协议、CLI）；TUI 的 `ui::geometry`
  是渲染与鼠标命中的唯一几何来源

## 代码审查与证据（review/*）

- 现行审查报告：`review/findings-deep-review-2026-09-14.md`（发现+证据）、
  `review/fix-notes-deep-review-2026-09-14.md`（修复台账+测试名）；更早的 `review/*` 是历史资料。
- 后续更新修复与 `/model` 扩展：`review/fix-notes-rust-updates-2026-09-14.md`。
- CI 转绿与首个发行版（含批准竞态两处真缺陷）：`review/fix-notes-ci-release-2026-09-15.md`。
- 只读审查不得修改被审文件；结论必须带可复跑的命令或探针（探针放 /tmp 或 `review/tmp/`），
  报"证伪"前先排除探针自身误差。

## 团队运行操作备忘（实测）

- **中断后检查任务状态**：未完成任务可能落 `BLOCKED`，阻止目标完成；`complete_task`
  只接受 PENDING/RUNNING 且只允许承接者提交。Leader 可用 `cancel_task` 结清 BLOCKED 任务；
  用户也可在 TUI 任务面板选中按 `c`（无相关活动回合时直接落 CANCELLED）。
  **避免中断成员回合**（步数/时限留足，或拆小任务）；重派任务时用新任务。
- **RT-06**：回合进入终态（含被取消）时其 PENDING 批准自动置 EXPIRED（`core/src/control.rs` 的
  finalize/reconcile 路径），`signal_done` 不会被残留批准卡住；个别残留可在批准面板 `d` 拒绝。
- 派发"读很多、写报告"的重任务时写明**尽早落盘**要求：模型步数上限
  `limits.max_model_steps_per_turn` 会在半途结束回合（产生 `limit_reached` 事件）。
- **读超时不再直接判回合失败**：响应头之后的传输故障（读超时/流截断）在未送出任何
  可见文本时按 `max_retries` 在回合内重试；已送出文本或协议错误仍立即失败。
  看到 `ChatError: model stream: ...` 说明重试已耗尽或响应已有可见输出。

## 代码风格

- Lazy-first：标准库 > 已有依赖 > 新依赖；抽象与脚手架以当前需求为限。
- 每个非平凡逻辑留一个可运行的检查（acceptance 测试或 `__main__` 自检）；删除代码优于新增代码。
- 给「已知天花板」的简化留 `ponytail:` 注释（写明升级路径）。
- 文档、提交信息与面向用户的输出用中文；代码标识与注释用英文。
- 密钥只从环境变量/本机凭据读取，禁止写入仓库、TeamSpec、提示词或事件。
- Skills 约定：唯一用户级注册根为 `~/.agents/skills`，配套范围为
  `K-Dense-AI/scientific-agent-skills` 科学技能集合、`browser-use`、`find-skills`。
  配套技能安装到该目录，通过 `skill search/read` 按需检索和读取。来源与范围见 `docs/DECISIONS.md` D-34。
