# v0.1.2 二进制发行验证（2026-09-18）

已公开发布 [v0.1.2](https://github.com/seek-hope/TeamAgents/releases/tag/v0.1.2)，
GitHub API 回读为最新版、非草稿、非预发行。标签指向 `0b058f620e2cf374e9f4e6c0a05250fd6143eb0f`。
平台为 Linux x86_64，包内含 `teamagents` 与 `teamagents-tui` 两个 musl 静态程序。

## 构建与回归

- [main CI](https://github.com/seek-hope/TeamAgents/actions/runs/35328233859) 与
  [标签 CI](https://github.com/seek-hope/TeamAgents/actions/runs/35328463949) 均通过。
  main CI 日志汇总：core 63、engine 222、TUI 91 项通过，0 失败；engine 另有 1 项显式 ignored。
- [发行工作流](https://github.com/seek-hope/TeamAgents/actions/runs/35328463977) 成功，包含版本一致性检查、
  两程序构建、实际制品安装与 init 冒烟、附件上传及公开发布。
- 发布前修正了 doctor 测试对宿主 bwrap 的依赖，以及 `/model` 测试并发改写进程环境的竞态；
  证据和复跑命令见 [安装验证记录](install-2026-09-18.md#发布前-ci-测试修正)。
- CI 的 bubblewrap 与真实服务跳过边界仍见 [验收对照表](../docs/ACCEPTANCE.md)，本次没有调用真实模型 API。

## 公开制品复核

通过不带 GitHub 凭据的 curl 下载三个附件，`sha256sum -c SHA256SUMS` 全部通过：

```text
4ca7ee6adbbc5b614a87e45e8560adec38f31bc340b36ee4bce0df523fd3826e  teamagents-0.1.2-x86_64-unknown-linux-musl.tar.gz
b9354eca758e8df6974905ae8dacdeb6402a2b381168c03d55d956e897aaa4c1  install.sh
```

- 离线安装到含空格的临时目录成功，两个已安装文件与归档内文件的 SHA-256 一致。
- `file` 确认两程序均为 x86-64、`static-pie linked`；`teamagents version` 返回 `0.1.2`。
- 安装器没有提前创建配置；`init` 创建内容与内置最小模板一致的文件，权限为 `0600`，
  再次 `init` 保留文件内容和修改时间。
- 将返回失败的 `gh` 替身置于 PATH 首位，强制公开 curl 路径；安装器自动选择 v0.1.2，
  完成实际下载、校验和成对安装。
- 使用安装后的 `teamagents` 启动同目录 TUI，复用 `tui/scripts/pty_smoke.py`：
  启动、括号粘贴、消息回显、任务/批准面板切换、终端模式恢复及退出全部通过。
  使用独立 XDG 配置/状态目录，并移除 `DEEPSEEK_API_KEY`。

## 复跑安装与初始化

在仓库根目录运行；所有写入都在临时目录，不修改用户安装或配置：

```bash
release_check_dir="$(mktemp -d)"
for asset in install.sh SHA256SUMS teamagents-0.1.2-x86_64-unknown-linux-musl.tar.gz; do
  curl -fSL "https://github.com/seek-hope/TeamAgents/releases/download/v0.1.2/$asset" \
    -o "$release_check_dir/$asset" || exit 1
done
(cd "$release_check_dir" && sha256sum -c SHA256SUMS) || exit 1
XDG_CONFIG_HOME="$release_check_dir/config" sh "$release_check_dir/install.sh" \
  --archive "$release_check_dir/teamagents-0.1.2-x86_64-unknown-linux-musl.tar.gz" \
  --bin-dir "$release_check_dir/bin"
XDG_CONFIG_HOME="$release_check_dir/config" "$release_check_dir/bin/teamagents" init
XDG_CONFIG_HOME="$release_check_dir/config" "$release_check_dir/bin/teamagents" init
XDG_CONFIG_HOME="$release_check_dir/config" "$release_check_dir/bin/teamagents" version
```

继续对该安装目录运行真终端检查：

```bash
TA_RELEASE_CHECK_DIR="$release_check_dir" python3 - <<'PY'
import importlib.util, os, sys
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("release_pty", "tui/scripts/pty_smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
root = os.environ["TA_RELEASE_CHECK_DIR"]
smoke.BIN = root + "/bin/teamagents"
smoke.ENV = dict(os.environ, TERM="xterm-256color",
                 XDG_CONFIG_HOME=root + "/config", XDG_STATE_HOME=root + "/state")
for key in ("DEEPSEEK_API_KEY", "TEAMAGENTS_TUI", "TEAMAGENTS_ENGINE"):
    smoke.ENV.pop(key, None)
raise SystemExit(smoke.main())
PY
```
