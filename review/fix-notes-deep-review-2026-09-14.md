# 深度审查修复台账（2026-09-14）

对应发现：`review/findings-deep-review-2026-09-14.md`。基线变化：core 38→43 / engine 78→89 / tui 55→61，
`pty_smoke.py` 与 `pty_click_check.py` 通过。

## P1

| 发现 | 修复 | 回归测试 |
|------|------|----------|
| P1-1 观察者 scope fail-open | views.rs:56 `_` 分支改按 status 档裁剪；models.rs validate 白名单校验 payload_scope/wake_policy | views::tests::unknown_observer_scope_is_rejected_and_fails_closed |
| P1-2 web_fetch 重定向 SSRF | tools.rs http_get_guarded：redirects(0) 手工跟随，每跳过 guard_url；新增 url 依赖解析相对 Location | tools::tests::redirect_chain_is_guarded_at_every_hop、web_fetch_stops_after_ten_redirects |
| P1-3 项目配置注入提示词 | config.rs：项目级 instruction_files/skills_paths 与项目 tools 同走 trust_project_tools 门 | config.rs 测试模块（信任门用例）|
| P1-4 UTF-8 截断 panic | tui/app.rs:455 drain 前回退到字符边界 | delta_truncation_respects_char_boundaries |
| P1-5 轮询游标退化 | tui/main.rs:282 日志面板未激活时 after=app.cursor | pty_smoke 覆盖 |

## P2

| 发现 | 修复 | 回归测试 |
|------|------|----------|
| P2-1 complete_task 无 run_id | control.rs:328 validate 必填 | complete_task_requires_an_active_run |
| P2-2 cancel 漏 QUEUED 回合 | control.rs:1324 QUEUED 直接落 CANCELLED | cancel_task_cancels_queued_run |
| P2-3 boundary 补丁死锁 | control.rs:399 允许拒绝 WAITING_BOUNDARY；892 拒绝恢复 Draining→Idle；1857 agent_has_live_run 排除 in-process 批准挂起回合 | waiting_boundary_patch_can_be_rejected_and_releases_draining、approval_parked_run_does_not_block_boundary |
| P2-4 codex initialize 泄漏 | codex.rs:130 失败先 close() 再 Err（close 幂等） | process_leaks.rs::codex_failed_initialize_reaps_the_child |
| P2-5 MCP initialize 泄漏 | mcp.rs:80 两条失败路径先 close() | process_leaks.rs::mcp_failed_initialize_reaps_the_child |
| P2-6 skill 符号链接逃逸 | tools.rs skill_candidates canonicalize+starts_with 拒绝；member_context 复用 | skill_candidates_reject_symlink_escapes、member_context_rejects_symlinked_skills |
| P2-7 cwd 找 TUI 二进制 | main.rs 搜索根只留 exe 祖先+PATH | main.rs tests::tui_search_roots_exclude_cwd |
| P2-8 工具调用拉全量 state | core server 加 agent_config_revision 端点；session.rs 工厂改用它 | member_executor_factory_probes_config_revision |
| P2-9 run_loop 全量 state | core state 端点加 include_events 参数；core_client.state_brief()；runtime 12 处热路径换用 | state_brief.rs::state_brief_omits_events_and_keeps_the_rest |
| P2-10 批准 toast 不响 | tui/app.rs apply_event 改读新快照 | approval_arrival_toasts_against_new_snapshot、approval_already_decided_stays_quiet |
| P2-11 点击命中错行 | app.rs select_row_visible 先按 key 解析当前索引；删重复的 panel_rows | click_hits_right_row_after_task_reorder |
| P2-12 断连无反馈 | 轮询连败 3 次出状态栏 chip+聊天提示（复用既有 i18n 文案）；恢复自动清除；submit/close 线程化留 ponytail 注释 | disconnect_chip_shows_and_clears |
| P2-13 共享面板不可滚动 | shared_rows 稳定 key（space_id:sequence）入 panel_row_keys | shared_panel_scrolls_with_stable_keys |

## 顺带修复

- engine/Cargo.toml 新增 `url = "2"`（重定向 Location 相对解析，P1-2 需要）
- 修复 Wegener 代理遗留：session.rs 测试 Arc 双包、spec 缺 leader；main.rs 测试误解 ancestors 语义

## 已知天花板（未做，有意）

- TUI submit/close 仍在 UI 线程同步调用，引擎卡死时最坏冻结 120s（main.rs:383、worker.rs:107 有 ponytail 注释，升级路径=后台线程化）
- 事件/成员历史无界增长（存储层无 retention）——P3，归档/删除会话兜底
