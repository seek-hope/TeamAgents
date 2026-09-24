# 评测

## 当前入口：固定任务 A/B/C 对照（真实模型）

```bash
python3 review/eval/r2-p6/run.py --phase pilot  --out review/eval/r2-p6/runs/<新日期>
python3 review/eval/r2-p6/run.py --phase formal --out review/eval/r2-p6/runs/<新日期>
```

- 三组：A = 直驱参考循环（`engine/examples/eval_group_a.rs`）、B = 持久化单实例
  （`engine/examples/eval_group_b.rs`）、C = B + 协作面；统一的模型目录键、权限与超时见
  [`r2-p6/design.md`](r2-p6/design.md)（跑前冻结的预登记）。
- 每个 trial 使用全新工作目录与状态目录，结果写 `runs/<日期>/results.jsonl`，逐 trial 的会话库与产物
  留在 `runs/<日期>/{state,work}/`；这些目录里的编译缓存与 SQLite 临时文件不入库
  （`.gitignore` + `make hygiene` 把关）。
- 模型必须使用原生上下文长度并记录数值与来源；结论只按预登记口径给出，样本不足写"未证实"。
- 复跑命令、成本与限制见各次 `runs/<日期>/REPORT.md`；最近一次见
  [`r2-p6/REPORT.md`](r2-p6/REPORT.md)。

## 评测纪律

- 没有实际运行的模型、命令或指标不得写进报告；假服务与被测对象分开记录。
- 判分在 trial 自己的工作目录里按 `checks.txt` 逐条执行，失败必须分类记录，不挑拣重跑、不择优。
- 证据不自动清理；运行期间不修改输入、判分或候选，不因换存储位置把历史失败改记成功。

## 历史

更早实现（≤ v0.1.2）的固定任务运行器、任务集与其评分器（`run.sh`、`tasks/`、`check-runner.sh`、
`live-models.example.toml`、`terminal-bench/`）已移入 [`archive/`](archive/)；它们的原始结果仍在
[`runs/`](runs/) 中按日期保留。
