# 组队请求、提案批准与回执重放（2026-09-20）

本批继续方案 §5.2/§8/§9、T10/T11/T12/T21 和 D-30/D-32/D-33 的现有工作。
使用本地协议服务和实际 SQLite，不调用真实模型服务。承接
[拓扑操作校验](topology-validation-2026-09-20.md)，补齐外层请求及已有提案进入准备钩子的路径。

## 先复现的问题

- 内联请求带 `reject: true` 却仍应用变更；已有提案的 `operations: null` 被静默解释为使用原操作。
  `reject: "false"` 也被宽松真假判断当成拒绝。未知字段与错误类型没有统一拒绝。
- 缺少或过期的 `base_revision` 虽然最终被 core 拒绝，网关此前仍先保存新成员 profile。
- Leader 只传 `patch_id` 批准已有成员提案时，没有执行该提案的自动 profile、工具继承和双向通道准备；
  省略 `model_profile` 的合法提案因此始终不能应用。
- 在含多个会话的同一 SQLite 中，按 ID 全局读取提案可让一个会话应用或拒绝另一会话的提案。
  生产默认按会话独立数据库不改变这个核心隔离缺陷。
- 已存提案的操作 JSON 损坏时被反序列化为默认空数组，仍返回 `APPLIED` 并增加团队版本。
  等待执行边界的空补丁也会被应用。
- profile/通道准备改写了用于动作去重的载荷，同一成功工具调用重放会返回
  `action_id ... was already used with different action data`。准备失败则只返回临时错误，没有持久拒绝回执。

五项请求/存储负例、一个等待边界负例、两个生产会话检查和回执重放检查均先在修复前观察到失败。
其中错误请求与损坏提案实得 `ok: true`、`APPLIED`；生产提案未增加成员；缺版本请求写出了
`profiles.json`；重放先出现载荷冲突，加入准备失败分支后又复现准备钩子被重复调用。

## 修复与兼容行为

1. `propose_team_change` 与 `apply_topology_patch` 的外层载荷使用严格类型及未知字段校验。
   `reject` 只接受布尔值，`true` 必须指定已有提案；显式 null 不能当成省略。已有提案仍允许
   省略 `operations` 或传 `[]` 使用原操作；非空列表为 Leader 的替换操作。
2. 网关在任何 profile 写入前，使用 core 的同一校验检查实际身份、请求结构、提案状态和版本。
   无效请求原样进入 `Control::submit` 留持久拒绝回执。已有提案从当前会话读取后，经过和内联
   请求相同的准备钩子；批准后保留原提案人和正常审计。
3. 提案读取增加 session 条件；操作及受影响成员 JSON 解析失败直接报错，保留原字节与状态。
   等待边界的空操作补丁标记失败、释放 draining 成员，不增加配置版本，也不发应用成功事件。
4. `Control::submit` 增加仅 Rust 调用方可构造的 `ActionSubmission::PreparedTopology` 参数形式。
   **原始请求**用于动作哈希与拒绝/成功回执；准备后的操作只进入同一个事务内的校验和 reducer。
   没有新增 JSON 协议字段、数据库字段或第二个团队事务入口。最终事务仍复核原请求和版本，
   只读预检查不能授权一个已经过期的补丁。
5. 准备成功、核心拒绝和准备失败均可按原始请求返回同一持久回执。缓存回执存在时先走 core
   的身份/哈希检查，不再执行准备；复用 ID 但改参数明确拒绝。实际模型工具说明同步外层类型、
   已有提案的空数组语义和默认值，并删除工具继承说明中相互矛盾的句子。

## 可复跑检查

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test engine topology_ -- --nocapture
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e topology_ -- --test-threads=1 --nocapture
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib prepared_topology_ -- --nocapture
make check
make pty
```

新增六项 core 回归：

- `topology_request_rejects_malformed_inline_envelopes`
- `topology_request_rejects_malformed_stored_decisions_without_deciding_the_proposal`
- `topology_request_rejects_malformed_proposal_envelopes`
- `topology_request_cannot_decide_another_sessions_proposal`
- `topology_request_preserves_corrupt_stored_operations_and_allows_repair`
- `topology_request_empty_waiting_patch_fails_without_advancing_revision`

新增四项 engine 回归：

- `topology_request_invalid_or_stale_envelope_never_prepares_profiles`
- `topology_request_stored_proposal_inherits_leader_defaults_when_applied`
- `prepared_topology_replays_original_receipt_without_running_preparation_again`
- `prepared_topology_rechecks_revision_after_preparation`

定向结果：core **15/15**、生产 Chat 会话 **8/8**、网关 **2/2**，包含既有检查。
回执重放覆盖内联/已有提案、成功/核心拒绝/准备失败、同进程/关闭数据库后重开以及改参数碰撞。
竞态测试在准备钩子内提交另一合法补丁，验证最终提交只保留先完成的版本且重放不会重复准备。
本批没有把数据库重开测试描述为真实模型或进程 SIGKILL 证据。

`make check` 已通过格式、严格 Clippy、三个 crate 全量回归与仓库卫生检查：
core **100** / engine **345** / TUI **104**。engine 另有三项显式 ignored，真实 Codex 开关未启用。
完整日志为 `/tmp/teamagents-topology-request-check-20260920.log`。本批文档本地链接检查通过。

`make pty` 三项通过：输入/恢复冒烟、滚动后鼠标命中、工作区审查；日志为
`/tmp/teamagents-topology-request-pty-20260920.log`。`git diff --check` 通过。

## 仍保留的边界

- D-30 的 profile 文件、catalog 和 SQLite 补丁仍非跨存储事务：准备完成后的最终语义拒绝、
  版本竞态或持久化故障仍可能留下未使用的 profile。没有自动回滚或删除这些残留。
- 新提交使用原始请求哈希。旧版本已记录的哈希与回执保留原样；没有迁移旧版丢失原始载荷的
  预处理记录，也没有承诺自动修复这些历史重放冲突。
- 提案的外层结构现在严格，具体操作语义仍由 Leader 应用时校验；普通提案不授予权限。
  其他团队动作的载荷还未全部完成严格 schema。T10 及真实供应商矩阵保持部分覆盖。
