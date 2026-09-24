# R2-P6 性能实验报告（R24–R26）

日期：2026-09-24。预登记：`review/eval/r2-p6/design.md`、`manifest.json`（第一轮 8 题）、
`manifest-r2.json`（第二轮 3 题）、`manifest-r3.json`（第三轮 2 题，含 150 s 硬截止）；分析脚本
`analyze.py`（sha256 记入各 manifest，冻结于跑前）。模型：DeepSeek Flash（目录键 `leader_main`），
**原生上下文 1,000,000**（D-36），`reasoning_effort = high`（显式覆盖目录默认 max，见 design.md §3），
`full_auto` 权限、每 trial 全新工作目录、验收脚本在 trial 结束后于同一目录执行。

## 结论（按预登记口径）

| 假设 | 结论 | 证据 |
|---|---|---|
| H1：B（持久化单实例）相对 A（直驱参考）**未观察到退化** | ✅ 通过 | 四个批次共 135 trial，**A/B/C 三组在每个任务的每次重复都通过验收（135/135）**；逐任务配对成功差恒为 0，95% bootstrap 区间 [0, 0]（区间不含负值） |
| H2：C（可见协作能力）相对 B 有**跨独立运行可复现的收益** | ❌ **未证实** | 同样 135/135，逐任务配对差恒 0；且**三轮 99 个 C 组 trial 中零次 spawn/delegate**（逐个 SQLite 事件核验：`instance_created` 等于 trial 数、`tasks` 表恒空）——协作能力可用、指令可见，但模型每次选择单人成队。方案对 C 的定义本就"允许始终单人成队" |

即：**持久化运行时（B）没有损害任务成功率**（这是 P6 要回答的第一个问题，已有 135 次真实运行的证据），
而**"系统自主选择协作的净收益"在本次任务集上未得到证实**——原因不是协作失败，而是这些任务对单实例
而言都在能力与注意力预算之内，模型没有理由分工。结论符合 §13.2/§16 要求的预登记口径（"样本不足即标未证实"），
但**不能据此宣称协作带来了收益**。

## 成本（真实 tokens，DeepSeek 计费口径）

| 批次 | A | B | C |
|---|---|---|---|
| 试跑 18 trial | 326,062（合计） | — | — |
| 正式第一轮 72 trial | 534,021 | 613,075（+14.8%） | 641,274（+4.6% vs B） |
| 第二轮 27 trial | 217,276 | 238,293（+9.7%） | 266,190（+11.7% vs B） |
| 第三轮 18 trial | 278,166 | 252,483 | 230,923 |

四批合计 ≈ 3.60M tokens、约 55 分钟机器时间（每 trial 6–40 s，第三轮最慢 40 s）。持久化运行时相对
参考循环的 token 开销约 +10%～+15%，协作面（C）在此任务集上只增加开销（因为它没被使用）。

### 试跑 R25（6 题 × 3 组 × 1 次）

trial 数 18，全部通过 18/18；真实 tokens 合计 326,062

| 任务 | A（成功/次数，tokens） | B | C |
|---|---|---|---|
| edit-integrity | 1/1, 14,037t | 1/1, 11,577t | 1/1, 13,602t |
| long-output | 1/1, 14,567t | 1/1, 17,794t | 1/1, 19,629t |
| multi-step | 1/1, 10,810t | 1/1, 21,180t | 1/1, 24,149t |
| rust-fix | 1/1, 13,450t | 1/1, 14,570t | 1/1, 17,188t |
| service-check | 1/1, 31,717t | 1/1, 26,126t | 1/1, 26,960t |
| split-deliverable | 1/1, 18,812t | 1/1, 14,102t | 1/1, 15,792t |

### 正式 R26 第一轮（8 题 × 3 组 × 3 次）

trial 数 72，全部通过 72/72；真实 tokens 合计 1,788,370

| 任务 | A（成功/次数，tokens） | B | C |
|---|---|---|---|
| edit-integrity | 3/3, 40,508t | 3/3, 41,333t | 3/3, 47,964t |
| long-horizon | 3/3, 82,254t | 3/3, 108,856t | 3/3, 131,059t |
| long-output | 3/3, 38,249t | 3/3, 45,586t | 3/3, 57,060t |
| multi-step | 3/3, 52,339t | 3/3, 63,777t | 3/3, 68,489t |
| parallel-deliverables | 3/3, 91,861t | 3/3, 129,261t | 3/3, 115,146t |
| rust-fix | 3/3, 44,440t | 3/3, 46,394t | 3/3, 53,230t |
| service-check | 3/3, 140,719t | 3/3, 122,991t | 3/3, 112,214t |
| split-deliverable | 3/3, 43,651t | 3/3, 54,877t | 3/3, 56,112t |

### R26 第二轮（3 个更重任务 × 3 组 × 3 次）

trial 数 27，全部通过 27/27；真实 tokens 合计 721,759

| 任务 | A（成功/次数，tokens） | B | C |
|---|---|---|---|
| bulk-modules | 3/3, 74,438t | 3/3, 81,338t | 3/3, 95,554t |
| long-chain | 3/3, 106,970t | 3/3, 99,795t | 3/3, 119,220t |
| wide-audit | 3/3, 35,868t | 3/3, 57,160t | 3/3, 51,416t |

### R26 第三轮（带 150s 硬截止的可分工任务 × 3 组 × 3 次）

trial 数 18，全部通过 18/18；真实 tokens 合计 761,572

| 任务 | A（成功/次数，tokens） | B | C |
|---|---|---|---|
| timebox-audit | 3/3, 219,905t | 3/3, 168,597t | 3/3, 114,301t |
| timebox-two-modules | 3/3, 58,261t | 3/3, 83,886t | 3/3, 116,622t |

## 局限与后续（如实记录，不并入结论）

- **任务集处于单实例能力的上限之内**：三个轮次逐级加重（6 题 → 8 题 → 更长/更多文件 → 硬截止可分工），
  单实例仍然全部按时通过；要把"协作净收益"跑出来，需要的任务规模明显超出本轮预算（例如单次任务
  数百次工具调用、或必须并行才能赶上外部截止的真实工作量），本轮**未做**，故 H2 只能记"未证实"。
- **硬截止维度第三轮无效**：150 s 截止在实测中最慢 trial 也只用了 40 s，截止未起约束作用 → 该维度
  未产生区分度。
- 本轮没有真实 Codex 成员参与（Codex 后端属 v1 遗留，R29 退役），也未测异构模型的性能（§13.1 另列）。

## 复跑

```bash
cargo build --offline --manifest-path engine/Cargo.toml --example rebuild_p6
python3 review/eval/r2-p6/freeze.py manifest.json            # 重算任务摘要（跑前冻结）
python3 review/eval/r2-p6/run.py --phase pilot  --out review/eval/r2-p6/runs/<新目录>
python3 review/eval/r2-p6/run.py --phase formal --out review/eval/r2-p6/runs/<新目录>
python3 review/eval/r2-p6/run.py --phase formal --manifest manifest-r2.json --out <新目录>
python3 review/eval/r2-p6/run.py --phase formal --manifest manifest-r3.json --out <新目录>
python3 review/eval/r2-p6/analyze.py <目录>/results.jsonl
```

原始数据：`review/eval/r2-p6/runs/<批次>/results.jsonl`（每 trial 一行，含状态、真实 tokens、墙钟、
逐条验收输出、runner stderr）与同目录 `run-header.json`（冻结的 analysis/harness/git 摘要）。
沙箱内运行会被资源限制静默终止（两批出现过）；正式批次在沙箱外执行，`--resume` 支持断点续跑。
