# 固定任务评测入口

`tasks.jsonl` 是少量可复跑的冒烟任务定义。每行包含 `id`、`prompt` 和 `checks`；真实模型
评测时，调用结构化 CLI 并记录 stdout、stderr、退出码、耗时和人工干预：

```bash
teamagents exec --json --timeout 900 \
  --check 'cargo test --offline --manifest-path core/Cargo.toml' \
  - < prompt.txt > result.jsonl 2> result.stderr
```

评测报告必须保留原始 JSONL 和验收制品，分别统计 `completed`、`failed`、`approval_required`、
`timeout`、错误修复率、错误编辑率、人工介入次数和 token/耗时。没有实际运行的模型、命令或
指标不得填入报告。
