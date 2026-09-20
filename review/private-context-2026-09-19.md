# T6 私有上下文与工具读取隔离（2026-09-19）

本批按方案 §5.1、§7、§12.2、§14、T6，接续[投递授权修复](delivery-acl-2026-09-19.md)和
[会话身份验收](member-lifecycle-2026-09-19.md)。使用生产 `open_session`、ChatRunner、ToolGateway、
文件工具与真实 bubblewrap；模型端为本地 HTTP 协议夹具，逐条检查实际收到的请求。
没有调用真实模型，也没有新增依赖。

## 先复现，再修复

1. **自动输出与主动制品共用目录。** 所有成员的工具执行器都收到会话的 `artifacts/`。
   A 调用 Shell 产生超 200 KB 输出后，B 用同一个已知 `/artifacts/exec-*.log` 引用读到了
   `PRIVATE_SHELL_OUTPUT_A`。UUID 文件名并不构成成员授权。
   专属回归先失败，信息为 `b can read another member's shell log`。
2. **共享工作根覆盖运行时状态。** 项目目录是 XDG 状态的祖先时，根路径限制仍允许
   C 读取 A 的 `members/a/chat_tree.json`；Shell 的整个工作根挂载也无法保证隐藏它。
   专属回归先失败，信息为 `the workspace exposes private state`。

可复核的本机原始日志：

```text
/tmp/teamagents-t6-red-verified-20260919.log
/tmp/teamagents-t6-overlap-red-20260919.log
```

最初的输出探针曾错误地把长工具结果的模型预览当作完整 JSON 解析；预览会被截断。
修正探针后才记录上面的失败证据，没有把探针解析失败当成产品缺陷。

私信投递本身未复现泄露：虽然消息事件的审计 `audience` 包括 Leader，当前 `event_push` 和
`project_delivery` 不会因此给 Leader 模型正文。本批没有修改该投递规则，也没有混淆用户的只读
history 界面和 Leader 的模型上下文。

## 最终行为

- 会话运行器将 Shell、grep 等自动输出写到 `members/<成员>/tool-output/exec-*.log`。
  `/tool-output/` 是相对当前成员的只读虚拟根，`read_file` / `read_artifact` 可分页读回；
  其他成员即使知道完整引用也不能访问原文件。正常恢复沿用同一目录，新会话同 ID 不继承。
- `/artifacts/` 继续表示主动交付的会话共享文件。模型工具说明明确区分两类目录，成员须主动
  将可共享成果写到共享目录，再经获准消息或共享空间告知接收者。复制引用本身不会扩大私有目录权限。
- 文件读取与 `view_image` 后的实际请求图片加载共用目录规则。穿越、跨根符号链接及写入私有
  输出根均被拒绝；图片测试检查了实际编码后的请求，防止文本工具拒绝但图片加载又读到内容。
- 升级前 `artifacts/exec-*.log` 没有可靠所有者信息。保留原文件，成员文件工具拒绝读写这类文件，
  也拒绝指向它们的符号链接；不猜测归属、不自动迁移、不删除用户数据。原有私有历史中保存过的
  工具结果仍可 `read_history` 读回，未保存的输出尾部不能由它恢复。
- 共享工作根与 TeamAgents 状态目录、用户配置目录、`CODEX_HOME` 互相包含时，在准备成员工作根时
  明确拒绝；运行器和工具执行器共用该入口，git-worktree 回落到 shared 时也遵守该检查。
  比较经过符号链接解析，对尚未建立的目录解析最近的现存祖先。管理的 isolated/worktree 根只挂载
  成员自己的 `work/`，不把旁边的历史、工具输出和 Shell 状态交给文件工具或 Shell。
- 原有单输出 64 MiB 上限保留；512 MiB 清理预算现在按成员输出目录执行，清理仍是新建输出时
  按 mtime 删除最旧自动日志。主动共享制品不参与该清理，历史检查点仍没有这项配额。

生产修改集中在 `engine/src/tools.rs`、`session.rs` 和 `chat.rs`，没有更改核心事务或成员通信权限。

## 六项专属验收

新增 `engine/tests/private_context.rs`，A/B/C 与 Leader 是四个独立 Chat 成员。
每项否定断言都要求实际有模型请求或实际工具调用，避免“没有运行，所以没有泄露”的空证据。

| 测试 | 检查范围 |
|---|---|
| `t6_private_messages_history_and_new_sessions_are_isolated_on_the_wire` | A 经原生团队工具给 B 发私信，B 收到而 C/Leader 不收到；成员工具输出和指令分开；伪造 `read_history` 成员/会话/线程参数不能越权；文件工具不能读其他历史或数据库；本人恢复可读旧结果；全新会话四成员都不继承旧私信和历史 |
| `t6_observer_scope_and_authorized_forwarding_do_not_grant_private_history` | `status`、`public_message`、`result` 三种观察范围；有权消息正常到达，但不附带被观察成员的私有历史，也不产生反向通道；B 主动经获准通道转发给 Leader 时正文正常到达 |
| `t6_automatic_shell_output_is_private_but_explicit_artifacts_remain_shared` | 真实 Shell 超长输出；本人可读，B/C/Leader 用同引用的两种读工具均被拒；Shell 对确实存在的数据库、其他成员历史、Shell 状态、自动输出及环境哨兵均不可见；主动制品发布/读回成功；恢复与新会话边界 |
| `t6_shared_workspace_cannot_expose_the_runtime_state_tree` | 共享工作目录包含状态树时拒绝；修复前探针确实从 C 的文件工具读到了 A 的历史 |
| `t6_shared_workspace_rejects_symlinked_or_not_yet_created_private_roots` | 配置目录符号链接、状态目录符号链接、尚未建立的 CODEX_HOME 都不能绕过重叠检查；模型调用数为零 |
| `t6_legacy_logs_and_private_image_references_cannot_bypass_file_permissions` | 无归属旧日志及别名拒读；私有输出根穿越与写入拒绝；本人/他人的图片加载隔离；显式共享图片仍进入请求；原文件保持不变 |

旧恢复测试的一些夹具把 `/tmp` 或包含配置/状态的整个临时根当项目。按照新的真实约束调整为
独立 `project/`，保留恢复、取消、分叉、会话切换和副作用次数的原断言；不以豁免路径绕过产品检查。
受影响文件为 `chat_e2e.rs`、`codex_recovery.rs`、`fork_rewind.rs`、`worker_protocol.rs`。
两项原 PTY 脚本也改用专门临时项目目录，工作区审查 PTY 原本已分开目录，无需改动。

## 检查与边界

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test private_context -- --nocapture
cargo test --offline --locked --manifest-path engine/Cargo.toml \
  --test codex_recovery --test fork_rewind --test worker_protocol
make check
make pty
```

`make check` 通过：**core 79 / engine 295 / tui 104**，含格式与严格 Clippy。
`make pty` 的启动/粘贴、点击命中、工作区审查三项也全部通过。
Engine 新增六项集成测试，库单测仍为 100；显式 ignored 项与 `live_codex` 未开开关时的提前返回仍如实保留。
本批没有重复此前的真实 Codex/DeepSeek 评测，也不将本地夹具计为供应商验收。

日志：

```text
/tmp/teamagents-t6-integration-20260919.log
/tmp/teamagents-t6-check-20260919.log
/tmp/teamagents-t6-pty-verified-20260919.log
```

T6 的三项原场景现在有专属端到端自动化证据；不是对恶意同用户宿主进程的绝对保密承诺。
配置为宿主执行的 MCP、外部 Codex 的全自动权限和用户主动复制私有内容不在本批阻止范围内。
另外，方案 §5.2 的 `publish_shared.ref` 私有上下文引用格式禁用规则尚未补齐：
当前发布字符串不会授予相应文件/历史读取权限，但不能把工具隔离已通过等同于该输入校验已实现。
五家真实供应商矩阵、复杂仓库任务和同条件竞品对照仍待完成。

同日后续：上述附件引用校验缺口已由[成果引用修复](output-references-2026-09-19.md)补齐，
同时覆盖 `complete_task.result_refs` 和任务结算时复核。本文保留本批结束时的验收范围。
