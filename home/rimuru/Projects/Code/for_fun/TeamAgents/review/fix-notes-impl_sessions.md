# fix-notes-impl_sessions — B-01 / AD-1 / AD-2 / AD-3

范围：`src/teamagents/{workspace,session,sessions,cli}.py`（仅这四个归属文件 + 新增测试）。
证据：`review/tmp/impl_sessions-fix-evidence.txt`；补丁脚本（可审计、从修复前源码可重放）：`review/tmp/impl_sessions_patch.py`；
备份：`.pre-fix-backup/{workspace,session,sessions,cli}.py`（修复前原件）。

## 改动摘要（文件 + 行号）

| 条目 | 文件 | 位置 | 改动 |
|---|---|---|---|
| AD-1 | `src/teamagents/workspace.py` | 71–100（`prepare`，worktree 分支） | ① `path/.git` 是文件 → 复用现有 worktree，返回其真实 branch（`rev-parse --abbrev-ref HEAD`，detached 时 None）与 merge-base；② `path` 存在但不是 worktree → `WorkspaceError`（明示路径、不删除不覆盖）；③ 路径不存在但同名 branch 残留（崩溃/手删 worktree）→ `worktree add <path> <branch>` 重挂，而不是 `-b` 撞名失败 |
| AD-2 | `src/teamagents/session.py` | 115–121（取锁后立即登记）+ 185–187（失败兜底） | `AsyncExitStack` 与 `os.close(lock_handle)` 回调提前到取锁之后、任何可能失败的操作之前；整个构建过程包在 `try/except BaseException` 中，失败时 `await stack.aclose()`（同时关掉 checkpointer）后原样抛出 |
| AD-3 | `src/teamagents/sessions.py` | 129–139（`new_session_id`） | 分配 id 前扫描 `sessions_dir()` 与 `sessions_dir()/"archived"` 两组目录名（原来只扫 active），归档过的 id 不再复用 |
| B-01 | `src/teamagents/cli.py` | 220（游标初始化）、232–234（推进）、275–279（打印） | 局部游标 `cursor=0` 取代不存在的 `rt.ui_cursor`；`_print_event` 新增 `leader_reply`（`payload["text"]`，截断 2000 字符，用户向文案 `[Leader] …`）与 `run_failed`（`payload["error"]`，截断 400，`[运行失败] <agent>: …`） |

未改动：TUI 代码、cleanup/archive/delete 语义、任何依赖。

## 复现（修复前，均为审查给出的脚本）

```
$ .venv/bin/python review/tmp/repro_worktree_reopen.py /tmp/ta-repro-wt
2nd prepare RAISED WorkspaceError: ... fatal: a branch named 'teamagents/b-1789187414' already exists
3rd prepare (1s later) RAISED WorkspaceError: ... fatal: '/tmp/ta-repro-wt/members/b/work' already exists

$ .venv/bin/python review/tmp/repro_session_id_collision_a.py
new_session_id (nothing exists): proj_5f6e98b6c554
new_session_id after archiving: proj_5f6e98b6c554 -> collides: True
list_sessions ids+archived: [('proj_5f6e98b6c554', False), ('proj_5f6e98b6c554', True)]
duplicate session ids visible to the UI: True

$ .venv/bin/python review/tmp/repro_lock_leak.py
after clean close: locked = False lock fds: []
2nd open RAISED WorkspaceError: git worktree add failed: ...
after FAILED open: locked = True lock fds: ['6']
retry RAISED SessionInUse: session is already running in another process ...

$ .venv/bin/python review/tmp/repro_plain_repl.py   # B-01：真实 SessionRuntime 跑 --plain 循环
  [input received: goal_...]
REPL RAISED AttributeError: 'SessionRuntime' object has no attribute 'ui_cursor'
```

## 验证（修复后）

单条复现脚本（完整输出见 `review/tmp/impl_sessions-fix-evidence.txt`）：

```
$ .venv/bin/python review/tmp/repro_worktree_reopen.py /tmp/ta-repro-wt2
1st prepare -> git_worktree /tmp/ta-repro-wt2/members/b/work teamagents/b-1789187742
2nd prepare -> git_worktree /tmp/ta-repro-wt2/members/b/work teamagents/b-1789187742
3rd prepare (1s later) -> git_worktree /tmp/ta-repro-wt2/members/b/work teamagents/b-1789187742
branches after: ['*', 'master', '+', 'teamagents/b-1789187742']      # 只多一条成员 branch

$ .venv/bin/python review/tmp/repro_worktree_open_session.py
1st open OK; dirty now: False
2nd open OK: <teamagents.runtime.SessionRuntime object at 0x...>      # AD-1：二次 open_session 成功

$ .venv/bin/python review/tmp/repro_lock_leak_forced.py             # AD-1 修好后原锁泄漏脚本不再触发失败路径，故用注入失败
open_session RAISED RuntimeError: injected failure while opening the session
after FAILED open: locked = False lock fds: []
retry open OK: s1
after clean close: locked = False lock fds: []

$ .venv/bin/python review/tmp/repro_session_id_collision_a.py
new_session_id after archiving: proj_5f6e98b6c554_2 -> collides: False
list_sessions ids+archived: [('proj_5f6e98b6c554_2', False), ('proj_5f6e98b6c554', True)]
duplicate session ids visible to the UI: False

$ .venv/bin/python review/tmp/repro_plain_repl.py
hasattr(rt, 'ui_cursor') -> False
  [input received: goal_...]
  [Leader] 计划已就位：先做 A，再做 B
cli.main returned 0
```

注：AD-2 的 `repro_open_session_lock_leak.py` / `repro_lock_leak.py` 用「worktree 二次打开失败」当作失败触发器；AD-1 修好后该路径已成功，因此新增 `review/tmp/repro_lock_leak_forced.py`（monkeypatch `session.Store` 注入异常）继续单独验证 AD-2。AD-3 的 `repro_session_id_collision.py` 结尾用 Textual DataTable 复现 `DuplicateKey`，在本沙箱无 ActiveApp 会抛 `NoActiveAppError`（环境限制，与本次改动无关）；关键断言行（collides / duplicate ids）在该崩溃前已打印，`_a` 变体完整通过。

新增回归测试（10 条，全部独立可跑）：

- `tests/test_b01_plain_cli.py`（4 条）：`_print_event` 渲染 leader_reply（含截断）/run_failed；用无 `ui_cursor` 属性的假 runtime 驱动真实 `cli.main(["--plain", …])` 循环，断言回复只打印一次、游标按事件推进（`store.calls == [0, 2]`）、`inspect.getsource(cli._repl)` 不含 `ui_cursor`。
- `tests/test_p5_workspace.py`（+3 条，追加）：worktree 复用（返回同一 path/branch、成员未提交产物保留、`git worktree list` 只有一条）；已存在非 worktree 目录 → `WorkspaceError` 且不销毁文件；cleanup 之后可重新建。
- `tests/test_session_recovery.py`（3 条）：AD-1 会话级（worktree 成员会话二次 `open_session` 成功、产物保留）；AD-2（注入失败后 `is_session_locked` 为 False，重试不再 `SessionInUse`）；AD-3（归档后 `new_session_id` 返回 `<id>_2`，`list_sessions` 无重复 id）。

```
$ .venv/bin/python -m pytest tests/test_b01_plain_cli.py tests/test_session_recovery.py tests/test_p5_workspace.py tests/test_p6_sessions_ui.py -q
21 passed in 9.77s

$ .venv/bin/python -m pytest tests/ -q
2 failed, 120 passed, 12 deselected in 43.44s
# 2 failed = 既有环境失败：test_config_cli.py::test_xhigh_maps_to_max_for_models_without_xhigh
#（缺 DEEPSEEK_API_KEY）、test_p3_web_tools.py::test_ssrf_guard_blocks_internal_targets（无 DNS）。
# 二者在修复前源码上同样失败（已实测），基线 109→120 为本轮各实现者新增测试所致。
```

防回归有效性实测：把四个文件换回 `.pre-fix-backup/` 版本后，新测试 9 条失败（1 条 cleanup 后重建与修复前无差别，属补充覆盖），换回修复版 16 条全绿。

## 遗留 / 风险

- `--plain` 的游标从 0 开始：`--resume` 时会先回放已有事件（含历史 Leader 回复）。按任务要求保持「初始 0」，未做「恢复时跳到最新序列」的额外优化。
- AD-1 的「残留 branch 重挂」路径在「同名 branch 已被另一个 worktree 检出」时仍会由 git 拒绝（报错信息明确，不静默）；时间戳使同名概率极低。
- `Workspace.path` 存在但为空目录（例如外部工具先建了目录）现在会 `WorkspaceError`；这是任务要求的显式失败，未见既有测试或流程依赖「静默用空目录」。
- AD-2 只保证 `open_session` 自身的失败路径释放锁；TUI 侧的会话切换失败展示由 impl_tui 跟进。

## B-01 任务结清

- 任务 id：`task_893f1e261bc7`（修复 `teamagents --plain` REPL 必崩 + 不打印 Leader 回复）。
- 当前状态：本轮开始时任务板仍显示 **RUNNING**（上一回合提交的完成回执未生效），本轮按 Leader 核对要求重新提交 `complete_task` 结清。
- 结清依据（详见前文 B-01 行）：改动 `src/teamagents/cli.py:220 / 232-234 / 275-279`；测试 `tests/test_b01_plain_cli.py`（`pytest tests/test_b01_plain_cli.py -q` → 4 passed）；端到端证据 `review/tmp/repro_plain_repl.py`（真实 SessionRuntime）→ `[Leader] 计划已就位：先做 A，再做 B`、`cli.main returned 0`；套件 `.venv/bin/python -m pytest tests/ -q` → 2 failed（既有环境失败，修复前同样失败）/ 120 passed / 12 deselected。
- 本轮动作：仅追加本节；未重跑测试、未改动任何源码或其他文件（镜像副本同步为同一内容）。
