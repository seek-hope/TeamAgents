# 真实模型评测：2026-09-15（DeepSeek）

- 被测：本仓库工作树版本的 `engine/target/debug/teamagents`（含沙箱工具链镜像修复）
- 模型：用户配置的 `leader_main`（deepseek/deepseek-flash），默认单成员团队
- 命令：`review/eval/run.sh --timeout 600`（每个任务全新工作目录 + 隔离 `XDG_STATE_HOME`）
- 原始证据：同目录 `*.jsonl`（stdout JSONL，含 event/result/usage）

| 任务 | status | exit | 秒 | tokens(prompt/completion) | 验收 |
|---|---|---|---|---|---|
| edit-integrity | completed | 0 | 16.9 | 29911/2459 | 全部通过 |
| long-output | completed | 0 | 20.3 | 32964/2938 | 全部通过 |
| rust-fix | completed | 0 | 20.7 | 59769/2500 | 全部通过 |

汇总：3/3 completed、0 人工介入、0 需批准、总 prompt 122,644 / completion 7,897 tokens，17–21 秒/任务。

## 逐任务复核（工作目录留档于本次运行目录）

- `rust-fix`：src/lib.rs 里 `add` 从 `a - b` 改为 `a + b`，测试未改；验收 `cargo test --offline`
  在沙箱内退出 0。该任务同时证明沙箱可用宿主工具链（修复前 `cargo` 会报
  "rustup could not choose a version of cargo"）。
- `edit-integrity`：`app.ini` 只有 `[staging]` 的 `retries` 变成 5，`[dev]`/`[prod]` 仍是 3；
  验收脚本用 configparser 逐段断言。
- `long-output`：`python3 gen.py` 输出 4001 行 / 282,910 字节，模型把完整输出落盘后取出
  `TOKEN=mid-9f3a1c` 写入 answer.txt；验收断言取值精确相等。

## 未覆盖（不得据此宣称）

- 供应商矩阵：本机只有 DeepSeek/OpenAI 两个密钥，本轮只跑了配置里默认的 DeepSeek；
  Anthropic/Gemini/OpenRouter 等需要凭据，尚未验收。
- 多成员协作任务（Leader + Codex 成员）、被中断恢复、批准回路：本任务集未覆盖。
- 这些任务的提示词带有明确验收线索；不能当作开放式编码能力的等价证明。
