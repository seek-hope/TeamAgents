<!-- item_2 -->
I'll start by exploring the repo structure and locating the session/fork call chain.

<!-- item_43 -->
Now I understand the call chain. Let me patch `session.rs` first (thread helper, fork file copy, override validation).

<!-- item_46 -->
Now the worker: transactional `open`, fork rewrite.

<!-- item_50 -->
Builds. Running the required test suite now.

<!-- item_61 -->
Now adding regression tests inside the two allowed files.

<!-- item_70 -->
Let me verify the protocol path end-to-end with a real `serve` process (probe in /tmp).

<!-- item_73 -->
Let me verify the fork-failure cleanup path with a test in `worker.rs`.

<!-- item_81 -->
Let me review the final diff regions for correctness and style.

<!-- item_85 -->
Let me strengthen the fork test to cover a second member with an auto-created profile (the reported scenario).

<!-- item_107 -->
修复完成。以下是行为变化、修改位置与验证结果。

**行为变化**
- `fork_session`（`engine/src/worker.rs:220`）现在：先复制源会话的 `profiles.json` 与 `model_overrides.json`（必须在新会话 `open` 之前，因为自动 profile 只存在于源会话，否则新会话的 TeamSpec 解析不到它）；新会话打开后按它自己的 `context_epoch` 把 Leader 当前线程 `ctx:<leader>:<src_epoch>` 重映射到 `ctx:<leader>:<dst_epoch>`，再复制 `chat_tree.json` 与旧版线性 `chat_history.json`（分支/节点/leaf 原样，其它 epoch 线程保留，源文件只读）。任务、运行记录、共享事实、成员私有历史、Shell 状态与项目工作文件都不继承/不回滚。fork 还镜像源会话的权限模式。
- `open`/`switch_session`/`new_session` 改为事务式（`engine/src/worker.rs:62`+`:78`）：新会话完全就绪后才关闭旧会话；打开失败时旧会话保持打开，`cwd`/团队配置/权限模式不变（原先先 `close_current` 再打开，失败即失去会话）。重复打开当前会话 id 直接复用，不再与自身锁冲突。
- 失败 fork 只删除本次新建的目标目录，不归档/删除源会话；有 QUEUED/RUNNING 回合仍拒绝 fork（`进行中`），不取消任何成员。
- `load_model_overrides`（`engine/src/session.rs:178`）改为同时校验会话级 profile，`/model` 指向自动 profile 时重开后不再被丢弃（源会话与 fork 都受益）。
- 协议未变：响应字段与错误回执形状不变，未加命令/依赖。

**修改位置**
- `engine/src/worker.rs`：`adopt` `:62`、`open` `:78`、`fork_session` `:220`、`session_info` `:296`，以及新增 4 个单测。
- `engine/src/session.rs`：`write_json_atomic` `:104`、`copy_session_model_config` `:118`、`copy_leader_history` `:137`、`remap_thread_key` `:160`、`load_model_overrides` `:178`、`leader_thread` 设为 `pub` `:279`，以及新增 1 个单测。
- 仅这两个文件被改动（mtime 核对其余文件均为快照时间），无新增文件，`.config/teamagents/config.toml` 与 Cargo 清单/锁未动。

**测试结果**
- 本次新增：`worker::tests::{a_failed_open_keeps_the_current_session_usable, fork_inherits_model_config_and_remaps_the_leader_history, a_failed_fork_cleans_up_and_keeps_the_source_session, fork_is_refused_while_a_member_turn_is_running}`、`session::tests::fork_copy_remaps_the_leader_thread_and_leaves_the_source_alone` 全部通过。
- 指定命令：`fork_rewind` 1/1、`model_override` 4/4、`session_boot` 2/2 通过；`worker_protocol` 4/5，唯一失败是 `model_discovery_does_not_block_worker_requests`（`tests/worker_protocol.rs:217` 的 `TcpListener::bind` → `PermissionDenied`）——本沙箱禁止回环 bind（`python3` 直接 bind 同样报 `Operation not permitted`），与本次改动无关且该测试文件未改。
- 全量：engine lib 57 通过 / 7 失败（全部为 TCP bind / bwrap 沙箱限制）；core 50/50；tui 51 通过，`render_tests` 12 个因快照缺少 `review/tmp/parity_scenario.json` 夹具而失败（tui 未改）。
- 额外在 `/tmp` 用真实 `serve` 进程做了协议级探针：非 1 epoch + 只有旧版线性历史的源会话 fork 后映射为 `ctx:leader:1`、源文件字节不变；只有源会话才有的自动 profile 与 `/model` 覆盖在新进程重开后报告完全一致；失败 switch 后旧会话仍可用；fork-of-fork 正常。
