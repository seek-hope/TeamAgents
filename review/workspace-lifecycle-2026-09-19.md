# 工作区生命周期与成果保护（2026-09-19）

## 目标与范围

承接工程化基线 `70c83b6`，按方案 §12.3、T18、D-32/D-35 推进复杂编码任务与恢复稳定性。
不引入新执行后端、权限模式、跨会话记忆或后台终端协议；保留 Rust 三 crate 和现有会话格式。
本轮解决实际工作区与成果保护问题，不以增加测试数宣称达到 Codex CLI、Claude Code、pi、Hermes 的整体水平。

## 先复现、再修复

对原实现增加独立真实 Git 集成测试后，首批 **9/9 失败**。每项使用临时仓库，
Git 操作返回码均断言成功，测试持有 `support::TestEnv`；不触碰用户仓库或调用模型。
后续还独立复现了被 Git 忽略的交付文件随会话删除的问题。

| 缺陷 | 修复与回归（`engine/tests/workspace_lifecycle.rs`） |
|---|---|
| 主项目变脏时，已有成员 worktree 被改用 shared，读不到进度且后续写入落到用户目录 | 优先核对并复用已有成员根；`reopening_worktree_keeps_member_root_when_project_becomes_dirty`，另以 `resumed_session_file_tools_write_to_the_member_worktree_not_dirty_project` 验证生产会话启动、恢复及实际文件工具调用 |
| worktree 可能属于另一个项目，损坏的 `.git` 也被当作有效 | 核对工作根与 common Git dir，错误不静默回退；`reopening_worktree_refuses_a_different_repository_and_damaged_git_metadata` |
| 同秒建立两个会话的同名成员，时间戳分支名碰撞 | 使用已有 UUID 依赖生成独立分支；`different_sessions_get_distinct_member_branches` |
| 清理只看缓存分支，detached HEAD 的新提交可被删除 | 对实际 HEAD 做祖先关系检查，未合并就拒绝；`cleanup_protects_the_actual_detached_head_until_merged` 同时验证合并后可清理 |
| 用户 Git 配置可隐藏未跟踪输入；Git status 错误被当作干净 | 显式要求全部未跟踪文件与子模块状态，错误保守拒绝；`dirty_detection_fails_closed_on_git_errors_and_hidden_untracked_files` |
| 成员切到用户分支后，清理会删除该分支 | 新建时记录原始分支归属和基线到成员私有目录的 `worktree.json`；仅清理有来源记录且已合并的原分支，不强删；`cleanup_never_deletes_a_user_branch_after_member_switches_to_it` |
| 删除到后一个脏成员才失败，前一个成员已经被删 | 先核对所有成员，再逐个清理且重复检查；`deleting_a_session_preflights_every_worktree_before_removing_any` |
| 归档只移动文件夹，Git backlink 仍指旧位置且变为 prunable | 移动后修复各 worktree 注册；`archiving_worktrees_repairs_registration_and_preserves_unmerged_results` 验证归档后 Git 可读、未合并拒删、合并后可删 |
| 同名归档直接被覆盖，旧成果消失 | 明确报错并保留双方；`archive_collision_preserves_both_sessions_instead_of_overwriting` |
| `.gitignore` 中的文件即使有交付结果也被 Git 无强制删除 | 清理时额外检查忽略文件；`session_cleanup_preserves_ignored_results_until_explicitly_removed` |

补充失败、兼容与并发边界：

- `old_sessions_without_origin_metadata_resume_but_keep_their_branch`：无新元数据的老会话继续恢复；
  不推断旧分支归属，宁可保留分支。
- `manually_moved_worktrees_are_repaired_on_resume_and_missing_markers_block_deletion`：手动移动后恢复
  修复注册；已记录 worktree 的标记丢失不被当作“没有工作区”而删除。
- `locked_or_corrupt_members_prevent_partial_session_deletion`：Git 锁、损坏标记阻止删除，
  不先删除其他成员；损坏时也拒绝归档。
- `member_git_checks_ignore_inherited_git_environment_and_fsmonitor_hooks`：核对 Git 状态不受
  `GIT_DIR/GIT_WORK_TREE/GIT_COMMON_DIR/GIT_INDEX_FILE` 重定向，不触发仓库配置的 fsmonitor 命令。
- `session_mutations_respect_a_live_session_lock`：活跃会话不能归档/删除，释放后可清理。
- `failed_archive_repair_rolls_back_and_releases_the_session_lock`：以临时 Git 包装器注入第二个成员
  注册修复失败，验证第一成员已改动的注册也被恢复、目录回到原处、锁释放后可重试。
  在故障点用屏障尝试从归档路径删除，验证移动后的原锁仍被持有。

## 可复跑验证

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test workspace_lifecycle
make check
make pty
```

- 新增 **17 项**真实 Git/会话回归全部通过。会话文件工具测试用确定性成员驱动；初始 Chat runner
  只构造不请求模型，不将此测试归类为真实模型效果。
- `make check`：格式、三个 crate 全目标严格 Clippy、回归、仓库卫生全部通过。
  Cargo 基线：core **63**、engine **240**、TUI **91**；另 1 项显式 ignored。
- `make pty`：启动/粘贴/发送/面板/恢复/退出，以及普通与滚动后点击命中两项均通过。
- 真实服务跳过条件沿用 [验收表](../docs/ACCEPTANCE.md)，本轮未使用模型凭据。

## 保留的边界与后续

- worktree 成员目录与 Git 注册不能组成跨目录/跨仓库原子事务。归档中的普通错误会尝试回滚；
  kill -9、断电或回滚本身的磁盘故障仍可能需要按错误中的路径手工修复。
- 会话锁保护合作的 TeamAgents 操作，不约束用户编辑器、外部 Git 和磁盘故障。
  删除的全量预检消除确定性的“先删一半再发现未合并”，但不承诺检查之后永不发生外部写入。
- Git 忽略目录也可能包含成果，自动清理因此保守拒绝，包括构建缓存；用户应先核对再移除。
  旧会话无法证明分支归属时保留分支，不做破坏性猜测。
- 交付审查仍只有最近文件工具 diff；完整多批次/Shell/Codex 改动审查、完整成员历史浏览、
  多语言复杂任务隐藏评分、多供应商真实验收与四产品公平对照仍待推进。此批不缩减总体目标。
