# 组队拒绝、模型配置与拓扑结构校验（2026-09-20）

本批在方案 §5.2/§8、T10/T12/T23 与 D-30/D-32/D-33 的既有范围内修复拒绝请求的副作用和
动态操作校验。保留 Rust 三 crate、`Control::submit` 单事务权威和原有会话 profile 准备顺序。
没有调用真实模型服务，也没有将 T10 或其他部分验收项改为完成。

## 复现与修复

1. **普通成员越权补丁先污染模型配置，再被 core 拒绝。** 原网关在核对 Leader 身份前执行
   profile 准备钩子。生产 `open_session` 加本地 HTTP 模型的负例发现，拒绝回执为
   `only the Leader`，Leader 的模型却从 `test` 变成 `forbidden-model`。
   现在网关先读取实际 Leader 身份，普通成员直接走 core 的持久拒绝路径，不运行准备钩子。
2. **重复成员或新成员的 ID 与已有模型配置同名时覆盖原连接。** 自动创建前拒绝与用户模型
   配置同名的成员 ID，错误提示允许更换 ID 或显式引用已有 `model_profile`。
   已有会话 profile 若被 TeamSpec 或 `/model` 覆盖引用，也不能由新增成员请求改写。
   未被使用的拒绝残留允许用同一成员 ID 修正模型。
3. **准备中途失败或保存失败污染内存。** 准备过程现在修改局部候选；只有确实改动 profile
   才保存，并在保存和 catalog 更新成功后安装内存视图。显式 `model_profile` 必须是字符串。
   回归分别在第二个成员的参数校验与 profile 暂存路径写入处制造失败，检查内存、core catalog、
   TeamSpec 版本与文件，并继续同一会话重试合法请求。
4. **拓扑操作吞掉格式错误并返回成功。** 修复前 `changes: []`、`channels: {}` 的操作被标记为
   `APPLIED`，同批前面的合法改名也被写入。现在先用私有的强类型操作枚举解析，再交给既有
   JSON reducer；未知字段、错误字段类型及嵌套结构在丢弃或合并之前被拒绝。
   `set_space_acl` 区分省略与显式 null；省略保留、空数组清空、null 拒绝。
   修改成员后立即解析其结构，防止后续操作掩盖类型错误。
5. **成员广播绕过 D-33。** 原检查只查看 source/targets，普通成员可用 `broadcast` 加
   `targets: ["leader"]` 获得全队发送权限。动态新增通道和新增成员的内嵌通道现在都拒绝
   普通成员广播；Leader 广播保留。没有修改历史导入 TeamSpec 的既有广播行为。

三个新的 core 负例测试在修复前均失败，实际回执为 `ok: true`、`status: APPLIED`。
对应的合法简写测试在修复前后均通过。非法操作测试覆盖空闲/活动 Leader 和内联/已有提案
四种组合，验证配置、成员状态、任务、回合、补丁、版本不变；重复 action 返回同一拒绝回执，
没有重复事件；随后合法变更和委派仍成功。

## 可复跑检查

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml --test engine topology_patch_ -- --nocapture
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e topology_ -- --test-threads=1 --nocapture
make check
make pty
```

新增四项 core 回归：

- `topology_patch_rejects_malformed_operation_fields`
- `topology_patch_rejects_malformed_embedded_channels_and_spaces`
- `topology_patch_rejects_member_broadcast_even_when_targets_only_name_the_leader`
- `topology_patch_keeps_observer_removal_partial_acl_and_leader_broadcast`

新增五项 engine 生产会话回归：

- `topology_nonleader_cannot_change_profiles_before_core_refuses_the_patch`
- `topology_duplicate_member_refusal_preserves_profiles_and_allows_correction`
- `topology_prepare_failure_preserves_memory_catalog_and_allows_retry`
- `topology_unused_profile_can_be_corrected_but_referenced_profile_cannot_be_replaced`
- `topology_profile_name_collision_allows_explicit_existing_profile`

定向检查为 core **8/8**、engine **6/6**，包含既有回归。`make check` 的格式、严格 Clippy、
仓库卫生和全量回归通过：core **94** / engine **341** / TUI **104**；engine 另有三项
显式 ignored，真实 Codex 开关未启用，本轮没有新增真实服务结论。bubblewrap 本机探针通过。
完整检查日志为 `/tmp/teamagents-topology-check-20260920.log`，当前基线同步于
[验收表](../docs/ACCEPTANCE.md)。假模型检查包括恢复后实际 Leader 请求的模型名，不能计为
真实供应商兼容性或自然语言任务成功率。

`make pty` 的输入/恢复冒烟、滚动后鼠标命中和工作区审查三项均通过，日志为
`/tmp/teamagents-topology-pty-20260920.log`。本批文档的本地链接和 `git diff --check` 通过。

## 仍保留的边界

D-30 的 profile 文件/catalog 与 core 补丁不是跨存储事务。准备成功后，补丁仍可能因最终语义
或版本检查被拒绝并留下未使用的 profile；本批不宣称所有拒绝都没有文件副作用。catalog 更新
失败也不回滚已保存的候选文件。已有被使用的配置不允许自动覆盖，未使用的残留允许修正。

强类型校验覆盖这里列出的拓扑操作；外层动作 payload 和其他团队动作尚未全部使用严格 schema。
五家真实服务、混合供应商、真实 TUI/远端工具矩阵及总体成熟度仍按验收表的未完成项推进。
