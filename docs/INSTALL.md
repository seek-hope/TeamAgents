# 安装、首次使用与升级

TeamAgents 当前支持 **Linux x86_64**。发行包采用 musl 静态链接，使用时不需要 Rust、Python
或 Node.js；Codex CLI 仅在使用 Codex 执行成员时需要。模型服务需要你自己的凭据。

## 1. 下载与安装

推荐执行 [README 的安装命令](../README.md#安装最新版推荐)：自动选择最新发行版、校验 SHA-256，
将 `teamagents` 和 `teamagents-tui` 安装到 `~/.local/bin`。
两个程序应保持同一版本并安装在同一目录。

仓库已公开，无需 GitHub 账号或克隆仓库。安装器优先使用已登录的 `gh`；没有时使用 `curl`。
安装器仅支持 Linux x86_64，其他系统会在下载前报错。

**自定义安装：** 将 README 中的 `sh "$installer"` 换为以下任意一条：

```bash
sh "$installer" --version 0.1.2             # 指定版本，支持带 v 前缀
sh "$installer" --bin-dir "$HOME/bin"      # 自定义目录，也支持 TEAMAGENTS_BIN_DIR
sh "$installer" --archive /path/to/teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz
```

本地安装不联网，发行包旁需有配套 `SHA256SUMS`。安装器只校验当前平台的发行包，
不会因校验清单中列出其他平台而失败。校验失败或包不完整时，已安装程序保持原样；
替换第二个程序失败时会恢复旧程序。安装器不调用 sudo、不修改 Shell 启动文件。

**浏览器下载：** 打开 [最新发行版](https://github.com/seek-hope/TeamAgents/releases/latest)，
从 Assets 下载 `install.sh`、`.tar.gz` 和 `SHA256SUMS` 到同一个空目录，运行：

```bash
# 将 VERSION 换成下载的实际版本号。
sh install.sh --archive ./teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz
export PATH="$HOME/.local/bin:$PATH"
```

旧版 v0.1.1 的 Assets 没有安装脚本，可从 README 所用的源码地址下载 `install.sh` 后使用本地安装。

**源码构建：** 见 [README](../README.md#从源码构建)。只需构建 engine 和 tui，Cargo 会自动构建 core。
已有源码时也可以直接运行 `sh install.sh` 安装预编译发行版。

## 2. 首次配置与启动

运行以下命令，使用程序内置的模板生成最小配置，无需寻找或复制示例文件：

```bash
teamagents init
```

配置位置为 `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents/config.toml`；重复运行会保留已有文件，
包括符号链接。新文件权限为 `0600`，不包含密钥，也不会创建会话。
默认 profile 为 `leader_main`，模型为 `deepseek-flash`（上下文 1,000,000，推理档位 max）。
若使用其他模型服务，先编辑
配置中的 `provider`、`protocol`、`model`、`base_url` 和 `api_key_env`，格式见
[用户指南](USER-GUIDE.md#1-配置)，并同步填写所选模型的原生 `context_window`。
配置中只填写密钥环境变量名；网页搜索与 MCP 需要时再按发行包中的 `config.example.toml` 配置。

旧版 v0.1.1 没有 `init`，安装脚本会在配置缺失时复制发行包模板，直接继续设置密钥即可。

安装 Shell 隔离依赖，按发行版选择一条：

| 发行版 | 命令 |
|---|---|
| Debian / Ubuntu | `sudo apt install bubblewrap` |
| Fedora | `sudo dnf install bubblewrap` |
| Arch Linux | `sudo pacman -S bubblewrap` |

在同一个终端中设置密钥并启动：

```bash
export DEEPSEEK_API_KEY='你的模型密钥'
teamagents doctor
teamagents --cwd /path/to/project
```

将 `/path/to/project` 换成实际项目目录；省略 `--cwd` 即使用当前目录。进入后直接输入目标。
完整参数可运行 `teamagents --help` 查看。无交互终端可使用 `teamagents --plain`。

`doctor` 验证本机条件，不发送模型请求。`FAIL` 需要修复；`WARN` 表示可选能力有问题，
例如缺少 Codex CLI 不影响内置成员，但 Codex 成员需要相关检查通过。
已发布的 v0.1.1 使用旧版诊断，可能将缺少 Codex 列为失败；从 v0.1.2 起使用新版诊断。

## 3. 升级与卸载

升级前退出正在运行的 TeamAgents，再执行 README 的安装命令，两个程序会一起更新。
命令不会覆盖已有配置或删除会话。升级后运行 `teamagents version` 和 `teamagents doctor`。
若需要为已有配置增加新字段，请自行编辑；`init` 不会自动迁移或合并配置。
指定旧版本安装可回退程序，但跨版本会话兼容性需查看对应发行说明。

卸载程序：

```bash
rm -f ~/.local/bin/teamagents ~/.local/bin/teamagents-tui
```

配置和会话会保留，分别位于 `${XDG_CONFIG_HOME:-$HOME/.config}/teamagents` 与
`${XDG_STATE_HOME:-$HOME/.local/state}/teamagents`。只有确实不再需要时才自行删除这些目录。

## 4. 常见问题

| 现象 | 处理 |
|---|---|
| `teamagents: command not found` | 执行 `export PATH="$HOME/.local/bin:$PATH"`，并将其加入当前 Shell 的启动文件（如 `~/.bashrc` / `~/.zshrc`） |
| 找不到 `teamagents-tui` | 两个程序一起安装到同一目录；勿只复制 `teamagents` |
| 配置缺失 / 模型未配置 | 按第 2 节创建配置；自检会显示实际配置路径 |
| 密钥未设置或为空 | 在启动 TeamAgents 的同一终端导出配置引用的环境变量 |
| 下载 404 / 无发行包 | 检查版本号、网络及发行列表；私有派生仓库还需核对 GitHub 账号权限 |
| SHA-256 校验失败 | 停止安装，清空这次下载后重新下载发行包及配套校验文件 |
| 已安装 bwrap，但探针失败 | 检查容器、内核或发行版是否限制非特权 user namespace；受限环境不能运行成员 Shell |
| macOS / Windows / Linux ARM | 当前没有对应发行包；不要下载 x86_64 Linux 包尝试原生运行 |
