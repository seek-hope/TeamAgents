### 下载安装

仓库是私有的，用已登录（`gh auth login`）的 `gh` 拉包：

```bash
ver=0.1.0   # 换成需要的版本号（本页顶部标题里的 v 后部分）
gh release download "v$ver" --repo seek-hope/TeamAgents \
  --pattern "teamagents-$ver-x86_64-unknown-linux-musl.tar.gz" \
  --pattern SHA256SUMS
sha256sum -c SHA256SUMS
tar -xzf "teamagents-$ver-x86_64-unknown-linux-musl.tar.gz"
install -Dm755 "teamagents-$ver-x86_64-unknown-linux-musl/teamagents" \
                "teamagents-$ver-x86_64-unknown-linux-musl/teamagents-tui" ~/.local/bin/
teamagents doctor          # 自检：依赖 / 配置 / 密钥 / 隔离 / Codex 协议
```

静态链接（musl），不挑发行版的 glibc；`~/.local/bin` 不在 `PATH` 时自行加入。

### 运行要求

- Linux + `bubblewrap`（成员 shell 隔离，缺失会明确报错，不会退化成不隔离执行）
- 模型密钥（`DEEPSEEK_API_KEY` 等）与 `~/.config/teamagents/config.toml`，参见包内
  `config.example.toml` 与 README
- `codex` CLI 仅在使用 Codex 执行成员时需要
