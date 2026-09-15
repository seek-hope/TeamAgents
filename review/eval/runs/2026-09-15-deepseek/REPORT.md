# 真实模型评测：2026-09-15（DeepSeek）

- 被测：本仓库工作树版本的 `engine/target/debug/teamagents`
- 模型：用户配置的 `leader_main`（deepseek/deepseek-flash），默认会话（`team-collab` 由 Leader 自行组队）
- 命令：`review/eval/run.sh --timeout 900`（每个任务全新工作目录 + 隔离 `XDG_STATE_HOME`）
- 原始证据：同目录 `*.jsonl`（stdout JSONL：`session`/`tool`/`event`/`result`，含 usage）

| 任务 | status | exit | 秒 | tokens(prompt/completion) | 工具调用 | 验收 |
|---|---|---|---|---|---|---|
| edit-integrity | completed | 0 | 34.5 | 77993/6174 | 12 | 全部通过 |
| long-output | completed | 0 | 18.0 | 39042/2632 | 6 | 全部通过 |
| rust-fix | completed | 0 | 14.5 | 38278/1448 | 8 | 全部通过 |
| team-collab | completed | 0 | 39.8 | 138031/8009 | 36 | 全部通过 |

汇总：4/4 completed、0 人工介入、0 需批准、0 个失败的工具调用；15–40 秒/任务。

## team-collab：组队能力的前后对比（本轮主要发现）

首轮跑（未修文档/可见性）在 900 秒超时失败，exit 124：Leader 建的成员**没有工具**（只有
send_message/assign_task 等团队工具），成员自己报告 "no file/shell tools visible on my side"，
两个任务永远完不成；同时 Leader 连续 5 次猜错 `apply_topology_patch` 的形状。

修复（只改契约清晰度与可见性，不放宽权限）：把 add_agent 的 operation 形状、`base_revision`、
"不写 tool_bindings 就没有执行工具" 写进工具说明；`<team>` 上下文里每个成员带上 `tools`；
缺 `base_revision` 的报错直接给出当前版本号。修复后同一提示词：36 次工具调用、0 失败、
40 秒通过验收：

`ls`/`read_file` 探明现状 → `apply_topology_patch`（带 `base_revision`，一次成功）建
alpha-fixer/beta-fixer（`tool_bindings: [files, shell]`）→ 两次 `assign_task` → 两个成员并行
`read_file`(含 sha256)/`edit_file`/`shell` 自验 → `complete_task` → Leader `wait_for_tasks`、
自己再跑一遍检查、`publish_shared`、`signal_done`。

## 其它任务复核（`tool` 行即实际动作）

- `rust-fix`：`cargo test` 复现失败 → `edit_file`（`a - b` → `a + b`）→ 再跑 `cargo test` 通过。
  该任务同时证明沙箱内可用宿主工具链。
- `edit-integrity`：`edit_file` 用 `[staging]` 段名 + host 行做唯一锚点，备份 + `diff -u` 自证只改了目标段。
- `long-output`：`python3 gen.py > full_output.txt`（4001 行 / 282,910 字节）→ `grep TOKEN=` →
  `write_file answer.txt` → 复核。

## 未覆盖（不得据此宣称）

- 供应商矩阵：本机只有 DeepSeek/OpenAI 两个密钥，本轮只跑了默认 DeepSeek；其它供应商需要凭据。
- 被中断后的恢复、批准回路、多轮多人协作（>2 成员）未纳入本任务集。
- 这些任务的提示词带有明确验收线索；不能当作开放式编码能力的等价证明。
