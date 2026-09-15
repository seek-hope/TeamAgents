# 带 Codex 成员的跨后端评测：team-codex（2026-09-15，DeepSeek + Codex）

- 被测：本仓库工作树版本的 `teamagents`，TeamSpec 见 `review/eval/tasks/team-codex/team.yaml`
  （Leader = Chat 成员，`codex-dev` = `runtime_kind: codex`）
- Codex 成员**不使用官方订阅**：`model_profile = codex_deepseek`，其 `codex_profile = "deepseek"`
  会把 `$CODEX_HOME/deepseek.config.toml`（`model_provider = deepseek`、`env_key = DEEPSEEK_API_KEY`）
  展开成 `codex app-server -c ...` 覆盖
- 命令：`review/eval/run.sh --only team-codex --timeout 900`（该任务的 `config.toml` 只在本次运行生效）

| status | exit | 秒 | tokens(prompt/completion) | 工具调用 | 验收 |
|---|---|---|---|---|---|
| completed | 0 | 37.9 | 641212/4465 | 9 | 全部通过 |

实际链路：Leader（DeepSeek chat）`ls`/`find`/`cargo test` 复现失败 → `assign_task` 派给 codex-dev →
`wait_for_tasks` 等待 → codex-dev（Codex app-server，DeepSeek 后端）改 `src/lib.rs` 的 `mul` 为
`a * b` 并 `complete_task` → Leader 自己复跑 `cargo test`（1 passed）→ `signal_done`。

## 本轮顺带修掉的两个 Codex 后端问题

1. **`codex --profile X app-server` 在当前 CLI 上被拒**（`--profile` 只适用于 runtime 命令与
   `codex mcp`），一开始 5 个回合全部 "CodexError: codex app-server exited"。现在改为把
   `<CODEX_HOME>/<name>.config.toml` 展开成 `-c key=value`（嵌套表变点号键）传给 app-server。
2. **回复/摘要词间空格**：deltas 是连续片段、`item/completed` 又重复同一段文本，原来用空格
   join 导致 "I 'll  start  by ..."，且摘要里同一段出现两遍。现在按 run 累积一个文本缓冲
   （deltas 直接拼接，item 文本仅在不是尾部时追加），回复与 `complete_task` 摘要都取自它。
3. 另外 "codex app-server exited" 现在带最后 3 行 stderr，坏参数与崩溃不再无法区分。
