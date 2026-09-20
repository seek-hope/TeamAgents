# Chat 真实模型冒烟证据（2026-09-19）

本批实现与边界见[记录](../../../live-models-2026-09-19.md)，入口见
`engine/tests/live_models.rs::live_chat_model_matrix`。实际供应商只有 DeepSeek，
未对其他四家、混合供应商团队或竞品进行测试。

两次实跑使用同一完整 profile：DeepSeek Flash / deepseek / 原生 **1,000,000** /
high / 请求超时 **120 秒** / 重试 **5 次**，每阶段时限 **600 秒**。
原生长度来源为用户确认的 D-36。`selected.config.toml` 仅保存所选模型设置及密钥环境变量名，
没有密钥值；不导入原用户配置的其他内容。

| 运行 | 入口版本 | 三阶段 | 耗时 | 请求 | 输入 / 输出 / 合计 token | 成功 / 失败工具 |
|---|---|---|---:|---:|---|---|
| `deepseek-1` | 首次入口 | 全部通过 | 12.828 秒 | 11 | 54,314 / 1,583 / 55,897 | 7 / 1 |
| `deepseek-2` | 加入空目录断言的最终入口 | 全部通过 | 14.933 秒 | 14 | 69,301 / 1,567 / 70,868 | 11 / 0 |

三阶段是文件工具及工具结果续接、同会话 Shell、关闭并销毁后重建相同会话。
驱动核对真实文件和回合终态；两次的初始输入不变、用量恢复一致性均通过。
首跑有一次无效 `complete_task`，保留在报告的失败工具计数中，不改成零错误运行。
复核后新增续接前工作目录为空的断言，拒绝留下另一份输入文件的情况；最终实跑两处断言均通过。
`initial-source-manifest.json` / `final-source-manifest.json` 区分源码与测试二进制；
`history-input-check.patch` 记录中间增补。两次入口不同，不合并为统计完成率。

`missing-key-final` 是无真实请求的入口反例：输出 `incomplete` / `skipped`，Cargo 退出 **101**。
两个真实请求命令均退出 **0**。模型未列入清单不等于通过；本地服务的五项契约也不算真实供应商成绩。

最终 `make check`：core **84**、engine **312**（**3 ignored**）、TUI **104**，格式、
Clippy 与仓库卫生检查通过。`check-runner.sh` **30** 项通过；完整日志在 `validation/`。
默认 Cargo 的 ignored 数包含本入口一项，真实请求必须显式启用。

在仓库根目录复跑（需要本机 `DEEPSEEK_API_KEY` 与可用的 bubblewrap；证据目录须未存在）：

```bash
TEAMAGENTS_LIVE_MODELS_CONFIG="$PWD/review/eval/runs/2026-09-19-live-models/selected.config.toml" \
TEAMAGENTS_LIVE_MODELS_MANIFEST="$PWD/review/eval/runs/2026-09-19-live-models/selection.toml" \
TEAMAGENTS_LIVE_MODELS_EVIDENCE="/tmp/teamagents-live-models-recheck" \
cargo test --offline --locked --manifest-path engine/Cargo.toml --test live_models \
  live_chat_model_matrix -- --ignored --exact --nocapture
```

报告保存计数和行为断言，不包含原始模型文本、任意工具参数或供应商错误正文。
临时输入/输出与完整会话状态在测试退出后清理；归档前检查当前环境中的凭据值没有混入。
清单与配置来源可以复现设置，但工具轨迹与模型输出本身具有随机性。
这不覆盖进程崩溃中恢复、完整批准/取消矩阵、远端工具任务、L2 压缩或仓库复杂任务完成率。
此前仓库任务 **0/3** 成绩保持不变，长期竞品目标仍未完成。
