# Leader 不变量与非法团队恢复（2026-09-19）

本批按方案 §5.1/§5.2/§8 和 D-32/D-35 修复现有团队校验，不改变 Rust 三 crate、
核心单事务或 Leader 决策入口。归档见
[运行记录](eval/runs/2026-09-19-leader-invariants/REPORT.md)。

## 复现与影响

原 `TeamSpec::validate` 只统计同时满足 `id == leader_id` 和 `role == leader` 的成员，
因此额外成员声明为 Leader 不影响统计；也未检查指定 Leader 的运行后端。

- `teamagents validate` 对重复 Leader 的 JSON 返回 `ok` 和退出码 0。
- 新增第二 Leader、将普通成员改成 Leader、将指定 Leader 改成 Codex，
  三类动态变更均返回 `APPLIED`，配置版本增长。
- 持久读取只反序列化，非法旧配置可进入会话恢复。
- 仅补齐结构校验后，再以显式初始配置恢复坏会话，旧启动逻辑仍会把读取错误当作配置缺失，
  经 UNIQUE 分支保存新修订并继续启动。这会绕开恢复时的拒绝。

第二个 `role: leader` 会影响成员环境提示，Codex Leader 缺少组队所需的团队动作入口。
本批没有证明动作授权被绕过；核心动作授权仍依据 `leader_id`，不把该问题描述为已成功越权。

## 修复

1. `TeamSpec::validate` 统计全体 `role: leader`，要求恰有一个、匹配 `leader_id`，
   且为 `runtime_kind: deepagents`。其他角色仍为自由字符串，普通 Codex 成员保持合法。
2. `Store::load_team_spec` 在旧 limits 字段兼容处理后调用同一校验，
   对非法修订返回带会话 ID 和修订号的错误，不改写或回退到旧修订。
3. 核心增加只读 `session_metadata`，通过实际记录存在性区分无配置与坏配置，
   查询失败直接返回错误。`open_session` 仅对尚无任何修订的会话补写初始配置。
   已有配置先成功读取，再处理全自动模式与成员构造；不再吞掉 `state` 错误后尝试覆盖。

JSON/YAML 导入、`save_spec` 和动态 patch 复用既有入口校验。
非法 patch 在计算执行边界或保存新修订前被拒绝，无需改动事务调度算法。
已有合法配置和半创建会话的显式重试保持兼容；无专用自动修复或迁移非法旧记录的命令。

## 验证

新增 **8 项**测试（core 6、engine 2）：

| 测试 | 断言 |
|---|---|
| `topology_patch_rejects_adding_a_second_leader` | 拒绝新增第二 Leader |
| `topology_patch_rejects_promoting_a_second_leader` | 拒绝提升普通成员为第二 Leader |
| `topology_patch_rejects_switching_the_leader_to_codex` | 拒绝将指定 Leader 改成 Codex |
| `save_spec_rejects_invalid_leaders_without_advancing_revision` | 保存入口拒绝，版本、运行状态及成员行不变 |
| `stored_leader_invariants_are_checked_without_rewriting_revisions` | 最新/指定非法修订均拒绝，原 JSON 保持不变，指定合法旧修订仍可读 |
| `session_metadata_distinguishes_missing_and_unreadable_specs` | 新会话、半创建、坏 JSON 与查询错误分别处理 |
| `validate_enforces_one_builtin_leader_in_json_and_yaml` | 两种格式各检查重复/外部/缺失/指向错误 Leader，并接受内置与 Codex 混合团队及自定义角色 |
| `resume_rejects_invalid_leaders_without_replacing_persisted_work` | 重复/外部 Leader 和坏 JSON，在有无初始配置、请求全自动时均拒绝；配置、权限、任务、回合、事件、回执及成员状态不变，锁释放 |

前三项各覆盖 Leader 空闲/正在执行 × 内联/已有提案四个组合。
每次把一个有效操作放在非法操作之前，验证未半应用、无新增边界等待、配置修订不变；
拒绝回执重放不增事件，之后合法角色变更与任务派发仍成功。
既有 `legacy_limits_keys_are_dropped_on_load`、
`failed_open_releases_the_session_lock_and_can_be_retried` 继续通过。

```bash
cargo test --offline --locked --manifest-path core/Cargo.toml
cargo test --offline --locked --manifest-path engine/Cargo.toml --test cli --test session_boot
make check
```

完整 `make check` 退出码 **0**，耗时 **89.558 秒**：core **90**（25 库 + 50 控制场景 + 15 其他集成）、
engine **322**（108 库 + 1 CLI + 213 集成；另 3 ignored）、TUI **104**。
格式、全部目标严格 Clippy 和仓库卫生通过。
本批没有 TUI 操作/布局改动，未重复 PTY；没有新增真实模型请求、供应商兼容或竞品对照成绩。

## 证据范围

正式归档保留初始编译失败日志、修正测试后的行为失败、仅修核心校验时的恢复失败、
最终定向绿日志与完整检查。测试宏和事件字段访问的初始编译错误不计入产品缺陷证据。

`product-and-tests.patch` 只含本批 7 个生产/测试文件的增量，以本批开始时的脏工作树快照为基线，
不等于相对 Git HEAD 的完整 diff。manifest 记录前后哈希、3 份既有文档及新报告范围，
不归档凭据、会话数据库或整份旧源码。旧真实模型记录保持原样；整体成熟度目标仍在进行中。
