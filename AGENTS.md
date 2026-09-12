# TeamAgents 仓库约定（对本仓库工作的所有 Agent 生效）

## 基准与偏离

- `TeamAgents-Implementation-Plan.zh-CN.md` 是产品与实现的基准（P0–P7、T1–T24、DP-1..12）。
- **任何与方案不同的实现（更简单或更好的方案）必须先告知用户并得到确认，才可写进代码。**
  已确认的偏离记录在 `docs/DECISIONS.md`；未确认的只讨论，不落码。
- 阶段的完成条件是方案 §16 的「完成条件」，不能把后续阶段降级为「之后再做」。

## 快速命令

```bash
UV_CACHE_DIR=/tmp/uv-cache uv sync            # 依赖（uv.lock 已锁定）
.venv/bin/python -m pytest tests/ -q          # 全部验收测试
.venv/bin/python -m teamagents doctor         # 依赖/配置/隔离/Codex 协议自检
```

## 架构速览（改代码前先读这 6 行）

- 唯一团队事务入口：`control.py::Control.submit`（ingest→validate→reduce→schedule→persist，单个 SQLite 事务）
- 权威状态：`storage.py`（SQLite，WAL，动作去重回执、事件序列、投递批次确认）
- 执行：`runtime.py`（asyncio，成员独立回合；`QUEUED` TurnRun = 持久化执行意图）
- 工具/权限唯一入口：`agents.py::ToolGateway` → `permissions.py`（批准/全自动）
- 信息权限：`views.py`（`audience` 可见 ≠ `push` 注入；观察者按 scope 裁剪载荷）
- 验收测试按场景编号：`tests/test_t*.py`、`tests/test_p*_*.py`，假成员脚本驱动（`agents.py::FakeMember`）

## 代码审查（review/*）工作区备忘

- 文件工具与 shell 的挂载视图**每个会话可能不同，动手前先核实**。实测两种映射：(a) 文件工具 `/**` = 资源仓库根（`/src`、`/tests`、`/review`、`/tmp` 均落在 repo 下；写 `/tmp/foo` 实际生成 `<repo>/tmp/foo`，shell 的 `/tmp` 与仓库 `/tmp` 不是同一目录）；(b) 文件工具看到的 `/home/rimuru/Projects/Code/for_fun/TeamAgents/**` 是共享 review 空间（物理 `<repo>/home/rimuru/Projects/Code/for_fun/TeamAgents/**`）。写盘后一律用 shell `ls` 核实真实路径。
  - 此时**改 `src/`、`tests/` 必须用 shell**（heredoc + python 精确替换脚本、断言锚点唯一），文件工具会报 not found；改完用 shell `diff -u` 对仓库内备份自证。
  - shell 的 `/tmp` **每次 shell 调用之间会被清空**（沙箱每次调用是新环境）：把"稍后要用的备份/副本"放 `/tmp` 会失效；备份与证据一律放仓库内（如 `<repo>/.pre-fix-backup/`、`review/tmp/`），"改动→回退→再改回"的对比测试必须在**同一次 shell 调用**内完成（或从仓库内副本恢复）。
- 报告写 `<repo>/home/rimuru/Projects/Code/for_fun/TeamAgents/review/findings-<域>.md`，探针脚本放同目录 `tmp/`；只读审查不得改被审文件（本仓库非 git 仓库，无法用 git status 自证）。
- 汇总时以 `<repo>/review/` 为准（nested 副本出现后需合并；`review/REVIEW-REPORT.md` 为终稿）。
- 审查回合模型步数有限：先跑确定性套件（`.venv/bin/python -m pytest tests/ -q`），**尽早**把报告骨架 + 已确认发现落盘，再补探针与其余章节。
- 对抗性验证先核实再定级：把"疑似缺陷"实读代码复核（例：曾疑 `runners.py` 步数上限硬编码 50，实读为按 `limits.max_model_steps_per_turn` 读取，runners.py:540-543，且记于 D-10）；探针要能证伪自己的假设。
  - 主副本在 `<repo>/review/`（findings/fix-notes/REVIEW-REPORT、exploit 型探针如 `review/tmp/exp_escape.py`、`repro_*.py`），file 工具路径的 `review/` 是镜像；发现文件只在镜像不见时，用 shell `cp` 从镜像同步回主副本。
  - 修复批次 1 核对已完成（reviewer_verify）：6 项全部证实；报告 `review/fix-verify-report.md`、25 项探针 `review/tmp/verify_fix_batch1.py`（A1-E2，可复跑）。教训：探针前置条件（如目录不存在）会造成假「证伪」，报结论前先排除探针自身误差。

## 团队运行操作备忘（实测）

- **中断的任务会卡住整个目标**：成员回合被中断/超时后其任务落 `BLOCKED`；而 `COMPLETE_TASK` 只接受 PENDING/RUNNING（control.py:217）、只允许承接者提交 → BLOCKED 任务任何 Agent 都无法结清，唯一路径是用户侧 `CANCEL_TASK`（TUI 已有入口：任务面板选中按 `c`；BLOCKED 无活动回合，直接落 CANCELLED，验收见 tests/test_p6_tui_cancel_refresh.py A-03）。**避免中断成员回合**（步数/时限留足，或拆小任务）；重派任务时用新任务，旧 BLOCKED 任务用任务面板的 `c` 结清。
- **RT-06 已修复**：回合进入终态（含被取消）时其 PENDING 批准自动置 EXPIRED（`control.expire_run_approvals`，在 `_finalize`/`_reconcile` 中调用；验收 tests/test_rt06_approval_expiry.py），`signal_done` 不再被残留批准卡住；个别残留仍可在批准面板 `d` 拒绝。
- 派发审查这类"读很多、写报告"的重任务时：写明**尽早落盘**要求（模型步数上限会在半途掐断回合）。
- **卡住目标的收尾清理（宿主机执行，已端到端验证并在真实会话上执行成功）**：`.venv/bin/python review/tmp/unblock_session.py --apply` 以 user 身份经 `Control.submit` 提交 `CANCEL_TASK` + `APPROVAL_DECISION(deny)`（自动定位含目标 id 的会话 DB；TUI 开着时亦可，WAL+busy_timeout；回执幂等）。取消任务会向 Leader 推 `task_cancelled` 并调度一次通知回合，该回合走完后完成阻塞项清空。验证：`review/tmp/verify_unblock_recipe.py`（live 连接 + 子进程执行），输出 `review/tmp/unblock-recipe-check.txt`。

## 代码风格

- Lazy-first：标准库 > 已有依赖 > 新依赖；不引入未要求的抽象、不写"以后可能用"的脚手架。
- 每个非平凡逻辑留一个可运行的检查（acceptance 测试或 `__main__` 自检）；删除代码优于新增代码。
- 给「已知天花板」的简化留 `ponytail:` 注释（写明升级路径）。
- 文档、提交信息与面向用户的输出用中文；代码标识与注释用英文。
- 密钥只从环境变量/本机凭据读取，禁止写入仓库、TeamSpec、提示词或事件。
