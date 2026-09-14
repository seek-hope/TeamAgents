# TeamAgents 仓库约定（对本仓库工作的所有 Agent 生效）

## 基准与偏离

- `TeamAgents-Implementation-Plan.zh-CN.md` 是产品与实现的基准（P0–P7、T1–T24、DP-1..12）。
- **任何与方案不同的实现（更简单或更好的方案）必须先告知用户并得到确认，才可写进代码。**
  已确认的偏离记录在 `docs/DECISIONS.md`；未确认的只讨论，不落码。
- 本仓库现在**只有 Rust 实现**：Python 原版已在迁移完成后移除（历史保留在 git；`review/`
  与 `docs/RECONSTRUCT.md` 里出现的 `src/teamagents/...`、`tests/test_*.py` 均为历史引用）。
- 不要再引入 Node/TypeScript（D-17）或 Python 实现代码；`tui/scripts/*.py` 只是真终端测试工具。

## 快速命令

```bash
cd core   && cargo test --offline    # 权威核心（models/storage/control/views/server）
cd engine && cargo test --offline    # 引擎（runtime/chat/gateway/codex/tools/CLI/worker）
cd tui    && cargo test --offline    # ratatui TUI（逻辑 + TestBackend 帧）
engine/target/debug/teamagents {doctor,validate,sessions,version,--plain}   # 入口
python3 tui/scripts/pty_smoke.py         # 真终端冒烟
python3 tui/scripts/pty_click_check.py   # 真终端点击命中检查
```

- 当前基线：core 43 / engine 116 / tui 70 项测试全绿。验收清单 `docs/ACCEPTANCE.md`，
  迁移台账与未移植项 `docs/RECONSTRUCT.md`，决策记录 `docs/DECISIONS.md`。

## 架构速览（改代码前先读这 6 行）

- 唯一团队事务入口：`core/src/control.rs::Control::submit`（ingest→validate→reduce→schedule→persist，
  单个 SQLite 事务；错误向上传播，不再 `let _ =` 吞掉）
- 权威状态：`core/src/storage.rs`（SQLite，WAL，动作去重回执、事件序列、投递批次账本、`expire_approval`）
- 执行：`engine/src/runtime.rs`（线程化回合循环；`QUEUED` TurnRun = 持久化执行意图；超时会中断成员）
- 工具/权限唯一入口：`engine/src/gateway.rs::ToolGateway`（批准/全自动；web/MCP 工具执行层 fail-closed）
- 信息权限：`core/src/views.rs`（`audience` 可见 ≠ `push` 注入；观察者按 scope 裁剪载荷）
- 产品层：`engine/src/{session,worker,cli}.rs`（会话服务、`serve` 协议、CLI）；TUI 的 `ui::geometry`
  是渲染与鼠标命中的唯一几何来源（不要再在别处重算行号/列号）

## 代码审查与证据（review/*）

- 现行审查报告：`review/findings-deep-review-2026-09-14.md`（发现+证据）、
  `review/fix-notes-deep-review-2026-09-14.md`（修复台账+测试名）；更早的 `review/*` 是历史资料。
- 后续更新修复与 `/model` 扩展：`review/fix-notes-rust-updates-2026-09-14.md`。
- 只读审查不得修改被审文件；结论必须带可复跑的命令或探针（探针放 /tmp 或 `review/tmp/`），
  报"证伪"前先排除探针自身误差。

## 团队运行操作备忘（实测）

- **中断的任务会卡住整个目标**：成员回合被中断/超时后其任务落 `BLOCKED`；`COMPLETE_TASK`
  只接受 PENDING/RUNNING 且只允许承接者提交 → BLOCKED 任务任何 Agent 都无法结清，唯一路径是
  用户侧 `CANCEL_TASK`（TUI 任务面板选中按 `c`，BLOCKED 无活动回合直接落 CANCELLED）。
  **避免中断成员回合**（步数/时限留足，或拆小任务）；重派任务时用新任务。
- **RT-06**：回合进入终态（含被取消）时其 PENDING 批准自动置 EXPIRED（`core/src/control.rs` 的
  finalize/reconcile 路径），`signal_done` 不会被残留批准卡住；个别残留可在批准面板 `d` 拒绝。
- 派发"读很多、写报告"的重任务时写明**尽早落盘**要求：模型步数上限
  `limits.max_model_steps_per_turn` 会在半途结束回合（产生 `limit_reached` 事件）。

## 代码风格

- Lazy-first：标准库 > 已有依赖 > 新依赖；不引入未要求的抽象、不写"以后可能用"的脚手架。
- 每个非平凡逻辑留一个可运行的检查（acceptance 测试或 `__main__` 自检）；删除代码优于新增代码。
- 给「已知天花板」的简化留 `ponytail:` 注释（写明升级路径）。
- 文档、提交信息与面向用户的输出用中文；代码标识与注释用英文。
- 密钥只从环境变量/本机凭据读取，禁止写入仓库、TeamSpec、提示词或事件。
- Skills 约定：TeamAgents 只复用 `~/.agents/skills`（唯一注册根）；**不使用** `~/.codex/skills`，
  缺哪个技能就专门安装进 `~/.agents/skills`（ponytail 系列已复制安装；scientific-skills-router 不装，检索已由 `skill` 工具覆盖）。
