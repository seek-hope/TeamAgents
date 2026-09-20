# 语言工具链与交付文件（2026-09-19）

按 D-32/D-35 与方案 §12，扩展真实 Shell 的 Python/Node 构建验收，修复工具 HOME 污染项目和发布包的问题。
本批使用本地项目、系统工具与真实 bubblewrap，没有调用模型或远端包仓库。
完整日志、命令、校验值和本批增量补丁见[证据目录](eval/runs/2026-09-19-language-toolchains/REPORT.md)。

## 实际缺陷与修复

原实现将 Shell 和 workspace 模式 MCP 的 `HOME` 设为项目根。Node 项目执行 `npm ci`、`npm test` 后，
项目出现 `.npm/_logs` 等文件；随后 `npm pack` 把这些日志、缓存一起打包。无依赖探针的首个包共六个文件，
其中四个是 npm 运行时文件。带本地依赖的回归进一步复现 `_cacache`、七个日志与其它运行时文件进入包。
MCP 也会将服务默认缓存直接写入项目。问题会影响交付范围、审查噪音和项目磁盘用量。

`tools.rs::sandbox_home` 现在为有状态 Shell 选择成员 `shell/home/`，作为已有 Shell 状态挂载的一部分；
缓存和用户级工具配置随该成员保留。无持久状态的 Shell 和 workspace MCP 使用命名空间内的临时 HOME，
其内容在进程退出后消失。临时 HOME 放在 `/run/teamagents/home`，避免 doctor 等以 `/tmp` 为工作根的调用
遮盖它。创建目录在 bubblewrap 命名空间内完成，不让引擎在宿主上跟随旧 HOME 的符号链接创建目录。
宿主 HOME、包管理器凭据和额外环境变量均未被导入；MCP host 模式继续使用原授权环境。

旧 Shell 状态没有版本字段：恢复时仅将等于初始项目根的旧 HOME 换成新默认值，保留当前目录与普通导出。
新快照写入 `__ta_home_version=1`，之后即使成员显式把 HOME 设回项目根也按其选择恢复。
旧 `.npm` 等项目文件不移动、不删除，也不将旧缓存导入成员目录。

## 五项新增回归

| 测试 | 实际检查 |
|---|---|
| `shell_home_preserves_member_caches_without_polluting_or_sharing_the_project` | 同成员跨 Shell 调用恢复缓存、Git 全局配置、目录与导出；另一成员和无状态调用隔离；无密钥环境继承；项目用户文件不变；以 `/tmp` 为根的无状态调用也有独立 HOME |
| `legacy_shell_home_migrates_but_explicit_home_and_exports_survive` | 旧 HOME 自动迁移、普通导出和子目录保留、旧项目缓存原样保留、新显式 HOME 选择继续生效 |
| `node_project_installs_tests_and_packs_without_runtime_cache_files` | 本地依赖打包/安装、失败测试、文件工具修复、离线 npm ci、通过测试与构建；读取实际 tarball，确认编译文件存在、无缓存/日志/状态；复核项目目录清单 |
| `python_project_builds_and_installs_a_wheel_after_venv_resume` | venv 激活跨调用、失败测试、文件工具修复、通过测试、无下载 PEP 517 wheel 构建/安装；从 `/tmp` 导入已安装模块，避免以工作区源码冒充安装成功 |
| `workspace_mcp_home_does_not_write_its_cache_into_the_project` | 启动真实本地 stdio 夹具并调用 echo；服务缓存写私有 HOME，项目仅出现测试声明的文件，关闭服务 |

Python venv/wheel 路径在原版已经通过，本批不把它记为产品修复。Node 包没有配置 `files` 白名单或额外忽略规则，
因此测试不会通过过滤项目文件掩盖运行时写错位置的问题。最终 tarball 有八个预期项目文件，没有 `.npm`、`.cache` 或 Shell 状态。

本机实际运行版本：Python **3.14.7**、pip **26.2.1**、Node **26.9.0**、npm **12.0.2**、bubblewrap **0.12.0**。
Rust **1.95.0**。五项均实际执行，无依赖跳过。缺 bubblewrap/系统工具时回归打印 skip；Python 另要求 venv/ensurepip，
Cargo 的通过数不代表缺依赖机器也完成了此验收。

## 检查结果与测试纠错

最终定向检查 **5/5**，4.771 秒；最终 `make check` 退出 **0**，77.342 秒：
core **90**、engine **334**（108 库 + 1 CLI + 225 集成，另 **3 ignored**）、TUI **104**。
格式、全部目标严格 Clippy、原有文件/网络/恢复/批准/成员隔离回归及仓库卫生通过。
本批未改 TUI 交互，未重跑 PTY；没有新真实模型或竞品成绩。

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test toolchain_projects -- --nocapture
make check
```

保留并区分测试本身的问题：

- `red.log` 最初把 `npm pack --json` 当作数组；本机 npm 12 返回按包名索引的对象。修正为同时接受两种格式后，
  `red-corrected.log` 才在实际缓存文件清单上失败。首个类型错误不计作产品缺陷。
- 首次 `make-check.log` 的旧 MCP 单测仍断言 HOME 等于项目根。更新为独立 HOME 存在的断言，原文件、网络、
  argv 和子进程回收检查保持。其后两轮完整检查通过；最终一轮另覆盖新增的 tarball 直接读取。

## 保证边界与剩余距离

成员缓存是普通文件副作用，中断不回滚，尚无独立缓存配额，清理沿用会话保留策略。
旧项目里的运行时文件不自动删除；显式选择项目作为 HOME 仍可能产生相同文件，这是成员主动配置的结果。
系统安装的 Python/Node 已有本地构建证据，nvm/pyenv/uv/pnpm 等用户 HOME 内工具链没有因此自动变为可见。
本批没有联网安装、供应商测试、真实模型编码任务成功率或复杂 Python/Node 仓库的证据。

“达到四个成熟工具水平”仍未完成。当前最重的剩余工作是代表性仓库/语言任务的多次实跑、长任务与并行协作稳定性，
五供应商与混合团队的真实联调，以及另外三个竞品的有效对照。已有少量 Codex 同题通过样本不能代替这些证据。
累计变更还需按发布流程做整体验收；本批不提交代码、不更新历史评测成绩。
