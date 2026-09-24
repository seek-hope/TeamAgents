# review：审查、评测与证据

本目录保存**可复跑的检查与它们的原始证据**。结论必须带命令或探针；测试与证据分开记录。

| 路径 | 内容 |
|---|---|
| [`eval/`](eval/README.md) | 固定任务与真实模型评测（当前入口 `eval/r2-p6/run.py`，原始结果在 `eval/r2-p6/runs/`） |
| `eval/runs/`、`eval/r2-p6/runs/` | 逐次运行留下的原始 JSONL、评分日志与逐 trial 状态（**结论的唯一来源**） |
| [`archive/`](archive/PROTOCOL.md) | 更早实现（≤ v0.1.2）与其迁移期的审查报告、修复台账、评测工具与截图 |
| [`dsec-kernel-reference-2026-09-24.md`](dsec-kernel-reference-2026-09-24.md) | 外部沙箱平台经验与内核设计的对照笔记 |
| [`fix-notes-verification-2026-09-24.md`](fix-notes-verification-2026-09-24.md) | 形式化验证发现的问题与修复台账（见 `verification/REPORT.md`） |
| `tmp/` | 被忽略的探针与临时制品（不入库） |

约定：日期化的 `review/archive/*.md` 是**当时**的结论，不描述当前实现；当前行为以
[`docs/DESIGN.zh-CN.md`](../docs/DESIGN.zh-CN.md)、[`docs/USER-GUIDE.md`](../docs/USER-GUIDE.md) 与
[`docs/ACCEPTANCE.md`](../docs/ACCEPTANCE.md) 为准。只读审查不得修改被审文件；
"证伪"结论要先排除探针自身误差。归档材料保留原文，其中的相对链接按当时路径书写，可能已失效（不追改）。
