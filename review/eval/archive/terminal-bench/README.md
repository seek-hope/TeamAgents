# Terminal-Bench 2.1 × TeamAgents

用 Harbor（Terminal-Bench 的官方 harness）把 TeamAgents 当作被测 agent，跑
`terminal-bench/terminal-bench-2-1@latest`（89 个任务）。每个任务在独立容器内执行，
Harbor 负责环境、超时、验证与打分；TeamAgents 的 Leader/成员、shell/文件工具和批准流程
全部发生在该容器内部，测的是产品本身，而不是把任务当成本仓库的评测任务重写一遍。

## 组成

| 文件 | 作用 |
| --- | --- |
| `teamagents_agent.py` | Harbor 适配器：上传静态二进制、准备容器内配置、驱动 `teamagents exec --json`、把 JSONL 用量写回 Harbor 的 `AgentContext` |
| `teamagents-caps.yaml` | Docker Compose overlay：历史版 bwrap 复现用的可选 overlay，当前 full_auto 不需要 |
| `run.sh` | 一行调用：数据集/模型/适配器/overlay/jobs 目录，其余参数透传给 `harbor run` |
| `status.sh` / `summarize.py` | 查看运行进度；汇总 job 目录并列出需要重跑的基础设施失败 |
| `check_adapter.py` | 无模型、无 Docker 的真实管道回归：退出码 0/1/3/124 必须原样写入摘要（需要 Harbor Python 环境） |
| `jobs/` | 评测输出（job 级 `result.json` + 每个 trial 的日志），默认落在这里 |
| `runs/<日期>-<job>/` | 已跑批次的留存（原始 JSONL + 逐 trial 结果 + 报告） |

## 已跑批次

- 2026-09-22 `runs/2026-09-22-tb21-teamagents-flash/REPORT.md`：
  deepseek-flash（high / 1M），TB 2.1 全量 89 任务，**53/89 = 59.6%**，逐任务表与失败分析见报告。
- 针对该批次的实现修复和子集复测见 [修复记录](../../terminal-bench-fixes-2026-09-22.md)。

## 前置条件

1. **Docker 守护进程**：`sudo systemctl start docker.socket docker.service`（本机默认未启动）。
2. **Docker Compose 插件**：本机 `docker` 没有内置 `compose`/`buildx`，Harbor 会调用
   `docker compose`。把 Compose v2 二进制放到 CLI 插件目录即可，无需 root：
   `mkdir -p $DOCKER_CONFIG/cli-plugins && curl -L -o $DOCKER_CONFIG/cli-plugins/docker-compose https://github.com/docker/compose/releases/latest/download/docker-compose-linux-x86_64 && chmod +x $_`
3. **Harbor**：`uv tool install harbor`（本次使用 0.23.0）。
4. **静态二进制**：任务镜像是 Ubuntu 系，宿主 glibc 版本更高，必须用 musl 静态构建：

   ```bash
   rustup target add x86_64-unknown-linux-musl
   RUSTFLAGS="-C target-feature=+crt-static -C relocation-model=static" \
     CC_x86_64_unknown_linux_musl=musl-gcc \
     CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
     cargo build --offline --release --manifest-path engine/Cargo.toml --bin teamagents \
       --target x86_64-unknown-linux-musl
   ```

   `-C relocation-model=static` 是必需的：只加 `+crt-static` 时 rustc 仍产出带
   `/lib/ld-musl-x86_64.so.1` 解释器的 static-pie，在容器里会报 `no such file or directory`。
5. **凭据**：`DEEPSEEK_API_KEY` 在被测容器内以同名环境变量注入，容器内配置用
   `api_key_env = "DEEPSEEK_API_KEY"` 引用，密钥不落盘到仓库。

## 运行

```bash
# 单任务冒烟
review/eval/terminal-bench/run.sh -i 'terminal-bench/fix-git' -n 1

# 全量 89 任务，4 并发
review/eval/terminal-bench/run.sh -n 4
```

适配器参数（`--ak key=value`）：`binary_path`、`workdir`（默认 `/app`）、
`reasoning_effort`（默认 `high`）、`context_window`（默认 1000000，D-36）、
`inner_timeout_sec`（`teamagents exec --timeout`，默认不传 = harness 的 1200s）、
`install_bubblewrap`（当前默认 false）、`team_spec`。

**长跑注意**：`harbor run` 必须脱离当前 shell 存活（例如 `systemd-run` 瞬时单元），
否则进程被回收时正在跑的 trial 会留下容器与网络，需要手动 `docker rm -f` /
`docker network rm` 清理。

## 结果解读

- Harbor 汇总：`jobs/<job-name>/<时间戳>/result.json`（reward、token、耗时、异常）。
- 单任务：`jobs/.../<task>__<id>/` 下的 `result.json`（reward）、`verifier/`（测试输出）、
  `agent/teamagents.jsonl`（逐行 JSON：session/tool/event/result）、
  `agent/teamagents.summary.json`（状态、退出码、逐模型 token）、
  `agent/teamagents.stderr.log`。

## 已知取舍（与本仓库基准的差异）

- **执行模式**：按 D-41，当前 `--full-auto` 的 Shell 直接在任务容器内执行，
  不安装 bubblewrap，不添加 `SYS_ADMIN` 或 `seccomp=unconfined`。容器本身仍由 Harbor 管理。
  历史批次使用了额外权限；不能把修复后的子集复测拼成新的全量分数。
- **旧版复现**：使用历史二进制并设置 `SANDBOX_OVERLAY=1`，会启用原 overlay 和
  `install_bubblewrap=true`。历史报告与原始日志保持不变。
- **超时**：默认沿用任务自带超时（中位数 900s），不加倍；harness 内部 `exec --timeout`
  默认 1200s，因此长任务经常由 Harbor 侧先判超时——这是 harness 的真实表现，不做掩盖。
- **模型档位**：`reasoning_effort = high`、原生 1M 上下文（沿用 `live-models.example.toml`
  的评测约定；宿主 `config.toml` 默认档是 `max`）。
- **工具集**：容器内只写模型 profile，不给 `fetch`/`web`/`remote_ssh`（宿主专有的 MCP
  与密钥不进入容器），即只测原生 shell/文件工具与多智能体协作。
