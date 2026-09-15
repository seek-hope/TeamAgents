# 真实模型评测：2026-09-15（DeepSeek）

- 被测：本仓库工作树版本的 `engine/target/debug/teamagents`
- 模型：用户配置的 `leader_main`（deepseek/deepseek-flash），默认单成员团队
- 命令：`review/eval/run.sh --timeout 600`（每个任务全新工作目录 + 隔离 `XDG_STATE_HOME`）
- 原始证据：同目录 `*.jsonl`（stdout JSONL：`session`/`tool`/`event`/`result`，含 usage）

| 任务 | status | exit | 秒 | tokens(prompt/completion) | 工具调用 | 验收 |
|---|---|---|---|---|---|---|
| edit-integrity | completed | 0 | 24.0 | 38530/3866 | 9 | 全部通过 |
| long-output | completed | 0 | 16.1 | 29753/2385 | 8 | 全部通过 |
| rust-fix | completed | 0 | 21.4 | 41421/3342 | 9 | 全部通过 |

汇总：3/3 completed、0 人工介入、0 需批准；prompt 109,704 / completion 9,593 tokens；16–24 秒/任务。

## 逐任务复核（`tool` 行即实际动作，摘录）

- `rust-fix`：`which cargo && cargo --version && cargo test` → `edit_file`（`a - b` → `a + b`，带
  上下文与 SHA-256 校验）→ 再跑 `cargo test` → `read_file` 复核 → `signal_done`。
  该任务同时证明沙箱内可用宿主工具链：修复前 `cargo` 会报 "rustup could not choose a version of cargo"。
- `edit-integrity`：`read_file`(含 sha256) → `edit_file`（用 `[staging]` 段名 + host 行做唯一锚点）→
  `read_file` 复核 → 用备份文件 `diff -u` 证明只有目标段落变了。
- `long-output`：`python3 gen.py > full_output.txt`（4001 行 / 282,910 字节）→ `grep TOKEN=` →
  `write_file answer.txt=mid-9f3a1c` → `grep -c`/`awk` 复核。

## 未覆盖（不得据此宣称）

- 供应商矩阵：本机只有 DeepSeek/OpenAI 两个密钥，本轮只跑了配置里默认的 DeepSeek；
  Anthropic/Gemini/OpenRouter 等需要凭据，尚未验收。
- 多成员协作任务（Leader + Codex 成员）、被中断恢复、批准回路：本任务集未覆盖。
- 这些任务的提示词带有明确验收线索；不能当作开放式编码能力的等价证明。
