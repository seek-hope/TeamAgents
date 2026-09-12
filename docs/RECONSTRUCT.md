# TeamAgents TS+Rust 重构（reconstruct 分支）

main 分支保留 Python 实现作为基准；本分支逐步实现 TypeScript + Rust 版本。

## 架构划分

- **Rust（`core/`）= 权威核心**：`models`（spec 即数据，DP-1）、`storage`
  （SQLite WAL，DDL 与 Python 版逐字一致）、`control`（唯一事务入口
  submit：回执去重 → validate → reduce → persist → schedule）。
  对外是 stdio 换行分隔 JSON 服务（`teamagents-core` 二进制），不用
  napi/socket —— 好调试、语言无关。
- **TypeScript（`ts/`）= 编排与交互**：runtime 回合循环、runners、Codex
  协议适配、CLI、TUI。通过 `CoreClient`（stdio JSON）调用权威核心。
  零运行时依赖：`node:test` 跑测试，Node ≥22 原生执行 `.ts`。

## 移植进度台账

| 模块 | Python 源 | 状态 | 验证 |
|---|---|---|---|
| models | models.py (519 行) | ✅ 枚举/结构/TeamSpec 校验全量 | `core` 3 个单测 |
| storage | storage.py (1060 行) | ◐ DDL 全量；操作子集（session/meta/spec/action/event/task/run/delivery/agent/approval/completion） | 随 control 测试 |
| control | control.py (1285 行) | ◐ submit 事务骨架 + 回执幂等 + payload hash + 派生 task_id；**validate/reduce/schedule 未移植**（未移植的 kind 返回可读拒绝） | `failure_receipt_commits_and_replays` |
| views | views.py | ✗ 未开始 | — |
| runtime/runners/codex/permissions/tools/execution/workspace/sessions | — | ✗ 未开始 | — |
| CLI | cli.py (315 行) | ◐ 骨架（`doctor` 走 ping） | `ts` 冒烟测试 |
| TUI | tui/ (~1700 行) | ✗ 未开始 | — |

## 既定决策

- 枚举线路格式与 Python StrEnum 完全一致（SCREAMING_SNAKE / snake_case），
  保证两版可读写同一份事件流与 DB。
- payload hash 用「键排序的规范化 JSON」的 sha256 前 32 位，与 Python
  `json.dumps(sort_keys=True)` 对齐。
- 移植顺序：control 按 action kind 逐个移植，每个以 `tests/test_t*.py`
  对应场景为 oracle；storage 操作随 control 需要增量补齐。

## 快速命令

```bash
cd core && cargo test            # Rust 核心测试
cd ts && node --test test/       # TS↔Rust 端到端冒烟
```
