# TeamAgents 会话分叉 / 切换修复报告

仓库：`repo-session-fork`（core / engine / tui 三个独立 crate，离线 Cargo）
改动范围：仅 `engine/src/worker.rs`、`engine/src/session.rs`（含这两个文件内的局部测试）。
其他源码、Cargo 清单与锁文件、既有测试、文档、`.config/teamagents/config.toml` 未改动，未新增文件。

## 1. 定位到的调用链

- 协议入口：`engine/src/worker.rs::Worker::handle` 的 `open` / `switch_session` / `new_session` /
  `fork_session`（JSON-lines，`serve()` 主循环）。
- 会话打开：`engine/src/session.rs::open_session`（锁 → 归档检查 → `create_session`/`save_spec` →
  载入 `profiles.json`、`model_overrides.json` → 建 runner → `runtime.start()`）。
- 会话文件布局：`sessions/<id>/{team.db,profiles.json,model_overrides.json,members/<agent>/{chat_history.json,chat_tree.json,turns/…}}`。
- 对话线程：`ctx:<agent>:<context_epoch>`（`core/src/control.rs::context_ref`；
  `OpenedSession::leader_thread()` 从 `state.agents[].context_epoch` 读取，Leader 重置上下文后 ≠ 1）。
- 分叉旧实现只把源会话 `members/<leader>/chat_tree.json` 原样拷到目标目录，其余一概不管。

## 2. 行为变化（对应 5 条要求）

1. **fork 继承完整会话配置**（`worker.rs:251` fork 分支）
   - 先 `copy_fork_model_state`（`session.rs:109`）把源会话的 `profiles.json`（D-30 会话级自动 profile）与
     `model_overrides.json`（D-29 `/model` 覆盖）拷入目标目录，**再**打开目标会话 —— 否则"只在源会话存在"
     的 profile 会让目标 open 直接以 `unknown model profile` 失败。
   - TeamSpec 仍走 `state.spec` → `initial_spec`；分叉后 `model` 报告的 profile/模型/思考档位与源会话一致，
     关闭后重新 `open` 仍一致（`model_overrides.json` 落盘）。
   - 附带小修（`session.rs:176 load_model_overrides`）：覆盖项的 profile 也在**本会话 profiles.json** 里解析，
     使"选中的会话级 profile + /model 档位"在重开时不再被丢弃（这正是"重开后仍相同"的前提）。
2. **按 `ctx:<leader>:<epoch>` 映射到新会话自己的线程**（`session.rs:134/158`）
   - 目标会话打开后取它自己的 `leader_thread()`，把源会话的活线程键改写到目标键上；树的全部分支、节点、
     `leaf`、`rewind_epoch` 原样带过去，其它线程键保持不动；旧线性 `chat_history.json` 按同一规则映射。
   - 源文件只读不写（探针逐字节比对）。
3. **只带 Leader 对话，不带团队事实**：新会话是全新 `team.db`，任务/回合/共享事实/成员私有历史/
   `turns/` 检查点/Shell 状态一律不复制（`copy_fork_leader_history` 只碰 leader 的
   `chat_tree.json` 与 `chat_history.json`）；项目工作文件既不复制也不回滚。
4. **失败不伤旧会话**（`worker.rs:80/119/139/148`）
   - `open` 拆成 `request`（只解析）→ `open_request`（只建新会话，不碰当前会话）→ `activate`（成功后才
     `close_current` 并更新 cwd/full_auto/team）。因此 `open`/`switch_session`/`new_session` 失败时，
     旧会话、工作目录、团队配置、权限模式原样保留。
   - 切到"当前已打开的那个会话"退化为无副作用 no-op（不会再因为自持锁而报错，也不会把会话关掉）。
   - fork 失败时：关闭本次打开的目标会话（若已打开），并在**目标目录是本 fork 新建**的前提下
     `remove_dir_all` 清掉未完成目录；预先存在/别人的目录不动，源会话不删不归档。
5. **进行中的回合仍拒绝 fork**：保留原有 `state.runs` 中 `QUEUED/RUNNING` 检查与中文错误回执，
   fork 不做任何取消动作；协议、命令集、依赖均未增加。

## 3. 修改位置

| 文件 | 位置 | 内容 |
| --- | --- | --- |
| `engine/src/session.rs` | 103–170 | 新增 `copy_fork_model_state`、`copy_fork_leader_history`、`copy_live_thread`（fork 的文件层） |
| | 172–200 | `load_model_overrides` 增加 `session_profiles` 参数，覆盖项可在会话级 profile 上解析 |
| | 277 | `OpenedSession::leader_thread` 改为 `pub`（worker 复用，避免重复实现 epoch 逻辑） |
| | 731 | `load_model_overrides` 调用点传入会话 profiles |
| | 1376–1409 | 新增局部测试 `fork_moves_the_live_thread_and_leaves_the_source_file_alone` |
| `engine/src/worker.rs` | 31–44 | 新增 `OpenRequest` |
| | 69–160 | 新增 `descriptor`/`request`/`open_request`/`activate`，`open` 改为"先建后切" |
| | 251–320 | `fork_session` 分支重写（配置先行 → 打开 → 线程映射 → 失败清理） |
| | 366–628 | 新增局部测试 3 支（协议层 fork/失败保活/忙时拒绝） |

## 4. 验证

### 4.1 指定命令（离线）

```
cargo test --offline --manifest-path engine/Cargo.toml --test fork_rewind --test model_override \
  --test session_boot --test worker_protocol -- --test-threads=1
```
`fork_rewind 1 / model_override 4 / session_boot 2 / worker_protocol 5` —— 12/12 全绿。

### 4.2 新增局部测试（`--lib`，63 项全绿，含新增 4 项）

- `worker::tests::fork_carries_model_state_and_the_conversation_onto_the_target_thread`
  （会话级 profile + `/model` 覆盖 + epoch=3 的树/线性历史 → fork → 同一 model 报告 → 重开仍一致）
- `worker::tests::a_failed_switch_or_fork_keeps_the_current_session_alive`
  （被占用会话的 switch 失败、坏配置导致 fork 失败 → 旧会话/目录/权限模式不变、无残留目录、修好后再 fork 成功）
- `worker::tests::fork_is_refused_while_a_member_turn_is_active`（`sleep` 脚本回合运行中拒绝 fork 且不取消回合）
- `session::tests::fork_moves_the_live_thread_and_leaves_the_source_file_alone`（线程键改写、源文件只读、坏文件按原样拷贝）

### 4.3 真实 `serve` 进程探针（JSON-lines，端到端，44 项检查全过）

以 `engine/target/debug/teamagents serve` 真进程为对象，覆盖：继承 spec/profiles/overrides、epoch 3→1 的
树与线性历史映射、源文件 sha256 不变、无任务/回合/共享事实、成员私有历史与 `turns/`/shell 状态不继承、
项目工作文件 sha256 不变、`rewind_points` 证明分叉后对话在其自有线程上可继续、跨进程持锁的 switch 失败后旧会话
仍在位（session/cwd/permissions_mode 不变）、坏配置令 fork 失败后 sessions 目录集合逐项不变、忙团队拒绝 fork 且
回合未被取消、`list_sessions` 同时列出两个会话。

### 4.4 全仓测试现状（与本改动无关的既有环境性失败）

- `core`：50/50 绿；`engine`：除下列两例外全绿（lib 63、chat_e2e 24、scenarios 9、topology 3、
  codex_* 10、mcp_* 8、recovery 2、process_leaks 2、state_brief 1、session_boot 2、model_override 4、
  fork_rewind 1、worker_protocol 5 …）。
  - `engine/tests/cli.rs::doctor_probes_isolation_codex_and_config_errors`：本沙箱无 `codex` CLI，
    doctor 不打印 `codex protocol schema` 行（与本次改动无关，断言的正是 codex 探测输出）。
  - `engine/tests/tools_sandbox.rs::guard_url_matches_the_blocked_range_table`：无网络，`example.com` 解析失败。
- `tui`：`app_tests 12 + 2 + render_tests 37` 绿；`render_tests` 的 12 项帧快照测试因缺少
  `review/tmp/parity_scenario.json` fixture 失败（该快照未包含此文件，与本改动无关）。

### 4.5 字节保护

只编辑了 `engine/src/worker.rs`、`engine/src/session.rs`；`.config/teamagents/config.toml`
（md5 `3eeaf2c24ea7eaf8bdbfed48a220d4ff`）与其余仓库文件未被写入，未新增文件。

## 5. 已知取舍

- 目标会话打开失败时按"本次新建才清理"原则删除目录；预先存在（例如被别的进程持锁）的目录一律不删。
  仓库既有语义"半成品会话行保留以便用修好的配置重开"（`session_boot` 既有测试）**未改动**，清理只发生在
  fork 自己的目标目录上。
- 历史文件解析失败（非法 JSON）按原样逐字节拷贝，而不是让 fork 失败：与旧拷贝行为一致，源会话本身也用不了该文件。
- 映射只针对 Leader 活线程键；源文件里其它 epoch 的旧线程原样保留（新会话不会读取它们）。
