# 工作区实际变更审查（2026-09-19）

## 本批范围

沿 D-32 的交付验证改进，在工作区生命周期修复之后补齐“用户实际核对发生了什么”的路径。
原实现只把成员最近一次 `edit_file/edit_files/write_file` 的返回字符串留在 TUI 内存；多批次覆盖、
Shell/外部后端编辑、恢复后的变更没有可靠清单，日志重放还会清空审查缓存。

本批替换为 engine 根据磁盘实际内容生成的只读报告。保持 Rust 三 crate、成员工具权限、core 事务入口；
未新增依赖、模型工具、回滚语义、外部服务或产品范围决策。没有进行真实模型或四产品对照评测。

## 实现

- `engine/src/review.rs`：首次使用成员实际工作根时保存文件清单、SHA-256、大小、权限位、文本/链接原文。
  包括原有未提交输入，既不是 Git HEAD，也不是工具输出。每个会话按规范化工作根分组；同根成员共用基线，
  不同会话独立。基线从捕获开始时刻标记；manifest 原子落盘，失败标记防止后续静默重建起点。
- `session.rs`：Chat/Codex 生产执行根准备时注册；首次捕获失败只警告，不阻止成员执行，审查时明确缺少基线。
  人类查询先校验成员，不向模型视图或工具注册表暴露会话快照。
- `worker.rs`：一个后台审查请求在途，第二个明确报忙；状态、取消、批准仍可走原控制线程。
  正常关闭/输入 EOF 发取消信号，Git 进程组终止并回收；读报告不重新建立会话状态目录。
- `tui/src/review.rs`、`app.rs/main.rs/ui.rs`：`/review` 查看 Leader，团队 `v` 查看选中成员，日志 `v`
  查看筛选成员（否则 Leader）。列表→文件→120 行分页；`i` 查看可滚动/横移的完整说明、限制与元数据；
  `r` 刷新，`Esc` 返回/关闭，`Ctrl+G` 切批准，`Ctrl+Q` 退出。
- 报告 revision 绑定基线时间、变化清单与警告；翻页时重读，检测到变化拒绝拼接不同版本。
  前端请求代次保护关闭、返回、换文件、换会话后的迟到回复；加载时清除旧文件详情。
  控制字符与双向控制符转义，鼠标/粘贴不穿透到被遮住的输入或面板。
- 用户指南、README、`/help`、中英文键位提示及验收表同步；`make pty` 加入审查端到端脚本。

## 可复跑证据

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml \
  --test workspace_lifecycle --test workspace_review --test workspace_review_protocol
cargo test --offline --locked --manifest-path tui/Cargo.toml --test review_tests
make check
TEAMAGENTS_REVIEW_PTY_EVIDENCE=/tmp/teamagents-review-pty-frames make pty
```

### engine 新增 19 项

`engine/tests/workspace_review.rs` 共 16 项：

| 检查 | 证据 |
|---|---|
| 原有脏输入、多批次外部编辑/Git 提交、删除/重命名、恢复 | `dirty_inputs_are_the_baseline_and_all_edit_batches_survive_reopen` |
| 非 Git 增删、权限位（含 sticky）、二进制哈希、纯权限无文本差异 | `non_git_files_have_diffs_for_add_delete_mode_and_binary_changes` |
| 链接目标不读、父目录替换越界、非 UTF-8 链接目标、路径穿越 | `symlinks_and_replaced_directories_never_expose_external_file_contents` |
| 忽略文件与私有状态排除、私有目录别名不泄露内容 | `ignored_files_and_session_private_state_are_not_copied_or_disclosed`、`a_parent_symlink_cannot_redirect_baseline_paths_into_private_state` |
| 原工作目录被链接到 `.git` 后不读 Git 私有元数据 | `parent_aliases_cannot_read_git_metadata_as_old_workspace_files` |
| 同根共享、跨会话独立、生产 runner 首次注册/恢复、isolated/worktree 范围 | `shared_members_reuse_one_baseline_and_sessions_stay_independent`、`production_session_captures_before_writes_and_resumes_the_same_review`、`isolated_and_worktree_roots_review_their_files_not_sibling_private_context` |
| 大文件/FIFO/长行/文件数上限、损坏 Git 不退回另一范围 | `large_special_and_long_line_outputs_report_incompleteness_without_blocking`、`corrupt_git_index_and_file_count_limit_are_explicit_not_clean_reviews` |
| 400 行分页覆盖、不执行 external diff/fsmonitor、Git 子目录、还原后列表清空 | `paged_diffs_cover_the_whole_change_and_do_not_trust_diff_configuration`、`git_subdirectory_review_is_scoped_and_refresh_can_show_reverted_changes` |
| 损坏 manifest/blob、初捕失败不重建、遗漏基线不伪装新增 | `tampered_baseline_and_failed_initial_capture_never_become_clean_evidence`、`unknown_baseline_paths_are_not_mislabeled_as_new_files` |
| 分页期间另一文件发生变化也拒绝旧 revision，刷新后可读 | `pagination_rejects_changes_between_pages_until_explicit_refresh` |

`engine/tests/workspace_review_protocol.rs` 共 3 项：

- `review_protocol_validates_input_and_preserves_baseline_across_worker_restart`：真正 `serve` 子进程，
  无会话/未知成员/越界路径/非法分页/错误 revision 拒绝；重启 worker 仍对同一原始基线生成差异。
- `slow_review_does_not_block_state_cancel_or_close_and_reaps_git`：临时 Git 包装器用文件屏障卡在枚举，
  可 ping/state，第二个审查拒绝，真实运行中的 scripted Leader 取消收敛到 CANCELLED；正常关闭回收包装器。
  初始用生产 runner 建立基线，然后恢复成确定性 runner；没有模型 API 请求。
- `a_late_review_after_archive_never_recreates_session_state`：审查在后台停住时归档，放行后可结束读取，
  原会话路径始终不存在。旧文本 blob 已随归档移动时允许明确读错，不回写旧路径。

### TUI 与真终端

新增 `tui/tests/review_tests.rs` 8 项：入口/成员选择，翻页 revision/刷新，迟到/切换隔离，加载期间批准与退出，
完整警告/元数据可达，长路径/控制符/双向控制符，中文/英文/极小尺寸 TestBackend，以及空列表/无文本差异/未读说明。
原最近工具 diff 单测替换为 `review_overlay_requests_actual_workspace_changes_and_ignores_tool_text`。

`tui/scripts/pty_review_check.py` 使用真实 TUI+engine、独立 config/state、无凭据：
外部修改→`/review`→第一/二/三页→上一页→元数据→刷新→批准面板→退出→明确 `--resume` 原会话重看原基线。
可选 `/tmp` 纯文本帧已逐页检查；三项 `make pty`（原输入、点击、新审查）均通过。

### 汇总

- `make check` 格式、严格 Clippy、全部回归、仓库卫生通过：core **63** / engine **259** / TUI **99**。
- engine 另有 1 项显式 ignored；`live_codex` 未设服务开关而提前返回，不能算真实 Codex 服务通过。
- 本机 bubblewrap 检查可运行；没有使用真实模型凭据，没有调整模型上下文窗口，也没有新增真实评分结果。
- 本次临时日志：`/tmp/teamagents-review-verified-check.log`、`/tmp/teamagents-review-verified-pty.log`。

## 调试中的假设修正

- 沙箱在 `/tmp/.git` 暴露只读空占位：祖先标记不能一概当 Git 仓库。现在祖先要求 `.git` 文件或 `.git/HEAD`；
  工作根自身有损坏/空 `.git` 仍明确失败，不静默切非 Git 范围。旧的 `/tmp` 根测试会出现该警告，主流程照常；
  专项回归与新 PTY 使用真实独立工作目录验证功能。
- 第一版 PTY 恢复探针没有传 `--resume`，实际创建了 `_2` 新会话，按设计建立新基线；从帧里的会话 ID 确认后
  修正探针，而不是修改产品恢复行为。修正后原始输入与刷新后的内容同时出现在恢复 diff。
- 并发取消探针最初误以为只替换一个成员为 scripted；实际上 scripts 选项替换全团队，因此未使用文件的成员没有
  注册基线。改成先构造生产基线、再恢复确定性 runner。取消请求也不能直接当终态：以期限内轮询 CANCELLED 验证收敛。

## 明确保留的边界

1. **净变化而非历史**：只比较首次观察与当前字节；创建后又删除、改动后完全还原不会留下历史条目。
   旧会话升级前内容不可重建；工作根换路径是新基线；归档仅保留数据，没有归档浏览器。
2. **范围与限制**：Git tracked+项目未忽略文件（不加载全局 Git 配置）；非 Git 跳过构建/依赖目录；
   私有状态、Git 元数据不读。最多 5000 枚举项、单文件 2 MiB、每根 64 MiB 内容预算；
   两个扫描阶段检查 10 秒预算，Git 限时 10 秒/输出 2 MiB；diff 上限 20000 行、每行 4000 字符。
   超限/FIFO/子模块/非 UTF-8 路径等明确未完成，不能以空列表证明完整无改动。
3. **无 FS 原子性**：读取期间变动会尽可能检测，但不是文件系统事务、签名证据或防同用户篡改机制；
   revision 防不同扫描版本混页，不能阻止返回后继续修改。阻塞文件系统操作不受硬超时保障。
4. **成本与持久化**：初捕在执行前同步进行，可能增加打开会话和新成员准备时间，捕获锁为进程级。
   每次审查重扫（已有 `ponytail:` 升级说明）；文本为本地明文 0600/目录 0700，不注入模型，
   任务输入可能敏感。每根内容预算不是整会话配额，异常退出可能残留临时 diff/未提交 blob；没有后台快照垃圾回收。
5. **非交付评分**：审查不运行测试，不合并、不撤销、不改变批准模式；共享根不声称个人归属。
   真实模型完成率、多语言隐藏评分、更长任务恢复、完整成员历史浏览和多供应商/竞品公平对照仍待推进。

总体成熟度目标未完成，本批仅为有证据的阶段推进。
