# 开发与维护

本项目保持 `core`、`engine`、`tui` 三个 Rust crate。产品约束见 [实施方案](../TeamAgents-Implementation-Plan.zh-CN.md)，
已确认决策见 [DECISIONS](DECISIONS.md)，真实服务验收边界见 [ACCEPTANCE](ACCEPTANCE.md)。

## 环境与统一入口

在仓库根目录工作。`rust-toolchain.toml` 固定 Rust 版本、Clippy 与 rustfmt；本机和两个 GitHub
工作流读取同一份版本配置。通过 rustup 安装 Rust 后，首次运行 Cargo 会准备所需工具链。
Linux 隔离检查还需要可工作的 bubblewrap；真终端检查需要 Python 3。

```bash
make check CARGO_FLAGS=--locked   # 首次允许下载依赖，保持锁文件不变
make check                        # 后续默认 --offline --locked
make fmt                          # 应用统一格式
make build                        # 构建 CLI 与 TUI，使用原有 target 路径
make pty                          # 独立配置/状态、无模型凭据的真终端检查
```

`make check` 依次检查格式、全部目标的 Clippy、三个 crate 的测试、Git 子模块配置、已跟踪缓存与安装脚本语法。
Clippy 警告视为错误。CI 使用相同的 Make 目标，仅允许联网下载锁定的依赖；发行工作流使用相同的 Rust 版本。
Cargo 报告 passed 可能包含缺少依赖时提前返回的测试，不能代替真实隔离或真实模型验收。

局部开发仍直接使用 Cargo，缩短反馈时间：

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test model_override
cargo test --offline --locked --manifest-path core/Cargo.toml --test engine topology_patch_rejects_
cargo test --offline --locked --manifest-path engine/Cargo.toml --test cli --test session_boot
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e worker_environment
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e topology_
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib prepared_topology_
cargo test --offline --locked --manifest-path engine/Cargo.toml --test history_protocol
cargo test --offline --locked --manifest-path engine/Cargo.toml --test codex_recovery
cargo test --offline --locked --manifest-path core/Cargo.toml --test output_references
cargo test --offline --locked --manifest-path core/Cargo.toml --test task_boundaries
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e task_requests_can_be_corrected
cargo test --offline --locked --manifest-path core/Cargo.toml --test action_requests
cargo test --offline --locked --manifest-path core/Cargo.toml --test stored_integrity
cargo test --offline --locked --manifest-path core/Cargo.toml --test delivery_acl
cargo test --offline --locked --manifest-path engine/Cargo.toml --test recovery
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e persisted_work_is_checked
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e communication_tools_correct_refusals
cargo test --offline --locked --manifest-path engine/Cargo.toml --test worker_protocol worker_user_input_rejects
cargo test --offline --locked --manifest-path engine/Cargo.toml --test private_context
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e masked_history_readback
cargo test --offline --locked --manifest-path engine/Cargo.toml --test live_models
cargo test --offline --locked --manifest-path engine/Cargo.toml --test tools_sandbox
cargo test --offline --locked --manifest-path engine/Cargo.toml --test toolchain_projects
cargo test --offline --locked --manifest-path tui/Cargo.toml --test render_tests
cargo test --offline --locked --manifest-path tui/Cargo.toml --test history_tests
```

## 改动应放在哪里

| 变更 | 所属位置与约束 | 优先回归 |
|---|---|---|
| 团队动作、任务状态、调度、权限 | `core/control.rs` 经 `Control::submit` 的单事务路径；`storage.rs` 持久化、`views.rs` 可见性 | `core/tests/engine.rs`、`engine/tests/scenarios.rs` |
| 持久业务记录完整性 | 任务/回合列表、事件/投递 JSON、批准 scope 解码错误向上传播，不能默认空状态；视图/调度/唤醒和批准过期失败回滚外围事务。会话启动先预检、后切权限及构建成员 | `core/tests/stored_integrity.rs`、`delivery_acl`、`chat_e2e::persisted_work_is_checked_before_session_start_and_resumes_after_repair`；保留原始值、故障解除后恢复 |
| 通信、共享读取与用户控制请求 | core 校验外层字段类型与 JSON 对象形状；`shared_entries_after` 按空间游标合并分页，读取与游标推进同事务，列表使用实际聚合。worker 在转交用户输入前检查文本与布尔补充标记 | `core/tests/action_requests.rs`、`chat_e2e::communication_tools_correct_refusals*`、`worker_protocol::worker_user_input_rejects*` |
| 任务请求与迟到结算 | 五个任务工具使用严格请求类型；Control 按 session 读取任务/回合，begin/finalize 复核承接者和状态，结算不复活终态任务或已移除成员、不覆盖成员新回合状态；读/写错误使事务回滚 | `core/tests/task_boundaries.rs`、`chat_e2e::task_requests_can_be_corrected_through_model_tools_and_survive_reopen` |
| 共享附件与任务成果引用 | `core/references.rs` 检查引用类型、私有标识及本地目标；动作提交和完成结算复核，保留审计、错误不半结算 | `core/tests/output_references.rs`、`engine/tests/private_context.rs` |
| 成员执行、取消、恢复与移除 | `engine/runtime.rs`；具体 Chat/Codex 协议留在各自适配器，已移除运行器在安全边界后异步关闭 | `recovery`、`chat_e2e`、`codex_contract`、`codex_recovery`、`member_lifecycle`、`session_identity` |
| 回合结果归档 | `Runtime` 保留已返回结果及投递确认；`finalize_run` 失败只重试事务，成功在事务内返回 `applied/status`，通知以该次提交为准，无变更不发旧结果。Chat 保留迟到取消前的已知终态；冷恢复先修复历史提交日志，再直接返回终态检查点 | `recovery` 的 `chat_*`、`finalization_*`、`returned_chat_outcome_survives_cancellation_*`；`chat_e2e::review_tree_commit_recovers_on_both_sides_of_rename`，见[终态恢复](../review/repl-finalization-2026-09-19.md)和[通知检查](../review/recovery-state-2026-09-19.md) |
| 旧排队回合恢复 | Runtime 先识别检查点/外部回合 ID/数据库开始事件，core `restore_queued_runs` 在一个事务中将整批已有执行的 QUEUED 恢复为 RUNNING，再核对结果；新意图保持 QUEUED，缺失或损坏检查点按未知结果处理。首次输入、恢复等待时输入及重开时的模式变更先做同一准备 | `task_boundaries` / `stored_integrity` 的 `queued_recovery_admission_*`，`recovery` 的 `returned_chat_outcome_survives_a_legacy_queued_cancellation` / `queued_*`；见[通知与准备](../review/recovery-state-2026-09-19.md)和[证据保留](../review/recovery-evidence-2026-09-19.md) |
| 运行时存储错误 | core `prepare_run` 原子读取启动输入；Runtime 保留原执行意图/返回结果并重试，`runtime_errors` 仅作瞬态诊断。CLI 先确认输入回执再启动，有错误时不运行交付检查；TUI 去重显示错误和恢复 | `stored_integrity::run_preparation_*`、`recovery` 中的 `returned_chat_*` / `unreadable_member_*` / `exec_reports_*`，以及 `render_tests::storage_wait_*`；证据与范围见[记录](../review/runtime-storage-2026-09-19.md) |
| plain 输入与故障反馈 | CLI 先检查 `Receipt.ok`，失败不等待执行；通过 Runtime 的单次等待观察传出读取错误，存储诊断使行模式及时回到命令输入。`status` 显示新增事件，后台仍按既有策略重试；原 `settle` 的限时等待行为保持 | `recovery::plain_rejects_uncommitted_input_and_accepts_a_new_request_after_repair`、`plain_storage_failure_returns_control_and_recovers_without_resubmitting_input`，见[记录](../review/repl-finalization-2026-09-19.md) |
| 运行中消息交接 | core `drain_mid_turn_pushes` 原子取得接收成员和当前投影，成功后排空；进程内客户端使用类型化结果，Runtime 串行交接并周期重试。后端注入/确认仍走既有权限与投递账本 | `delivery_acl::failed_mid_turn_batch_*`、`recovery::mid_turn_input_retries_after_*` / `member_tool_messages_*`；恢复核对另有 `chat_cold_recovery_retries_requeue_failure_without_replaying_tools`，见[记录](../review/mid-turn-storage-2026-09-19.md) |
| 排队取消与输入范围 | core `cancel_inactive_run` 在调用者事务中结清无外部回合的执行意图和批准；取消整个回合丢弃其未消费输入，取消任务只丢弃带该 `event_task_id` 的待投递 `task_ready`。旧取消标记经 schedule 结清；外部回合保持停止确认 | `engine::cancelling_*` / `queued_external_turn_*`、`stored_integrity` 的取消与回滚检查；`recovery::unstarted_member_*`、`returned_chat_outcome_survives_read_failure_across_close_and_sigkill`，见[记录](../review/queued-cancellation-2026-09-19.md) |
| 模型配置、会话启动 | `engine/config.rs`、`session.rs`、`sessions.rs` | `cli`、`session_boot`、`model_override` |
| 组队准备与结构校验 | `gateway` 准备前复用 core 的身份/请求/版本校验；`session` 局部准备 profile；`Control::submit` 按原请求记录回执、在同一事务中校验准备后的操作；已有提案按 session 读取，拒绝非法变更与成员广播 | `chat_e2e::topology_*`、`core/tests/engine.rs::topology_*`、`gateway::tests::prepared_topology_*`；失败修正、恢复、提案批准、去重与竞态 |
| TeamSpec 结构与恢复 | `core/models.rs::TeamSpec::validate` 为统一结构校验，持久读取保留旧 limits 兼容后也须校验；`session_metadata` 只报告会话/配置是否存在，已有非法配置不得由启动参数覆盖 | `core` 库单测、`core/tests/engine.rs::topology_patch_rejects_*`、`cli`、`session_boot` |
| 工具执行、隔离、批准 | `engine/tools.rs`、`gateway.rs`；自动输出按成员归属，主动制品按会话共享；已绑定 MCP 经 `bound.rs`/`mcp.rs` | `private_context`、`tools_sandbox`、`mcp_*`、批准回归 |
| 工具缓存与语言项目 | Shell 默认 HOME 在成员 `shell/home/`；旧默认 HOME 迁移但不移动项目文件。工作区 MCP 使用临时私有 HOME，宿主模式保持原授权 | `toolchain_projects`：成员隔离、旧状态、自定义导出、Node 安装/测试/打包、Python venv/wheel、MCP 缓存位置 |
| 工作区与实际变更审查 | `engine/workspace.rs`、`review.rs`；TUI 只经 worker 读取证据，不用工具输出代替文件 | `workspace_lifecycle`、`workspace_review`、`workspace_review_protocol` |
| 成员持久记录浏览 | `engine/history.rs` 只读读取会话数据库与成员文件；`engine/codex/history.rs` 通过独立客户端读取成员原生历史，必要时受限读取 rollout；TUI `history.rs` 只负责用户导航，不把记录送入模型 | `history_protocol`、`tui/tests/history_tests.rs`、`make pty` |
| Chat 历史恢复与回退 | `engine/chat.rs` 验证持久树及旧线性历史，损坏时保留证据并阻止模型调用；线性祖先遍历不删除废弃分支；session/runtime 传播回退错误 | `chat::tests`、`session_identity`、`chat_e2e::compaction_after_empty_rewind_keeps_the_new_branch_root_across_restart` |
| TUI 操作、显示、鼠标 | `tui/app.rs` 状态与动作、`review.rs` 审查交互、`ui.rs` 渲染；布局和点击复用 `geometry` | `app_tests`、`render_tests`、`review_tests`、`make pty` |
| 安装与发布 | Rust `init`、`install.sh`、`.github/workflows/release.yml` | `engine/tests/install.rs`、`cli`、发行制品冒烟 |

表中 Rust 路径均相对各 crate 的 `src/`。TUI 通过 worker 协议访问引擎，不直接读取数据库。
回调与队列类型在所属模块命名，例如 `TopologyPrepare`、`RunnerCache`、`PendingReplies`，避免跨文件复制复杂签名。
抽取模块应围绕独立职责和实际变更频率；不为消除单个 lint 创建只有一处使用的框架。

## 稳定的回归测试

优先在子进程上通过 `Command::env` 设置测试环境。必须改当前进程环境的 engine 集成测试，
使用 `engine/tests/support/env.rs` 的 `TestEnv`：

```rust
let mut env = support::isolated_state_home("descriptive-test-name");
env.set("TA_TEST_OPTION", "value");
let project = env.join("project");
// Create the runtime after env; close it and join workers before env is dropped.
```

该对象持有测试二进制内的互斥锁，设置独立的 XDG 配置/状态目录，退出时恢复环境变量并清理目录，
包括 panic 展开路径。必须保存返回值到局部变量；不要写 `let _ = ...` 或在辅助函数内提前丢弃。
额外环境变量用 `env.set`，不要另写 `std::env::set_var`；需要返回运行时的辅助函数应一并返回该对象。
engine 库单测修改进程环境时继续遵循现有 `crate::env_lock()` 约定。
生产 `open_session` 的测试项目必须与配置/状态目录分开，例如同一临时根下使用
`project/`、`config/`、`state/` 三个子目录；不要把包含 XDG 状态的整个临时根或 `/tmp` 作为共享工作根。
`private_context` 专门覆盖重叠拒绝，其他恢复、分叉、取消测试应使用有效的目录布局。

假服务应先读请求再回响应，避免响应先于 pending request 注册。并发断言优先使用屏障或通道，
关闭并等待所有线程、子进程后再销毁环境；不要依赖测试名称顺序或本机用户配置。
`test_environment` 回归验证变量恢复、非 Unicode 值、重复设置、panic 清理以及锁中毒后的复用。

`toolchain_projects` 通过真实 bubblewrap 执行系统 Python/Node 工具链，缺 bwrap 或对应系统工具时明确打印 skip；
Python 还要求系统解释器包含 `venv`/`ensurepip`（部分发行版需单独安装 venv 支持）。本机通过记录应确认没有 skip。
测试全部离线：Node 使用本地包，Python 使用测试内的 PEP 517 构建后端，不下载依赖、不继承宿主包管理器凭据。
它们验证工具执行与交付文件，不计作真实模型的编码成功率。

恢复与历史清理共享 `run_started` 证据校验。`stored_integrity` 覆盖千条以上事件读取、
十三类损坏/错误引用和跨删除批次回滚；`recovery` 覆盖丢失检查点后的真实进程重开及自动保留策略，
`codex_recovery` 覆盖旧排队回合缺少外部 ID。它们均使用本地夹具，证据和边界见
[恢复证据记录](../review/recovery-evidence-2026-09-19.md)。

## 代码检查与评审

- 统一使用根目录的 `rustfmt.toml`；大批格式调整与行为变更分别提交，便于查看实际逻辑差异。
- 不在 crate 根关闭 lint。确有必要保留的现有参数较多的接口，用函数级 `#[expect(..., reason = "...")]`
  解释原因；失去触发条件的 expect 也会在严格检查中报错。
- 文件打开明确选择保留、追加或截断；锁文件保持原有 inode 和内容，不因清理 lint 改成截断。
- 新逻辑以正常、失败和恢复行为验证；纯搬移或格式化复用既有回归，不增加只镜像实现的测试。
- `review/tmp/` 是被忽略的探针与临时制品目录。有效结论放到有日期的 `review/*.md`，正式评测证据
  放在 `review/eval/runs/`；不再提交 Python 缓存、临时数据库、嵌套 Git 仓库或整份旧源码备份。

## 依赖、工具链与发行

三个 Cargo.lock 各自提交，常规检查和发行都加 `--locked`。新增依赖先检查标准库和已有依赖能否满足需求；
升级依赖时只更新相关锁文件，并重跑受影响的接口检查及 `make check`。
升级 Rust 只修改 `rust-toolchain.toml` 的版本，再运行 `make fmt`、`make check` 与 `make pty`；CI/发行自动读取新版本。

发布前同步三个 crate 的版本及其锁文件、更新发行说明、通过 CI，再推送 `vX.Y.Z` 标签。
发布后从公开地址校验 SHA-256，验证安装、`init` 保留配置和实际 TUI 启动。
真实模型评测需明确配置凭据与模型原生上下文，单独记录；维护性回归不能宣称扩大供应商兼容范围。

固定仓库任务使用 `review/eval/tasks/<id>/fixture-source.toml` 记录完整 Git 提交与输入路径，
以 `fixture/` 显式叠加用户输入；不读取当前脏工作树，不把隐藏测试或参考修复放入模型目录。
多 crate 任务用 `grading.toml` 指定清单、公开 suite 与评分时限。候选只在原有 bubblewrap 内
离线构建和运行；受保护文件的内容及权限在最终候选上核对，不能称作全过程文件审计。
历史夹具需要用户式配置时，可在 `grading.toml` 显式设置 `config_home = ".config"`；该目录必须来自
固定输入，不能是绝对路径、父路径或单个文件。评分命令将其映射成评分副本内的绝对 `XDG_CONFIG_HOME`，
保持私有 HOME，不读取宿主配置。提示词和 `checks.txt` 也须显式使用同一夹具配置。
`repo-session-fork` 已按此修订测试环境；提示词/评分哈希变化后的样本单独统计，历史成绩原样保留。
新增或修改评测入口后运行 `bash review/eval/check-runner.sh`；当前覆盖 30 项契约，
包括固定输入准备失败时禁止启动模型，以及严格 `umask` 下复制输入仍保留文件权限。
任务集及复跑命令见[评测说明](../review/eval/README.md)。

大型仓库任务的构建输出可能占数 GB。先用 `df -h /tmp .` 核对文件系统；本机 `/tmp` 是
16GB tmpfs，不能用主磁盘的空闲量推断它也有足够空间。可将新评测输出目录放在被忽略的
`review/tmp/`，并令 `TMPDIR` 指向同磁盘上的独立目录，使评分器临时副本也使用该磁盘。
工作区、配置和状态应为同一评测根下的独立子目录，继续保留原有 bubblewrap 和隐藏测试隔离。
已有证据不自动清理；运行期间不修改输入、评分或候选，不因换存储位置把历史失败改记成功。
实际完整通过的例子与复跑命令见[仓库任务记录](../review/eval/runs/2026-09-19-repo-current/REPORT.md)。

竞品 CLI 若能读取工作区外的文件，仅把隐藏测试放在工作区之外不足以隔离验收。
模型启动前，应在独立文件系统视图中只挂载公开输入、必要工具链及独立状态目录，
用无模型探针确认隐藏测试、当前开发仓库和旧候选不可读，同时确认工作区可写、CLI 自身沙箱仍生效。
旧快照的无凭据测试配置也须在该视图中可用。若模型已经读到隐藏材料，终止并保留污染轨迹，
不计入能力完成率；修正隔离后使用全新输入和会话，不能续接已污染上下文。

## 真实 Chat 模型矩阵

`engine/tests/live_models.rs` 使用生产 `open_session`、ChatRunner、文件工具与 bubblewrap Shell，
依次检查工具结果续接、同会话继续、关闭并销毁会话对象后重新打开。随机输入只通过第一次
`read_file` 提供给模型；每阶段之后移除输入及输出并断言工作目录为空，
后续阶段从同会话历史恢复随机值。
独立检查实际文件内容、成功工具回执、回合终态、最终标记与用量账本。这里的恢复在同一测试进程内
重建会话，不替代 SIGKILL、执行中恢复或重复副作用验收。

默认 Cargo/`make check` 只运行五项本地契约测试，真实入口显式 `ignored`。先按
[清单示例](../review/eval/live-models.example.toml)填写要验收的 profile、精确模型名、
原生上下文长度及可复核来源，然后显式执行：

```bash
TEAMAGENTS_LIVE_MODELS_CONFIG="$HOME/.config/teamagents/config.toml" \
TEAMAGENTS_LIVE_MODELS_MANIFEST="$PWD/review/eval/live-models.example.toml" \
TEAMAGENTS_LIVE_MODELS_EVIDENCE="/tmp/teamagents-live-models-new" \
cargo test --offline --locked --manifest-path engine/Cargo.toml --test live_models \
  live_chat_model_matrix -- --ignored --exact --nocapture
```

示例仅适用于 DeepSeek Flash；其他模型先核实原生窗口，再增加 `[[models]]` 条目，不套用 1M。
`TEAMAGENTS_LIVE_MODELS_CONFIG` 可省略，此时读取当前 XDG 用户配置；其他两个变量必须提供。
证据目录必须尚不存在且父目录可写。每个 profile 单独运行，`phase_timeout_s` 为每阶段时限
（默认 600 秒，范围 1–3600），请求超时和重试仍沿用该 profile。

配置中已有的窗口必须与清单一致；未配置窗口时仅使用清单的显式声明，报告同时记录原值和有效值。
驱动不更换模型或覆盖生成参数。仅导入选中的完整模型 profile，隔离项目与 XDG 配置/状态，
不导入其他工具绑定、MCP、hooks、Skills 或指令文件；会话关闭后才释放隔离环境。
原生窗口来源由执行者核实，入口只检查必填项及一致性，不自动认证来源真实性。

报告 `report.json` 随阶段原子更新，包含配置指纹、模型/协议/档位、原生窗口来源、状态、
成功/失败工具次数、用量与行为断言。端点、任意生成参数、模型原文、工具参数及原始错误响应不入报告。
缺 profile/凭据记 `skipped`，不明确或冲突的模型/窗口记 `blocked`，行为失败记 `failed`；
后续条目继续执行。只有清单中所有条目实际通过才返回成功，任何跳过或失败都会令命令非零退出。
`passed` 只代表本次清单，不表示五家供应商、同队混用或全部工具生态已经验收。
首批真实结果及边界见[记录](../review/live-models-2026-09-19.md)。

## 真实 Codex 恢复

真实 Codex 恢复检查使用隔离的工作目录、XDG 状态和 CODEX_HOME；从选定的本机 Codex 配置读取
模型/供应商，认证仍取环境变量或本机凭据，不加载其他 MCP、hooks 或项目记录。配置未提供
`model_context_window` 时必须显式提供模型原生窗口；DeepSeek Flash 按用户确认的 1M：

```bash
TEAMAGENTS_LIVE_CODEX=1 \
TEAMAGENTS_LIVE_CODEX_CONFIG="$HOME/.codex/deepseek.config.toml" \
TEAMAGENTS_LIVE_CODEX_CONTEXT_WINDOW=1000000 \
cargo test --offline --locked --manifest-path engine/Cargo.toml --test live_codex -- --nocapture
```

该检查验证原生工具操作、同一外部线程的冷恢复、任务完成归档及副作用不重做。嵌套工具沙箱若导致
Codex 的 bubblewrap mount-lock 报只读错误，应按执行环境权限流程在外层沙箱之外运行，
仍保留 Codex 自身的 workspace-write/on-request 设置；不要将待批准或超时记作通过。

## Codex 原生历史浏览检查

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test history_protocol
cargo test --offline --locked --manifest-path tui/Cargo.toml --test history_tests
```

上述协议检查使用本地夹具，不需要模型凭据。原生来源只按数据库中对应成员的线程 ID 读取，
不得为浏览而恢复线程或启动回合。首次分页接口明确不支持时才降级；权限拒绝、传输错误或坏数据不得触发降级。
JSONL 路径限于 `CODEX_HOME/sessions` 与 `CODEX_HOME/archived_sessions`，各级目录不得是符号链接；
首条线程元数据必须匹配。文件和单响应有 32 MiB 上限，文件读取期间继续处理取消与超时。
声明非 legacy 模式的 rollout 不得用作回退：分页存储可能只在该文件保存元数据。

升级 Codex 后用所选 CLI 生成实验 Schema，核对 `ThreadItemsListParams/Response` 与 `ResponseItem`。
后者 ID 可以缺省/null，不能沿用 `ThreadItem` 的必填 ID 假设。也不能用刚建线程的“不支持”响应
推断重新打开的持久线程行为。0.155.0 的本机无模型联调、人工夹具范围、日志及复跑命令见
[原生记录检查](../review/native-history-2026-09-20.md)。
