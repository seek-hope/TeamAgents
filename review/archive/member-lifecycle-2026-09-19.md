# 成员生命周期与身份隔离（2026-09-19）

## 范围与发现

沿 D-32/D-35 检查长会话中的成员移除与复用，落实方案 §5.1、§8、T13/T24；
保留 Rust 三 crate、core 单事务权威、既有成员 ID 不可复用规则、成员私有目录和权限语义。
没有新增依赖、身份代次切换、跨会话记忆或后台终端协议，没有调用真实模型或进行竞品评测。

1. **移除后运行器未释放**：core 正确更新 TeamSpec，但 runtime 缓存未清理；原调度器查找
   `state.agents` 的 REMOVED 行无效，该接口只列当前 TeamSpec 成员。MCP/Codex 后端因此仍存活，
   多次组队/拆队会保留旧运行器与工具资源，直到整个会话关闭。
2. **MCP 只杀直接子进程**：stdio `close` 只 `child.kill/wait`，服务启动的子进程继续运行；
   若后代继承 stdout，读线程无法得到 EOF，工具调用一直等待原超时。初始化失败也走同一缺陷路径。
3. **用量探针延长生命周期**：`usage_probes` 闭包强持有运行器；单纯从 runtime 移除仍保留其私有历史。
   改弱引用后需保持 `/model` 切换、尚未创建新运行器时的用量，不能为释放内存而让计数消失。
4. **T24 原身份机制正确，缺专项证据**：Chat 以会话目录 + 成员目录 + `ctx:<id>:<epoch>` 寻址，
   Codex 外部线程存于会话库的成员行，显示名称不参与寻址。core 早已拒绝复用被删除 ID，
   本批不把它错误报告为新修复，也没有靠增长 epoch 来允许同 ID 重建。

## 实现

- `engine/src/runtime.rs`：按已提交 TeamSpec 集合判断移除；draining 成员仍在 spec 内，不提前关闭。
  等回合包装线程退出后从缓存取走运行器，在后台调用 `close`；不持有运行器/回合锁等待远端清理。
  暂停会话仍执行清理，完成的清理线程及时 join，正常退出 join 尚未完成的清理。
- `engine/src/session.rs`：用量探针用 Weak 持有后端，只缓存计数 JSON；`/model` 释放前保存最新计数，
  不再因计数引用保留后端。只读用量查询不按旧状态快照删探针，避免并发加成员后丢失其新计数入口。
- `engine/src/mcp.rs`：stdio 启动时建立独立进程组；关闭先释放 pending 调用，再 TERM/KILL 进程组并 wait。
  显式 closed 状态禁止后续工具请求；注册 waiter 与关闭 drain 同锁检查，避免关闭后新挂等待者。
  关闭幂等，绑定多个工具/析构不再重复 HTTP DELETE。移除过时的传输说明注释。
- 不修改成员工作目录、历史树、模型覆盖文件或 core 状态机，不删除归档证据。

## 可复跑证据

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml \
  --test member_lifecycle --test session_identity --test mcp_http -- --nocapture
make check
make pty
```

新增 **9 项 engine 回归**：

| 文件 / 测试 | 行为断言 |
|---|---|
| `member_lifecycle::idle_member_is_closed_and_evicted_even_while_session_is_paused` | 暂停态移除空闲成员，退出前已关闭并逐出，恰好关闭一次 |
| `member_lifecycle::removal_waits_for_boundary_and_slow_close_does_not_block_other_members` | 活动成员 WAITING_BOUNDARY 时保持可达；Leader 可在边界前、慢关闭期间继续接收输入 |
| `member_lifecycle::shutdown_joins_a_removal_cleanup_that_has_not_finished_yet` | 原子标志屏障保持旧后端 close 未完成，正常关闭不会提前返回 |
| `member_lifecycle::mcp_close_reaps_descendants_and_releases_pending_calls` | 真实 stdio 父/子进程作为正对照；子进程忽略 TERM、继承 stdout；关闭后父子停止且工具调用返回错误 |
| `member_lifecycle::failed_mcp_handshake_reaps_the_whole_process_group` | 服务启动后不响应 initialize，超时返回错误，不残留同组后代 |
| `member_lifecycle::removing_a_production_chat_member_closes_mcp_without_erasing_work` | 生产 open_session 绑定真实 MCP，移除后进程组停止、运行器 Weak 失效，原结果文件保留 |
| `session_identity::t24_chat_resume_same_name_replacement_and_new_session_are_isolated` | 真实文件工具输出→落盘→重开续用；模型覆盖续用；同名新 ID 和新会话无旧私有内容；Leader 模型请求不自动获得工具输出；旧文件/树保留 |
| `session_identity::t24_codex_resumes_only_the_same_session_member_and_retires_its_server` | 本地假 app-server 请求日志核对 thread/start/resume：同身份复用、同名替换/新会话新线程；移除停止服务，重启后仍拒复用墓碑 ID |
| `mcp_http::close_is_idempotent_and_a_closed_http_client_cannot_send_tools` | 重复 close + drop 只发一次 DELETE，关闭后的 list/call 不发网络请求 |

Chat 回归通过生产会话工厂、真实文件工具与 SQLite，而不是预置历史后只查文件。
Codex 回归将 PATH 指向测试脚本，不调用已安装的 Codex 或任何外部服务；线程 ID 随机生成，
避免固定假 ID 掩盖身份串用。HTTP 假模型有停止信号并 join；进程探针有失败清理，
用 `/proc` 排除 zombie 假阳性，环境均持有 `support::TestEnv`。

### 失败到通过

- 第一版探针漏 `base_revision`，补丁被核心正确拒绝；先修正探针，不作为产品缺陷证据。
- 修正后、改 runtime 前，两项移除检查分别失败于“退出前仍存活”和“边界后未启动清理”。
  日志 `/tmp/teamagents-member-before.log`。
- 只修 runtime 后，三项实际 MCP 检查仍失败于后代存活；父进程关闭不代表进程树关闭。
  日志 `/tmp/teamagents-member-process-before.log`。
- 弱引用初版使 `/model` 空档用量变 null，身份回归捕获后补最后计数缓存；不是旧版本身份串用。
  日志 `/tmp/teamagents-identity-before.log`。
- 最终专项与全部质量门禁通过，见 `/tmp/teamagents-member-targeted.log`、
  `/tmp/teamagents-members-check.log`、`/tmp/teamagents-members-pty.log`。这些为本机临时日志，
  长期复现以仓库测试命令为准。

## 验证与边界

- 本批生命周期修复结束时运行 `make check`：格式、strict Clippy、全部回归、卫生通过；当时为 core **63** /
  engine **268** / TUI **99**。随后成员持久记录浏览批次新增 3 项 engine 与 5 项 TUI 回归，当前基线见
  `docs/ACCEPTANCE.md`（core **63** / engine **271** / TUI **104**）。
  engine 另 1 显式 ignored；`live_codex` 未设开关提前返回，不能计为真实服务通过。
- `make pty`：输入/恢复、鼠标命中、工作区审查三项无模型真终端检查通过。
- `member_lifecycle` 与 `session_identity` 合计 8 项又连续重复运行 3 次，均通过；
  日志 `/tmp/teamagents-member-repeat-{1,2,3}.log`。
- 本批身份测试走正常退出/重开，不代替 T8 已有 kill -9 恢复检查；没有扩大模型上下文配置或供应商宣称。
- stdio 进程组回收面向本项目 Linux 运行范围；显式 host 服务主动 setsid/脱离进程组不在保证内。
  HTTP 已发操作不能撤销，在途请求仍受原超时约束；某个后端 close 永久阻塞时，正常退出仍可能等待。
- removed 的后台清理非数据库事务的一部分；kill -9 期间没有“所有宿主进程必清理”的新保证。
  异步清理不会删除已有成果，也不把清理完成冒充任务交付成功。
- 已移除成员的模型 profile/覆盖与私有文件仍保留；弱用量探针及其最后计数可能保留到会话关闭，
  但不再强持有后端对话树、MCP 客户端或运行器。
- T24 现有专属本地证据；后续批次已补齐有明确范围的本地持久记录浏览，但 Codex 完整外部历史/工具记录与
  内部推理仍不由 TeamAgents 接管。整体成熟度目标仍未完成：私有子代理、五家真实闭环、长任务/复杂仓库完成率
  与公平竞品对照仍需推进。
