# Terminal-Bench 2.1 暴露的 Shell 执行问题：修复与复测

实施与实跑：2026-09-22；归档核对：2026-09-23。
来源：用户指定的 Codex 会话 `01a0c86a-1f47-7d93-a409-97841215a99d`，以及
[原始 89 任务记录](eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash/REPORT.md)。
用户已确认 [D-41](../docs/DECISIONS.md)：full_auto 直接使用主机 Shell；默认模式保留隔离。

## 结果

选取原批次中具有执行环境故障证据的 6 个失败任务，以相同模型、推理档位、原生窗口和任务默认超时各复跑一次：
**0/6 → 6/6**，全部 reward=1、CLI completed/0，0 异常、0 超时。
本批没有给容器添加 `SYS_ADMIN`、`seccomp=unconfined` 或 privileged。
完整 [原始记录](eval/terminal-bench/runs/2026-09-22-tb21-fullauto-fix/REPORT.md) 已归档。

| 任务 | 原 reward | 新 reward | 原 CLI 时长 | 新 CLI 时长 | 原工具数 | 新工具数 |
|---|---:|---:|---:|---:|---:|---:|
| configure-git-webserver | 0 | 1 | 880.449s | 103.250s | 180 | 24 |
| kv-store-grpc | 0 | 1 | 345.142s | 60.886s | 45 | 17 |
| log-summary-date-ranges | 0 | 1 | 外层超时，无最终时长 | 33.968s | 1388 | 15 |
| nginx-request-logging | 0 | 1 | 390.408s | 39.068s | 63 | 13 |
| overfull-hbox | 0 | 1 | 293.549s | 351.402s | 51 | 57 |
| pypi-server | 0 | 1 | 262.286s | 52.451s | 73 | 15 |

工具数统一从留存 JSONL 的有效 `type=tool` 行重计，原报告表格中的部分计数来自较早快照。
六题合计 1800 → 141 次工具调用。`overfull-hbox` 本次耗时、工具数均增加，不能宣称所有任务提速。
本批输入 3,597,566 / 输出 94,301 tokens，缓存命中输入 3,342,208；保留 3 次失败工具回执和
6 次 Shell 非零退出。六题的日志中不再出现原有的 loopback 初始化或只读文件系统错误。

这只是事先选定的失败子集、每题一次，且同时调整了执行方式与指令；不证明单项改动的因果贡献或稳定通过率。
没有重跑全量 89 题，不将新样本拼进原 53/89 分数。其余失败任务和模型能力限制仍待进一步评测。

## 原始证据的修正

1. **网络隔离初始化失败比后台服务问题更广泛。** 合并原 run1/repair 后，88/89 个任务的原生
   Shell 回执含 `bwrap: loopback: Failed RTM_NEWADDR`，共 802 次 shell、36 次 grep。
   若包含转述、读回和私有子代理返回，匹配数为 871；不能把它们全部当成独立 Shell 失败。
   旧代码把 bwrap 的非零退出包装成 `Ok(text)`，工具回执因此仍是 `ok=true`。
2. **full_auto 仍受每次重建的沙箱约束。** 系统目录只读、`/var` 不可见、`/tmp` 随调用销毁，
   后台进程随 PID namespace 退出。模型无法完成服务配置、系统安装和跨调用服务验证。
3. **不能直接把超时归因于多智能体或 high 档位。** `log-summary-date-ranges` 的 9 次 Shell
   全部遇到上述初始化错误，随后退化为 1077 次 `read_file`、39 次任务分配等，最终超时。
4. **完成但评分失败不必然意味着没有验证。** `overfull-hbox` 明确做了 TeX 编译和警告检查，
   却是在缺少 `/var` 的沙箱里重建缓存后验证；这与评分器所见环境不同。本次先消除环境差异，
   再要求按实际验收条件提供证据，不通过自动添加隐藏检查来改变任务。

可复跑的原始计数探针（仓库根执行，无模型请求）：

```bash
python3 - <<'PY'
import collections, json
from pathlib import Path
root = Path('review/eval/terminal-bench/runs/2026-09-22-tb21-teamagents-flash')
trials = {}
for batch in ('run1', 'repair'):
    for p in (root / batch).glob('*/agent/teamagents.jsonl'):
        trials[p.parent.parent.name.split('__')[0]] = p
calls, tasks = collections.Counter(), collections.defaultdict(set)
for task, p in trials.items():
    for line in p.read_text().splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue  # interrupted final JSONL line is not a complete record
        if row.get('type') != 'tool':
            continue
        text = row.get('result', '') + ' ' + str(row.get('error') or '')
        if 'Failed RTM_NEWADDR' in text:
            calls[row['tool']] += 1
            tasks[row['tool']].add(task)
print({tool: {'calls': count, 'tasks': len(tasks[tool])} for tool, count in calls.items()})
PY
```

## 实现

- `engine/src/tools.rs`：可信模式选择 host/bwrap；host 使用当前用户文件系统、网络、PATH/HOME，
  初始环境白名单且不读登录启动文件。cwd/exports 分模式保存并正确引用含空格、引号的状态路径。
  成功调用保留后台服务，超时/取消杀当前进程组；可停止的输出读取替代可能泄漏的阻塞读取线程。
  bwrap 启动错误明确返回失败，默认模式没有不隔离回退。
- `engine/src/session.rs`：缓存执行器每次执行 shell/grep/glob 前读取权威会话模式；坏值或读取失败拒绝执行。
  Leader 默认说明与 D-33 的工具继承、自动通道一致，要求按验收条件验证，减少无效重复委派。
- `engine/src/chat.rs`：主成员与私有子代理每次模型请求看见当前 Shell 模式；`signal_done` 明确只是
  完成申请，运行时不会替模型证明产物正确。工具 schema 仍通过原有 <12000 字符预算检查。
- `engine/src/cli.rs`：`--check` 使用与会话相同的 Shell 执行模式。
- 评测适配器：`PIPESTATUS[2]` 正确记录 TeamAgents 退出码；静态构建补 `relocation-model=static`；
  当前版本默认不安装 bubblewrap、不添加容器权限；`SANDBOX_OVERLAY=1` 仍可复现历史版本。

边界：主机后台服务由任务显式停止，成功调用及 CLI 退出不自动回收。自行 `setsid()` 脱离进程组
的进程不受当前超时/取消保证约束，代码已记录 cgroup 升级方向。文件工具与团队 ACL 保持既有边界；
full_auto 的 Shell 可读取当前用户可访问的主机文件，不承诺同用户凭据文件隔离。
本次没有新增后台终端协议、数据库格式或依赖，也没有安装或发布新版到用户级目录。

## 回归检查

`make check` 通过：core **152**、engine **384**、TUI **109**；engine **3 ignored** 均是需显式启用的检查，
不计真实服务通过。完整 [检查日志](eval/terminal-bench/runs/2026-09-22-tb21-fullauto-fix/make-check.log) 已保存。
新增长期保留的 6 项检查：

- `shell_modes::host_shell_preserves_real_files_cwd_exports_and_filters_credentials`
- `shell_modes::host_shell_background_pipes_do_not_hold_readers_or_lose_foreground_output`
- `shell_modes::host_shell_timeout_and_cancellation_stop_descendants`
- `shell_modes::isolation_start_failure_is_an_error_and_never_falls_back_to_host`
- `chat_e2e::exec_full_auto_service_survives_cli_exit_and_checks_use_host_environment`
- `chat_e2e::shell_execution_and_prompt_follow_live_mode_changes_with_cached_executor`

```bash
make check
cargo test --offline --locked --manifest-path engine/Cargo.toml --test shell_modes
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e exec_full_auto_service
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e shell_execution_and_prompt
```

新增检查包含真实子进程、HTTP 服务、CLI 退出后访问及权限切换；模型协议部分使用本地假服务。
真实模型结论仅来自上面的六个 Harbor trial。未更改 TUI，本轮未追加 PTY 实跑。

另用 Harbor Python 环境执行 `python review/eval/terminal-bench/check_adapter.py`，
真实 shell 管道中的假 TeamAgents 分别退出 0/1/3/124，适配器均原样记录；不调用模型或 Docker。
`bash -n review/eval/terminal-bench/run.sh` 与适配器 Python 语法检查通过。
