# 任务等待隔离记录（2026-09-19）

本轮先用独立 Rust 探针构造以下路径：Leader 创建只分配给其他成员的任务，无观察权限的 `b`
知道任务 ID 后提交 `wait_for_tasks`，然后完成任务。旧实现允许 `b` 进入 `WAITING_TASK`，
完成时向它显式投递，并在 `wake_info` 中返回 `SECRET-REF`。修复后原探针返回
`ok: false`、`unknown task`，`b` 未登记等待、未收到任务结果。该缺陷对应旧审查 B-04。

修复位于 `core/src/control.rs`：

- `wait_for_tasks` 按当前成员身份、任务直接参与者、Leader 或观察者订阅校验。观察者订阅按任务
  requester/assignee 匹配对象；未完成任务允许订阅任一种结果事件的观察者登记等待，已结束任务要求匹配
  实际状态对应的事件。只订阅 `task_completed` 的合法观察者仍可等待。
- 缺失任务与无权任务返回相同的 `unknown task` 形式；混合任务列表中任何一项不通过，整个动作均不登记等待。
- `wake_info` 与立即返回的等待结果使用同一投影，按实际事件种类和当前 `payload_scope` 裁剪。
  缺失任务、成员已移除、订阅已撤销、未订阅当前事件或无法读取 TeamSpec 时，快照只返回
  `{"task_id": "...", "status": "UNKNOWN"}`，不透露真实状态、参与者或结果引用。
- 显式 waiter push 必须属于事件 `audience`；已授权观察者的投递仍按原有 scope 裁剪。
  `wake_policy=none` 不妨碍成员主动等待已订阅的完成结果。
- 调度不再把“没有 inbox 投递”视为无需检查等待条件。任务结束或等待权限撤销后，等待回合可恢复；
  未订阅的失败、取消、阻塞结果只返回 `UNKNOWN`，不会造成永久挂起。

新增六项回归（`core/tests/engine.rs`）：

| 测试 | 证据 |
|---|---|
| `wait_for_tasks_rejects_unrelated_member_and_filters_stale_waiter` | 新越权等待被拒绝；手工构造的旧 `WAITING_TASK` 行恢复执行、无结果投递，快照严格等于 `UNKNOWN` 结构 |
| `wait_for_tasks_refuses_invisible_tasks_without_partial_registration` | 同一 ID 在不存在与存在但无权时错误一致；与合法任务混用仍无部分登记或等待事件 |
| `authorized_observer_wait_receives_only_its_declared_scope` | 仅订阅完成事件，逐一验证 `status` / `public_message` / `result` 与两种 wake policy；完成投递、唤醒及立即返回均符合范围，未订阅的 PENDING 快照为 `UNKNOWN` |
| `observer_wait_resumes_without_disclosing_unsubscribed_outcomes` | 未订阅的 RUNNING 快照为 `UNKNOWN`；实际失败、取消或阻塞时无投递但恢复等待，快照为 `UNKNOWN`，再次直接查询被拒绝 |
| `observer_wait_uses_current_permissions_after_topology_change` | 经生产提案/应用路径在完成前撤销或降级观察权限；撤销立即释放等待，之后新产生的完成事件与快照遵循当前权限 |
| `task_participants_keep_full_wait_results` | Leader、requester、assignee 的正常结果访问保持，立即返回与唤醒一致 |

原有 `task_wait_parked_run_does_not_block_boundary` 夹具让 `b` 等待一个自己无权访问的任务，
现补上合法观察订阅，使它继续验证“合法挂起回合不阻塞拓扑边界”的原意。

复跑：

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml
make check
make pty
git diff --check
```

独立探针（本机临时目录）：

```bash
cargo run --offline --manifest-path /tmp/teamagents-wait-probe-S3s2nj/Cargo.toml --quiet
```

结果：`make check` 通过，包括格式、全部目标 Clippy（`-D warnings`）、三个 crate 的测试和仓库卫生检查。
core **69 passed**（22 库单测、47 集成测试）、engine **278 passed / 1 ignored**、tui **104 passed**。
`make pty` 的输入/粘贴/退出、鼠标命中和工作区审查三项真终端检查全部通过，`git diff --check` 通过。
`live_codex` 未启用真实服务开关；Cargo 报告的通过数不等于真实服务验收数。

范围边界：本批仅修复任务等待链路，没有修改 `assign_task` 的依赖校验。B-03（权限改变前已经排队、
尚未注入的投递没有复核当前授权）仍未解决，不能将本批的撤权测试解读为所有投递窗口都已修复。
此批结束时 T4/T6 保持部分覆盖；未运行真实模型，不计入真实供应商、Codex 恢复或完整信息隔离验收。
同日后续对 B-03 的复现、修复与实际输入检查见[投递时授权复核](delivery-acl-2026-09-19.md)；
上面的测试计数与边界描述保留本批完成时的口径。
