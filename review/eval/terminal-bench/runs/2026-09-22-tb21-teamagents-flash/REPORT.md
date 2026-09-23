# Terminal-Bench 2.1 × TeamAgents（deepseek-flash）实跑报告

日期：2026-09-22。数据：本目录 `run1/`（89 任务全量）与 `repair/`（13 个基础设施失败任务重跑），
原始 JSONL、逐 trial `result.json`、harness 摘要都随本报告保存。

## 1. 结论

| 指标 | 数值 |
| --- | --- |
| 任务数 | 89（`terminal-bench/terminal-bench-2-1@latest`） |
| 通过（reward = 1.0） | **53 / 89 = 59.6%** |
| 平均 reward | **0.5955**（89 个任务全部测得有效结果后） |
| Agent 超时（任务默认 900s） | 10 个 trial（其中 3 个仍在验证阶段通过） |
| "自称完成但验证失败" | 16 个 |
| 工具调用总数 | 6,456 次（每任务中位数 54） |
| trial 时长中位数 | 390s |
| 模型用量（89+13 trial） | 输入 128,382,789 / 输出 4,495,955 / 缓存命中 123,670,400 tokens |

首轮 89 个 trial 里有 13 个是基础设施失败（9 个 docker.io 镜像拉取 EOF、4 个容器内装包 404/超时），
按 Harbor 口径会把它们记 0（首轮指标 0.4944）。修复适配器的装包回退后重跑这 13 个任务，
9 个通过（0.6923），上表是合并后的结果。

## 2. 运行配置

| 项 | 值 |
| --- | --- |
| Harness | Harbor 0.23.0（Terminal-Bench 官方 harness），数据集 `terminal-bench/terminal-bench-2-1@latest`，89 任务 |
| 被测 agent | TeamAgents 0.1.2，`teamagents exec --json --full-auto`（leader + 动态成员） |
| 模型 | `deepseek/deepseek-flash`，`reasoning_effort = high`，原生 1M 上下文（D-36） |
| 容器 | Ubuntu 系任务镜像；`--extra-docker-compose teamagents-caps.yaml`（`SYS_ADMIN` + `seccomp=unconfined`，bwrap 需要） |
| 并发 / 重试 | `-n 4`；首轮不重试，修复轮 `--max-retries 2 --retry-exclude AgentTimeoutError` |
| 超时 | 任务自带（agent 中位数 900s），不加倍；harness 内部 `exec --timeout` 默认 1200s |
| 容器内工具集 | 仅原生 shell/文件工具（`fetch`/`web`/`remote_ssh` 依赖宿主密钥与路径，不注入容器） |

命令（完整步骤见上级 [README](../README.md)）：

```bash
PYTHONPATH=review/eval/terminal-bench harbor run \
  -d terminal-bench/terminal-bench-2-1@latest -m deepseek/deepseek-flash \
  -a teamagents_agent:TeamAgentsAgent \
  --ak binary_path=engine/target/x86_64-unknown-linux-musl/release/teamagents \
  --extra-docker-compose review/eval/terminal-bench/teamagents-caps.yaml \
  -o /tmp/tb21/jobs -n 4 --job-name tb21-teamagents-flash -q -y
```

## 3. 逐任务结果

`outcome` 列：`ok` = 正常完成验证；`agent-timeout` = Harbor 在任务超时后中断（reward 仍可能为 1.0）；
`infra:*` = 基础设施失败（本表中已被 `repair` 行替换）。`in tok`/`out tok` 来自 harness 的
`result.usage`（含缓存命中，缓存命中量见下节）。

| task | reward | outcome | source | agent status | tools | duration | in tok | out tok |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| bn-fit-modify | 1.00 | ok | run1 | completed | 110 | 856s | 2,692,277 | 74,877 |
| break-filter-js-from-html | 1.00 | ok | run1 | completed | 45 | 222s | 625,320 | 28,671 |
| build-pmars | 1.00 | ok | run1 | completed | 68 | 487s | 3,719,936 | 83,377 |
| build-pov-ray | 1.00 | ok | repair | completed | 62 | 291s | 2,710,490 | 36,824 |
| cancel-async-tasks | 1.00 | ok | run1 | completed | 119 | 867s | 5,907,894 | 162,577 |
| circuit-fibsqrt | 1.00 | ok | repair | completed | 56 | 756s | 3,396,448 | 78,717 |
| cobol-modernization | 1.00 | agent-timeout | repair | - | 158 | - | 0 | 0 |
| code-from-image | 1.00 | ok | run1 | completed | 28 | 45s | 103,464 | 7,083 |
| constraints-scheduling | 1.00 | ok | run1 | completed | 11 | 27s | 58,696 | 6,025 |
| count-dataset-tokens | 1.00 | ok | run1 | completed | 29 | 171s | 326,552 | 9,730 |
| crack-7z-hash | 1.00 | ok | run1 | completed | 51 | 329s | 1,791,618 | 49,426 |
| custom-memory-heap-crash | 1.00 | ok | run1 | completed | 73 | 356s | 2,260,531 | 63,145 |
| db-wal-recovery | 1.00 | ok | run1 | completed | 28 | 69s | 170,568 | 10,512 |
| distribution-search | 1.00 | ok | run1 | completed | 21 | 100s | 337,803 | 18,074 |
| extract-elf | 1.00 | ok | run1 | completed | 51 | 250s | 808,074 | 44,957 |
| feal-differential-cryptanalysis | 1.00 | ok | run1 | completed | 42 | 882s | 1,412,532 | 62,633 |
| feal-linear-cryptanalysis | 1.00 | ok | run1 | completed | 29 | 346s | 997,399 | 51,312 |
| financial-document-processor | 1.00 | ok | run1 | completed | 92 | 499s | 1,654,048 | 72,686 |
| fix-code-vulnerability | 1.00 | ok | repair | completed | 54 | 128s | 1,176,270 | 24,545 |
| fix-git | 1.00 | ok | run1 | completed | 44 | 112s | 427,000 | 21,737 |
| fix-ocaml-gc | 1.00 | ok | run1 | completed | 85 | 793s | 2,933,006 | 50,626 |
| git-leak-recovery | 1.00 | ok | run1 | completed | 54 | 105s | 358,514 | 21,391 |
| headless-terminal | 1.00 | ok | run1 | completed | 56 | 419s | 1,235,201 | 69,487 |
| large-scale-text-editing | 1.00 | agent-timeout | run1 | timeout | 97 | 1200s | 2,214,452 | 77,671 |
| largest-eigenval | 1.00 | ok | run1 | completed | 109 | 883s | 7,265,022 | 128,961 |
| llm-inference-batching-scheduler | 1.00 | ok | repair | completed | 50 | 504s | 2,359,480 | 102,409 |
| make-doom-for-mips | 1.00 | agent-timeout | run1 | - | 108 | - | 0 | 0 |
| make-mips-interpreter | 1.00 | ok | run1 | completed | 116 | 886s | 9,107,353 | 110,482 |
| merge-diff-arc-agi-task | 1.00 | ok | repair | completed | 68 | 413s | 1,778,556 | 61,182 |
| model-extraction-relu-logits | 1.00 | ok | run1 | completed | 24 | 257s | 346,513 | 22,933 |
| modernize-scientific-stack | 1.00 | ok | run1 | completed | 63 | 130s | 479,628 | 27,205 |
| mteb-leaderboard | 1.00 | ok | repair | completed | 86 | 850s | 4,762,095 | 50,357 |
| mteb-retrieve | 1.00 | ok | run1 | completed | 42 | 267s | 532,747 | 20,124 |
| openssl-selfsigned-cert | 1.00 | ok | run1 | completed | 24 | 42s | 132,564 | 7,740 |
| password-recovery | 1.00 | ok | run1 | completed | 60 | 146s | 969,287 | 27,460 |
| path-tracing | 1.00 | ok | run1 | completed | 41 | 215s | 1,724,677 | 42,133 |
| path-tracing-reverse | 1.00 | ok | run1 | completed | 41 | 371s | 1,723,241 | 89,749 |
| polyglot-c-py | 1.00 | ok | run1 | completed | 43 | 226s | 812,022 | 38,234 |
| polyglot-rust-c | 1.00 | ok | run1 | completed | 17 | 644s | 515,477 | 33,535 |
| portfolio-optimization | 1.00 | ok | run1 | completed | 72 | 495s | 631,689 | 32,821 |
| protein-assembly | 1.00 | ok | run1 | completed | 91 | 591s | 2,399,237 | 101,661 |
| pytorch-model-recovery | 1.00 | ok | repair | completed | 40 | 135s | 371,119 | 23,689 |
| regex-log | 1.00 | ok | repair | completed | 41 | 190s | 859,511 | 40,553 |
| reshard-c4-data | 1.00 | ok | run1 | completed | 37 | 702s | 1,210,484 | 35,809 |
| rstan-to-pystan | 1.00 | ok | run1 | timeout | 75 | 1200s | 1,722,007 | 66,267 |
| sam-cell-seg | 1.00 | ok | run1 | completed | 89 | 763s | 2,772,119 | 190,053 |
| schemelike-metacircular-eval | 1.00 | ok | run1 | failed | 67 | 1200s | 1,707,168 | 83,596 |
| sparql-university | 1.00 | ok | run1 | completed | 25 | 120s | 254,446 | 27,435 |
| sqlite-db-truncate | 1.00 | ok | run1 | completed | 35 | 105s | 289,572 | 20,655 |
| torch-tensor-parallelism | 1.00 | ok | run1 | completed | 24 | 196s | 350,038 | 39,873 |
| video-processing | 1.00 | ok | run1 | completed | 63 | 704s | 2,665,894 | 116,934 |
| vulnerable-secret | 1.00 | ok | run1 | completed | 35 | 83s | 259,693 | 10,885 |
| write-compressor | 1.00 | ok | run1 | completed | 29 | 256s | 701,219 | 49,734 |
| adaptive-rejection-sampler | 0.00 | agent-timeout | run1 | - | 35 | - | 0 | 0 |
| build-cython-ext | 0.00 | agent-timeout | run1 | - | 124 | - | 0 | 0 |
| caffe-cifar-10 | 0.00 | ok | repair | failed | 42 | 1200s | 1,420,753 | 52,869 |
| chess-best-move | 0.00 | ok | run1 | completed | 15 | 98s | 109,425 | 19,940 |
| compile-compcert | 0.00 | ok | run1 | completed | 126 | 1090s | 4,802,325 | 129,433 |
| configure-git-webserver | 0.00 | ok | run1 | completed | 180 | 880s | 3,721,499 | 126,517 |
| dna-assembly | 0.00 | ok | run1 | completed | 72 | 692s | 3,017,304 | 125,542 |
| dna-insert | 0.00 | ok | run1 | completed | 55 | 393s | 1,097,766 | 78,034 |
| extract-moves-from-video | 0.00 | ok | run1 | timeout | 51 | 1200s | 575,670 | 43,980 |
| filter-js-from-html | 0.00 | ok | run1 | failed | 75 | 1200s | 3,691,723 | 175,492 |
| gcode-to-text | 0.00 | agent-timeout | run1 | - | 118 | - | 0 | 0 |
| git-multibranch | 0.00 | agent-timeout | run1 | - | 157 | - | 0 | 0 |
| gpt2-codegolf | 0.00 | agent-timeout | run1 | - | 51 | - | 0 | 0 |
| hf-model-inference | 0.00 | ok | run1 | completed | 43 | 415s | 749,541 | 45,874 |
| install-windows-3.11 | 0.00 | ok | run1 | timeout | 78 | 1200s | 3,632,252 | 99,594 |
| kv-store-grpc | 0.00 | ok | run1 | completed | 45 | 345s | 1,279,420 | 50,687 |
| log-summary-date-ranges | 0.00 | agent-timeout | run1 | - | 1183 | - | 0 | 0 |
| mailman | 0.00 | ok | run1 | failed | 47 | 955s | 1,618,991 | 60,783 |
| mcmc-sampling-stan | 0.00 | ok | repair | timeout | 57 | 1200s | 854,439 | 30,963 |
| multi-source-data-merger | 0.00 | ok | run1 | completed | 34 | 119s | 471,544 | 25,166 |
| nginx-request-logging | 0.00 | ok | run1 | completed | 63 | 390s | 1,251,688 | 67,748 |
| overfull-hbox | 0.00 | ok | run1 | completed | 51 | 294s | 1,150,094 | 47,900 |
| prove-plus-comm | 0.00 | ok | run1 | - | 0 | - | 0 | 0 |
| pypi-server | 0.00 | ok | run1 | completed | 73 | 262s | 965,280 | 42,655 |
| pytorch-model-cli | 0.00 | ok | run1 | completed | 30 | 222s | 498,203 | 23,926 |
| qemu-alpine-ssh | 0.00 | ok | repair | incomplete | 56 | 183s | 306,764 | 36,946 |
| qemu-startup | 0.00 | ok | repair | failed | 56 | 215s | 653,261 | 42,237 |
| query-optimize | 0.00 | ok | run1 | completed | 25 | 662s | 191,931 | 8,897 |
| raman-fitting | 0.00 | ok | run1 | completed | 52 | 628s | 2,283,106 | 126,882 |
| regex-chess | 0.00 | ok | run1 | failed | 15 | 789s | 380,873 | 60,285 |
| sanitize-git-repo | 0.00 | ok | run1 | completed | 100 | 352s | 3,969,804 | 64,881 |
| sqlite-with-gcov | 0.00 | ok | run1 | completed | 100 | 529s | 2,445,057 | 99,646 |
| torch-pipeline-parallelism | 0.00 | agent-timeout | run1 | - | 54 | - | 0 | 0 |
| train-fasttext | 0.00 | ok | run1 | timeout | 62 | 1200s | 537,312 | 23,178 |
| tune-mjcf | 0.00 | ok | run1 | failed | 20 | 378s | 242,430 | 23,626 |
| winning-avg-corewars | 0.00 | ok | run1 | timeout | 33 | 1200s | 443,383 | 33,722 |
## 4. 失败与行为分析

### 4.1 后台服务类任务结构性失败（harness 侧最重要的一条）

`shell` 工具的每次调用都跑在独立的 bwrap PID namespace 里，并带 `--die-with-parent`，
命令返回后同一次调用里起的进程全部被杀；且 bwrap 给每次调用 `--tmpfs /tmp`。
因此"起一个常驻服务，之后的调用/验证脚本再连它"的任务在 TeamAgents 下无法完成。

证据：

- 89 个任务里 15 个的指令提到 server/daemon/listen/port；其中 12 个有有效结果，**8 个失败**
  （`configure-git-webserver`、`git-multibranch`、`hf-model-inference`、`install-windows-3.11`、
  `kv-store-grpc`、`mailman`、`nginx-request-logging`、`pypi-server`），通过的 4 个都不依赖常驻进程。
- 8 个 trial 的模型日志里直接出现 `die-with-parent`：模型自己复现并记录了这条限制。
  `kv-store-grpc` 的 Leader 结论原文：
  "This runtime runs every shell command in a fresh `bwrap` PID namespace with `--die-with-parent`,
  so a normal `nohup … &` server cannot outlive a command — I confirmed this empirically."
- 验证侧证据：`configure-git-webserver` 的 verifier 输出
  `❌ TEST FAILED: Web server returned HTTP 000`；`pypi-server` 的
  `pip install --index-url http://localhost:8080/simple` 直接失败。

这不是模型能力问题，而是当前 harness 的隔离模型与"常驻服务"类任务不兼容；要提升这一类，
需要给 `shell` 引入可跨调用存活的后台进程（例如持久化 shell 之外的长驻服务句柄）。

### 4.2 自称完成但验证失败（16 个）

16 个 trial 里模型以 `run_completed(status=COMPLETED)` 收尾，但验证判 0。典型：
`overfull-hbox` 的 4 项检查过 3 项，只有 `test_no_overfull_hboxes` 失败——
模型替换了同义词并让文档编译通过，却没有真正消除 overfull hbox 警告。
说明 Leader 的"完成"判定偏乐观，缺少任务侧验收标准的对照。

### 4.3 超时（10 个 trial）

任务默认 900s；其中 `log-summary-date-ranges` 单次 trial 跑了 1183 次工具调用仍失败。
3 个超时 trial 的产物在验证阶段仍通过（`cobol-modernization`、`large-scale-text-editing`、
`make-doom-for-mips`），说明超时主要来自"多智能体 + 高推理档位"的回合开销，而不是工作没做完。
本轮未使用 `--agent-timeout-multiplier`，保持 TB 默认超时口径。

### 4.4 基础设施失败（首轮 13 个，修复轮 0 个）

- 9 个 `RuntimeError`：`docker compose` 拉镜像时 `registry-1.docker.io` 返回 EOF（网络抖动）。
  之后把 89 个镜像全部预拉取到本地，修复轮不再出现。
- 4 个装包失败：容器内 `apt-get install bubblewrap` 命中已下架的 security 池（bullseye 的
  `bubblewrap_0.4.1-3+deb11u1` 404）或瞬时网络失败。适配器改为
  "普通安装 → `--fix-missing` → 按 `VERSION_CODENAME` 回退到发行版主仓" 后，
  修复轮 13 个任务 0 基础设施失败。

## 5. 用量与成本

| 项 | 首轮（89 trial） | 修复轮（13 trial） |
| --- | --- | --- |
| 输入 tokens | 107,733,603 | 20,649,186 |
| 输出 tokens | 3,914,664 | 581,291 |
| 缓存命中输入 | 103,525,120 | — |

合计输入 128.4M / 输出 4.5M，缓存命中率约 96%（DeepSeek 前缀缓存生效）。
上表不含 10 个被 Harbor 中断的 trial（harness 的 `result` 行来不及写出，用量无法归集；
适配器已改为在取消时尽力回填，下一轮可覆盖）。Harbor 未配置价格表，`cost_usd` 为空，
本报告只记录真实 token 数。

## 6. 结论与后续

- TeamAgents + deepseek-flash（high / 1M）在 TB 2.1 全量 89 任务上 **53/89 = 59.6%**；
  修复基础设施噪声后不需要再人工剔除任务。
- 当前最大结构性短板是"需要常驻服务"的任务族（8 个失败）与"自称完成"的判定（16 个），
  两者都比模型推理能力更靠近 harness 设计。
- 可选后续：`--agent-timeout-multiplier 2` 复跑 7 个真超时任务；给 `shell` 增加跨调用存活的
  后台服务；对失败任务族开 `-k 2` 观察方差；把 `fetch`/`web` 工具作为可选注入做消融。

