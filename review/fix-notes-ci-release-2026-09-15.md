# 修复台账：CI 转绿 + 首个发行版（2026-09-15）

本轮起因：把本地 37 个提交推到 `origin/main` 并发布可下载的发行包，途中发现 CI 从未成功过
（`ci.yml` 是当天早些时候加的，5 次运行全红）。下面每条都先复现、再修、再留证据。

## 发行

- `.github/workflows/release.yml`：打 `v*` tag 构建 `x86_64-unknown-linux-musl` 静态包
  （musl 不挑用户 glibc），strip + `tar.gz` + `SHA256SUMS`；`workflow_dispatch` 只验证构建、不建 Release。
- `.github/release-notes.md`：Release 正文的中文下载安装说明（私有仓库 → `gh release download` + 校验和）。
- `README.md`：改为产品介绍 + 使用说明（安装/快速开始/参数键位/配置/现状与限制/文档地图）。
- 版本号 `core`/`engine`/`tui` 0.1.0 → 0.1.1；`codex.rs` 的 `clientInfo.version` 改用
  `env!("CARGO_PKG_VERSION")`，不再手写常量。
- 已发布 `v0.1.0`（先验证流水线），修复后再发 `v0.1.1` 作为可下载版本。

## 真缺陷（两处，都影响产品行为）

1. **批准决定早于"停下等批准"到达 → 回合永远醒不过来**（CI 抓到，`chat_e2e` 三条审批用例）
   - 证据：`runs [WAITING_APPROVAL], pending 0, events [user_message, run_started, approval_decided]`
     —— 用户已批准、行已决定，回合却停在等待上。
   - 根因：决定先 commit，`wake_approval_run` 见回合仍是 RUNNING 便什么都不做；随后回合才落
     WAITING_APPROVAL，再无人唤醒。用户手快或自动化拍板即可复现。
   - 修复：`core/src/control.rs::finalize_run_inner` 在落 WAITING_APPROVAL 前按 run_id 查 PENDING 行，
     为空（决定已落地）就把回合同事务置回 RUNNING，由网关读到那条已决定的行。
   - 回归：`core/tests/engine.rs::run_parked_after_its_approval_was_decided_is_woken_instead_of_stuck`
     （撤掉修复即失败：left WaitingApproval / right Running）。
2. **Codex 成员的批准 waiter 同样会丢决定**
   - `codex.rs` 注册 waiter 后新增 `decided_before_waiting`：行已被决定就直接采用，不再干等到
     `approval_wait_timeout`（600s）后回 decline。回归：
     `engine::codex::tests::an_already_decided_approval_is_read_instead_of_waited_for`。

## 健壮性

3. **`CodexAppServer::close()` 依赖 PATH 上的外部 `kill` 二进制**：最小镜像/裁剪 PATH 下拿不到，
   app-server 起的孙进程（如它自己的 shell 命令）会活下来。
   - 复现：把 `kill` 从 PATH 拿掉后旧代码原样失败（`the spawned child of the app-server survives close()`）。
   - 修复：改用 `/bin/sh -c 'kill -TERM/-KILL -<pgid>'`（POSIX 内建，不依赖 PATH），并在宽限期后
     对**整个进程组**补 SIGKILL。
4. **`chat.rs::system_prompt` 留下未用变量 `allowed`**（重构残留）：删掉，少一次绑定计算。

## CI 与测试解耦（原先 6 个测试二进制依赖开发机）

| 现象（CI） | 处理 |
|---|---|
| `mcp::tests::stdio_workspace_isolates_*`：无 bwrap 直接 unwrap panic | 无 bwrap 时改为断言"不得降级执行"（workspace 必须 `IsolationUnavailable`）再跳过 |
| `chat_e2e` 4 条真沙箱用例失败 | 同上：`bwrap_available()` 为假时打印原因并跳过 |
| `cli.rs` 两条：`invalid: unknown model profiles ["leader_main"]` | 测试自建 `XDG_CONFIG_HOME`；doctor 的 schema 断言只在真的装了 codex 时才要求 |
| `fork_rewind` 两条：`fork_session failed: unknown model profile leader_main` | worker 启动指向临时配置目录（含 `[models.leader_main]`） |
| `mcp_stdio` / `mcp_tools`：workspace 模式无 bwrap 起不来 | 与隔离无关的用例改为显式 `host` 模式；隔离覆盖保留在库级用例 |
| 审批用例失败信息只有一句断言 | 失败信息带上 runs 状态 / pending 数 / 事件种类，下次直接可诊断 |
| `Check patch whitespace`：把 `review/*.svg` 的历史空格算到本次提交 | `fetch-depth: 0`：shallow clone 下 HEAD 无 parent，整棵树被当成本次新增 |

**验证方式**：本机造"类 CI 环境"（无用户配置、无 codex、无 bwrap 的 PATH）逐个跑 18 个 engine
集成二进制，先复现失败再修，修完全绿；正常环境 core 55 / engine 189 / tui 91 亦全绿。

## 仍未做

- 三类线上协议（responses / anthropic / chat-completions）的真机 smoke：需要 Anthropic/Gemini 等凭据。
- CI 上的真隔离覆盖：GitHub runner 无法用 bubblewrap（见上），保持"本机权威 + CI 跳过并可见"。
