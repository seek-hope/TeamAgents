### 安装与开始使用

仓库和发行包已公开，无需 GitHub 登录。推荐使用自动安装程序：

```bash
(
  set -eu
  installer="$(mktemp)"
  trap 'rm -f "$installer"' EXIT
  curl -fsSL https://raw.githubusercontent.com/seek-hope/TeamAgents/main/install.sh -o "$installer"
  sh "$installer"
)
export PATH="$HOME/.local/bin:$PATH"
teamagents init
export DEEPSEEK_API_KEY='你的模型密钥'
teamagents doctor
teamagents
```

安装器自动下载最新版、校验 SHA-256，并安装两个程序；重复执行可升级，已有配置与会话保留。
将 `export PATH="$HOME/.local/bin:$PATH"` 加入 `~/.bashrc` 或 `~/.zshrc`，以后打开终端也能直接使用。

也可从本页 Assets 下载 `install.sh`、发行包和 `SHA256SUMS`，在同一目录执行
`sh install.sh --archive ./teamagents-VERSION-x86_64-unknown-linux-musl.tar.gz`（VERSION 换成本页版本）。
指定版本或目录使用 `--version VERSION` / `--bin-dir DIR`。

### 运行要求

- Linux x86_64；发行包为 musl 静态链接，无需 Rust 工具链。
- Shell 隔离需要 `bubblewrap`：Debian/Ubuntu 用 `sudo apt install bubblewrap`。
- `init` 创建内置最小配置，不写入密钥、不覆盖已有文件；默认使用 DeepSeek Flash，其他服务可编辑 TOML。
- Codex CLI 仅在使用 Codex 执行成员时需要；`doctor` 将可选能力问题标为 WARN。

完整安装、升级与故障处理见 [安装指南](https://github.com/seek-hope/TeamAgents/blob/main/docs/INSTALL.md)。
