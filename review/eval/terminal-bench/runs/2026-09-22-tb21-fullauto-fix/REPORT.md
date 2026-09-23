# Terminal-Bench 2.1：full_auto 修复版的六题复测

2026-09-22 实跑，2026-09-23 归档。**6/6 reward=1**，零异常、零超时。
六题在原批次均失败；这是预选失败子集，不是新的全量 89 题分数。
实现、逐题对比与局限见 [修复报告](../../../../terminal-bench-fixes-2026-09-22.md)。

配置：Harbor 0.23.0；deepseek-flash / high / 1,000,000 context tokens（来源：用户确认的 D-36）；
4 并发、每题 1 次、不重试、不增加任务超时。原生文件/Shell 工具、Leader 可动态组队。
数据集固定为与原批次相同的
`terminal-bench/terminal-bench-2-1@sha256:7d7bdc1cbedad549fc1140404bd4dc45e5fd0ea7c4186773687d177ad3a0699a`。
没有注入隐藏检查或针对具体题目的解法提示。

| 任务 | reward | CLI 状态 | CLI 时长 | 工具调用 |
|---|---:|---|---:|---:|
| configure-git-webserver | 1 | completed/0 | 103.250s | 24 |
| kv-store-grpc | 1 | completed/0 | 60.886s | 17 |
| log-summary-date-ranges | 1 | completed/0 | 33.968s | 15 |
| nginx-request-logging | 1 | completed/0 | 39.068s | 13 |
| overfull-hbox | 1 | completed/0 | 351.402s | 57 |
| pypi-server | 1 | completed/0 | 52.451s | 15 |

Harbor 整批时长 7m49s（包含容器准备和评分）。3 次失败工具回执、6 次 Shell 非零退出均保留。
输入 3,597,566 / 输出 94,301 tokens，缓存命中输入 3,342,208。未计算费用。

本次 full_auto 在普通任务容器中执行；抽查运行中容器确认
`CapAdd=null, SecurityOpt=null, Privileged=false`。与原批次额外授权的容器配置有差异。
原始 `config.json`、`result.json`、每题模型 JSONL / 摘要 / stderr / verifier 日志完整保留；
二进制和产品源码 SHA-256、基准提交见 [provenance.json](provenance.json)。

复现（先准备 Harbor、Docker 与环境变量凭据，不在命令中填写密钥）：

```bash
DATASET='terminal-bench/terminal-bench-2-1@sha256:7d7bdc1cbedad549fc1140404bd4dc45e5fd0ea7c4186773687d177ad3a0699a' \
review/eval/terminal-bench/run.sh --build -n 4 \
  -i terminal-bench/configure-git-webserver \
  -i terminal-bench/kv-store-grpc \
  -i terminal-bench/log-summary-date-ranges \
  -i terminal-bench/nginx-request-logging \
  -i terminal-bench/overfull-hbox \
  -i terminal-bench/pypi-server
```

本机 Harbor 与 compose 分别位于 `/tmp/tb21/venv/bin/harbor`、`/tmp/tb21/docker-config`。
因当前工作区外目录只读，实际启动时仅将 Harbor 缓存路径常量指向
`/tmp/tb21/home/.cache/harbor` 后调用其原 CLI；未修改任务内容、执行器、超时或评分逻辑。
容器由 Harbor 正常回收，归档时没有残留运行容器。
