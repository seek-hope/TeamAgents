# 固定任务评测

`tasks/<id>/` 每个任务三件套：`prompt.md`（真实提示词）、`checks.txt`（每行一条验收命令，
在隔离 Shell 里按顺序执行）、可选 `fixture/`（先拷进工作目录的初始文件）。

```bash
cargo build --offline --manifest-path engine/Cargo.toml   # 或被评测的版本
review/eval/run.sh --timeout 600                    # 全部任务；--only ID 只跑一个
review/eval/run.sh --out /tmp/evals/x --keep        # 指定输出目录
```

每个任务在**全新工作目录**里跑一次真实模型回合（`exec --json --full-auto`），结果写到
`$OUT/<id>.jsonl`（可复跑、可 diff）。脚本最后按 JSONL 打印一张汇总表：status、exit code、
耗时、真实 tokens（`result.usage`）与验收是否通过。

报告纪律：没有实际运行的模型、命令或指标不得写进报告；模拟服务与被测对象要分开记录。
`runs/` 下按日期保存真实跑过的结果。

## 任务集

| 任务 | 考什么 |
|---|---|
| `rust-fix` | 修一个失败测试的 Rust crate（`cargo test` 验收；同时验证沙箱里工具链可用） |
| `edit-integrity` | 只改指定段落里的同名项，其它段落必须原样（验收脚本逐段断言） |
| `long-output` | 命令输出超过 200KB 预览上限，必须从完整输出里取值（考制品/分页读取路径） |

## 已知缺口

供应商矩阵（Anthropic/Gemini/…）需要对应凭据；会话级磁盘配额与跨进程文件锁仍待治理。
