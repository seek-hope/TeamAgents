# TeamAgents

[English](README.md) · [设计决策](docs/DECISIONS.md) · [架构图](docs/architecture.canvas)

TeamAgents 在本地工作区执行编程任务。会话中的每个智能体都有持久身份和私有对话记录。未指定接收者时，任务交给 `default`。每个阶段可以自行完成工作，也可以向子阶段分派任务。叶子阶段结束后，由树外的汇总智能体向用户报告结果。

## 安装与启动

Shell 沙箱支持原生 x86-64 和小端 AArch64 Linux。先用系统包管理器安装 `bubblewrap` 和 `slirp4netns`。Ubuntu 可运行 `sudo apt install bubblewrap slirp4netns`。运行时使用 `/usr/bin/bwrap`，联网助手默认为 `/usr/bin/slirp4netns`。如助手安装在其他位置，请在守护进程启动前，将 `TEAMAGENTS_SLIRP4NETNS` 设为可信助手可执行文件的绝对路径。系统须允许普通用户使用用户命名空间和网络命名空间。使用二进制压缩包或源码安装脚本均需单独安装这些依赖。

从源码安装需要 `rust-toolchain.toml` 指定的 Rust 工具链和 C 编译器（用于编译随包提供的 SQLite）。如果 TeamAgents 已在运行，先等在途工作结束，再在每个运行中的会话里执行 `teamagents stop`，然后更换程序。在源码目录运行 `cargo install --locked --path .` 或 `./install.sh`，把 Cargo 安装目录（通常是 `~/.cargo/bin`）加入 `PATH`，运行 `teamagents version` 确认安装成功。

启动会话守护进程前先选好模型服务，在需要处理的项目目录运行客户端。

**使用 ChatGPT 订阅。** 运行 `teamagents auth login`，用同一台机器上的浏览器打开终端给出的 OpenAI 登录链接，确认授权 TeamAgents 使用订阅额度，然后回到终端运行：

```sh
teamagents auth status
TEAMAGENTS_PROVIDER=chatgpt teamagents chat
```

守护进程通过 OpenAI 的[订阅 Responses API](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference) 调用模型。`TEAMAGENTS_MODEL` 指定初始模型。如果没有指定模型，则自动选择账户目录中的第一个可见模型。工作节点与汇报节点都使用选定的服务。账户资格和额度由 OpenAI 决定。授权文件保存在工作区外的 `~/.teamagents-auth/`。`teamagents auth logout` 可清除本机授权。项目中如已有守护进程，请先等在途工作完成，再运行 `teamagents stop`，然后切换模型服务。

**使用 API 密钥。** 连接兼容 Chat Completions 的服务：

```sh
export TEAMAGENTS_PROVIDER=api
export OPENAI_BASE_URL=https://api.openai.com/v1  # 默认地址
export TEAMAGENTS_MODEL='your-provider-model-id'
# 通过密钥管理工具在当前环境设置 OPENAI_API_KEY。
teamagents chat
```

默认情况下，守护进程在已登录时使用订阅，否则使用 API 密钥。设置 `TEAMAGENTS_PROVIDER=chatgpt` 可以在重启后也只使用订阅额度。`OPENAI_BASE_URL` 和 `OPENAI_API_KEY` 仅用于 API 密钥模式。非回环 Chat Completions 地址需要 HTTPS 和密钥，回环地址可使用 HTTP，也可不提供密钥。请求在 120 秒后超时，不跟随重定向，也不重试推理请求。

### 在对话中切换模型和思考强度

| 对话命令 | 作用 |
|---|---|
| `/model` | 查看下一个任务将使用的服务、模型和思考强度。 |
| `/model list` | 查看当前登录的 ChatGPT 账户可用的模型 ID。使用 API 时，填写服务商提供的模型 ID。 |
| `/model MODEL [EFFORT]` | 选择模型。省略强度时使用该模型的默认值。如果模型 ID 恰好也是命令名（如 `default`），可用 `/model set MODEL [EFFORT]`。 |
| `/model effort high` | 仅修改思考强度。`/model effort default` 恢复模型默认强度。 |
| `/model default` | 恢复守护进程的 `TEAMAGENTS_MODEL` 设置和模型默认强度。 |

TeamAgents 接受 `none`、`minimal`、`low`、`medium`、`high`、`xhigh` 和 `max`，每个模型支持的档位由服务商决定。`default` 表示由服务商选择。设置按会话和模型服务保存，从下一次通过 `chat` 或 `exec` 提交任务时生效。子节点、修复节点、最终汇报都沿用根任务提交时的选择；回答澄清问题或重启后恢复工作时也一样。

### 交互式 TUI 与纯文本对话

在交互式终端中运行 `teamagents` 或 `teamagents chat [--review]` 会自动进入 TUI；使用管道输入、`--json` 或 `--plain` 时回退为纯文本行模式。使用 `teamagents tui [--review]` 可显式启动 TUI（需要交互式终端）。TUI 中快捷键负责导航、布局与直接操作，斜杠命令负责配置与工作流控制，二者互补不重叠：

| 快捷键 | 作用（导航、布局与直接条目操作） |
|---|---|
| `Tab` / `BackTab` | 切换焦点面板（输入区、侧栏、详情检查器）或补全 `/命令` |
| `Ctrl+B` | 显示/隐藏左侧边栏 |
| `Ctrl+P` | 打开待审批队列（`a` 批准，`d` 拒绝） |
| `Alt+1`–`4` / `1`–`4` | 切换检查器标签页（`Summary`、`Evidence`、`Tree`、`Log`） |
| `[` / `]` | 切换侧栏视图（`Sessions` 会话 / `Agents` 智能体） |
| `a` / `d` / `r` / `x` | 批准 / 拒绝 / 回复澄清问题 / 取消运行中任务 |
| 连续两次 `Del` | 选中会话后第一次触发删除确认提示，第二次执行删除 |
| `Enter` / `Shift+Enter` | 提交任务或回答 / 插入换行 |
| `F1` / `Ctrl+Q` | 打开快捷键与命令参考 / 退出 TUI（守护进程继续后台运行） |

互补的 TUI 斜杠命令：`/model`、`/to`、`/check`、`/review`、`/new TEXT`、`/answer ROOT_ID TEXT`、`/clear`、`/stop`。

## 提交任务并验证结果

新任务默认无人值守。`exec` 默认等到最终报告或澄清问题，没有客户端等待时限。下面的示例要求修复脚本，并指定如何检验其输出。

```sh
teamagents exec --check 'test "$(sh greet.sh Ada)" = "Hello, Ada!"' \
  '修复 greet.sh，让它按传入的名字问候用户。'
```

**任务如何流转。** `handoff` 在同一笔事务中结束当前阶段，创建 1 至 16 个子阶段。每个子阶段都收到发送方的阶段小结和分给自己的任务。分派时可以在会话成员数上限内新增具名智能体，也可以选择尚未参与本次任务的已有成员。一个成员在同一棵树中最多承担一个阶段。子阶段可使用工作区工具、自行完成并提交叶子报告，或再次向下分派。子阶段不能唤醒父阶段，也不能把工作转给其他同级阶段。

**怎样判定验收成功。** `--check` 授权执行你指定的原样命令。所有执行分支结束后，带检查的根任务须通过该命令，汇总智能体才能给出成功报告。检查失败时，运行时可以把诊断交给新的阶段来修复问题，即使会话的成员名额已满也可以。**包括首次执行在内，检查最多运行三次**，共享的模型调用预算可能提前耗尽。未指定 `--check` 时，即使模型主动运行的命令成功，`verification` 仍为 `not_requested`。

汇总智能体收到原始请求、澄清问答、叶子结果，以及运行时判定的整体结果和验收状态。各智能体的私有对话历史和工具原始输出不在汇总输入内。它只能编写报告，无权运行工具、重启阶段或更改验收结论。若无法生成可用的模型报告，运行时将根据已记录的叶子结果返回部分报告。即使此前执行成功，整次运行也会标为 `failed`。

**跟踪根任务并回答问题。** `exec --detach --json` 立即返回根任务 ID。`exec --timeout SECONDS` 只限制客户端等待时间。重新连接时使用同一会话和工作区。如果缺少必要信息，首个执行阶段可以在调用工具或分派前向你提问。`exec` 会打印问题并以非零状态码退出，收到答复后任务才会继续。例如，它询问该实现哪种问候语时，可以用根任务 ID 查看问题并作答。

```sh
teamagents status ROOT_ID --json
teamagents answer ROOT_ID '请使用完整的问候语。'
```

`teamagents status ROOT_ID` 显示整次运行的状况。有分支时，它会用树形结构列出各阶段的状态和任务 ID。在 JSON 中，`status` 表示整体状态，`stage_status` 表示首个阶段的状态，可能已是 `done`。`tree_nodes` 列出各阶段，`leaf_results` 包含叶子状态及报告，`subtree_receipts` 列出实际操作的回执摘要。用 `teamagents status TASK_ID` 查看单个阶段的完整 `evidence`。

根任务状态为 `input` 时，先读 `question`，再用该根任务的 ID 作答。在对话中可用 `/status ROOT_ID` 和 `/answer ROOT_ID TEXT`。当会话中只有一个待答问题、且问题属于当前选定的智能体时，直接输入文字即可作答。有待答问题时，用 `/new TEXT` 另开任务。

`/members` 列出智能体，`/to NAME` 选择下次提交对话任务时的接收者，`/tasks` 列出会话任务。离开对话或等待超时都不会取消任务。`cancel TASK_ID` 请求取消任务，`delete ROOT_ID` 删除已结束或空闲的会话树，`stop` 关闭空闲的守护进程并保留会话。

**如需逐项审批模型请求的 shell 命令或文件编辑**，使用 `teamagents exec --review '…'` 或 `teamagents chat --review`。子任务继承根任务的策略。在同一会话中用 `teamagents tasks` 查找待审批任务，用 `teamagents status TASK_ID` 阅读具体操作，再将该操作的 `approval_id` 传给 `teamagents approve APPROVAL_ID` 或 `teamagents deny APPROVAL_ID`。`--check` 指定的原样命令已在提交时获得授权。

## 工作区的访问范围

Shell 命令和验收检查都在工作区的暂存副本 `/workspace` 中运行。每条命令都有独立的沙箱家目录和 `/tmp`，通过网络安装到这两个目录的依赖和 Cargo 缓存不会保留到下一条命令。暂存支持目录和只有一个硬链接的普通文件，上限为 256 MiB、32,768 个条目、64 层目录。不支持符号链接、多硬链接文件、特殊文件和嵌套挂载。Git 元数据和用 `CACHEDIR.TAG` 标记的 Cargo `target` 缓存不进入副本。如果超过上限，可选择一个较小的项目目录。

命令正常退出后，即使退出码非零，也可能写回改动。运行时先检查改动是否与宿主机上的编辑冲突，再逐项写回。因此崩溃或 I/O 错误可能造成部分改动已经写回。命令的执行时限为 60 秒，标准输出和标准错误各限 64 KiB。

**请仔细选择工作区。** Shell 能对外联网，也能把工作区内的文件（包括你放进去的密钥）发送给外部服务。沙箱不挂载宿主机的家目录、凭据存储或守护进程套接字。回环地址指向沙箱自身，但局域网中的其他服务仍可能可达。

## 会话与限制

`--workspace DIR` 选择工作区，默认使用当前目录。其规范路径决定会话位置 `${XDG_STATE_HOME:-$HOME/.local/state}/teamagents-swarm/sessions/<workspace-hash>`，也可以用 `--session DIR` 指定会话目录。重新连接时沿用同一对路径。会话目录必须由当前用户所有，不允许组或其他用户写入，也不能与工作区重叠。

| 守护进程环境变量 | 默认值 | 限制范围 |
|---|---:|---|
| `TEAMAGENTS_MAX_AGENTS` | 8 | 会话内的持久成员总数，含 `default`。创建会话时确定。 |
| `TEAMAGENTS_TURN_BUDGET` | 32 | 整次运行的模型调用次数，包括执行、修复和最终汇总。创建会话时确定。预算大于 1 时，为汇总预留一次调用。 |
| `TEAMAGENTS_CONCURRENCY` | 4 | 会话内同时进行的模型或工具调用数。守护进程启动时确定。 |

每个会话只有一个守护进程写 SQLite，使用 WAL 和 `synchronous=FULL`。已启动的工具如果没有持久化结果，其效果未知，守护进程不会自动重放。开始新任务前请检查工作区及可能发生的外部操作。取消任务不会撤销已有操作，也不能证明命令已经停止。模型调用次数和并发数都不等同于 token 费用上限。

开发时，`make build` 生成 `target/debug/teamagents`，`make fmt` 格式化代码，`make check` 检查格式、运行 Clippy 和测试。`make package` 在 `target/dist/` 生成当前主机的二进制压缩包及 `SHA256SUMS`。
