# Shell 目录恢复修复：真实任务复跑（2026-09-19）

[Shell 目录恢复修复](../../../shell-cwd-2026-09-19.md)后，使用与
[首次对照](../2026-09-19-ledger-comparison/REPORT.md)相同的 `rust-ledger` fixture、
完整提示词、隐藏评分器、600 秒任务时限、DeepSeek Flash / high / 原生 **1,000,000** 上下文，
在全新工作区独立运行一次。原生长度来源为用户确认的 D-36。

沿用首次对照的隔离模型配置与环境凭据；没有复制用户 hooks、MCP、认证文件。
运行 `full-auto`，保留 TeamAgents 原生 bubblewrap 与文件工具边界。
冻结二进制、当前生产源码、任务文件和配置哈希见 [manifest](manifest.json)。
本轮产品改动仅为目录恢复逻辑、工具说明及对应回归，见[产品补丁](shell-cwd-fix.patch)。
没有再次运行 Codex 或其他竞品。

## 结果

| 项目 | 观测 |
|---|---|
| 任务状态 / 退出码 | completed / 0 |
| CLI 任务耗时 | **267.286 秒** |
| 外层 runner 耗时（含隐藏评分） | 268.167 秒 |
| 隐藏验收 | **11/11** |
| 公开及候选新增源码单测 | 3 个公开测试、26 个源码单测通过 |
| 最终候选文件保护 | 通过：受保护文件逐字节不变、无额外文件 |
| 模型调用 | 32 |
| 累计输入 / 输出 tokens | 725,633 / 65,312 |
| 累计 cached input tokens | 553,472 |
| 工具调用 | 50 |
| `read_history` | 0 |
| Shell 目录恢复拦截 | **3 次** |

本次由 Leader 调用一次私有 `run_subagent` 做验证，没有创建独立 verifier 团队成员。
子代理沿用父成员模型、窗口与预算，用量和工具事件归入 Leader。
这与此前两次 TeamAgents 实跑的自动组队方式不同。

267.3 秒低于此前 554.9 秒和 497.7 秒，高于首次 Codex 的 133.5 秒，
但模型轨迹、分工、并发条件和客户端协议不同，不能据此给出因果提速率或通用排名。
本轮确认的是：目录保护在真实任务里触发后仍可恢复，最终候选通过独立验收。

## 必须保留的失败与恢复

原始 [JSONL](teamagents.jsonl) 中：

- 第 40–41 行：`read_file` 尝试读工作区外临时副本，两次均被路径边界拒绝。
- 第 42 行：已保存的 `/tmp/verify-ledger-scratch` 消失，恢复保护跳过当前 Shell 命令，
  输出明确错误及新的起点。
- 第 43 行：随后直接读取已销毁的临时文件，命令退出 1。
- 第 45 行：模型改为在同一次 Shell 调用里复制仓库、创建验证测试并执行。
- 第 46、48 行：上一次调用结束于临时目录，下一次恢复再被保护跳过；
  第 47、49 行重试后继续，最终完成。

三条拦截回执单列于 [shell-restore-refusals.json](shell-restore-refusals.json)。
全部工具中有 **2 条 `ok=false` 回执**，另有 **4 条 Shell 非零输出**（其中 3 条是目录保护）；
两种计数不是同一种错误，Shell 非零沿用 `(exit N)` 输出合约。
没有把它们删掉或记成“零失败工具运行”。

这也说明新的工具说明没有让模型完全避免跨调用访问临时文件；
跳过当前命令后恢复根目录仍会增加重试。该限制保留，不能声称已实现持久临时工作区。
本次日志未显示此前那种恢复失败后继续相对写入的行为，但没有逐时刻文件系统审计；
文件保护结论仍仅针对最终候选。

## 证据与复跑

- [原始 JSONL](teamagents.jsonl)、[模型 stderr](teamagents.stderr)、
  [runner 日志](runner.log)、[driver stderr](driver.stderr)、[退出记录](exit.json)。
- [完整指标](metrics.json)、[独立评分](grade.json)、[评分日志](grade.log)、
  [候选补丁](candidate.patch)。
- [修复前失败回归](regression-before.log)、[修复后沙箱回归](regression-after.log)、
  [全量检查](make-check.log)。

隔离配置中仅保留选定模型的供应商/协议/凭据环境变量引用、120 秒请求超时、5 次重试，
并设置 `context_window=1000000`、`generation_options.reasoning_effort="high"`。

```bash
XDG_CONFIG_HOME=/path/to/isolated/config \
review/eval/run.sh --bin /path/to/frozen/teamagents \
  --only rust-ledger --timeout 600 --out /tmp/new-shell-cwd-run
```

模型结束后 runner 自动调用独立隐藏评分器；隐藏测试没有复制进模型工作区。
本次结果归档前检查实际供应商凭据未进入证据，验证冻结二进制/源码哈希、
与前轮一致的任务哈希以及最终进程状态。没有新增真实供应商或大仓库覆盖。
