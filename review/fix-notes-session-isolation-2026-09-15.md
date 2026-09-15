# 修复台账：批准会话归属 + 回执不可冒充（2026-09-15）

范围：`core` 单 crate（`control.rs` / `storage.rs` / `server.rs` + 测试）。引擎与 TUI 未改，
但都走同一 wire 面，故三个 crate 全量重跑。

## 缺陷

1. **批准行的会话归属未在 wire 面校验**
   - 现象：同一个 core 进程可服务多个会话（`Server::controls` 按 session_id 分表），而
     `get_approval` / `expire_approval` 两个方法按 `approval_id` **全局**查/改：
     `{"session_id":"s2","approval_id":<s1 的 id>}` 会读到 s1 的批准行，也能把 s1 的
     PENDING 行置为 EXPIRED（进而让 s1 的操作被静默作废）。
   - 修复：`storage.rs` 增 `get_approval_for_session` / `expire_approval_for_session`
     （SQL 增加 `session_id` 谓词，其余语义与原函数逐字一致）；`server.rs` 的 `get_approval`
     / `expire_approval` 与 `control.rs` 的两处决定/读取路径（`decide_approval`、
     批准决定入口）全部改走会话限定版本。
   - 回归：`storage::tests` 扩展——s2 视角下 `expire_approval_for_session("s2","a_pending")`
     为 false、`get_approval_for_session("s2","a_pending")` 为 None，而 s1 的行仍被正常过期。
   - 残留：`Store` 内部两个全局用法保留（`expire_run_approvals` 与
     `find_run_approval`→`get_approval`），它们的 id 来自 run-scoped 查询（run_id 全局唯一），
     不接 wire 输入。

2. **同一 `action_id` 换内容可冒充成功回执**
   - 现象：回执回放只看 `action_id` 是否存在。复用一个已用过的 id、换 payload 或换 actor 提交，
     会**静默返回旧动作的回执**——即"这次动作成功了"是假的，实际什么都没执行。崩溃去重
     （T21）信任的正是"回执=该动作的结果"，这一条把它打穿。
   - 修复：`control.rs` 抽出 `prior_receipt()`，回放前比对行内
     `(session_id, actor_id, kind, payload_hash)`（`payload_hash` 即 canonical payload 的 sha256）；
     不一致直接 `Err("action_id … was already used with different action data")`，
     覆盖成功路径与"失败回执重放"路径两处调用点。`storage.rs` 增 `get_action_metadata`
     取这四列（`actions` 表列均为 NOT NULL，无历史列缺失问题）。
   - 回归：`core/tests/engine.rs::action_id_reuse_with_different_payload_is_rejected`
     （换 payload、换 actor 都报错；事件数仍为 1，确认没有半执行）。
     同 id 同内容的合法回放仍绿：`t1_user_message_queues_leader_run`、
     `reduce_failure_rolls_back_and_the_refusal_replays`。

## 收紧与清理

3. `Control::validate` 增加会话归属校验：`action.session_id` 必须等于目标会话，否则拒绝。
   纵深防御——wire 的 `submit` 本来就按 `action.session_id` 路由到对应 control。
4. `read_shared` 拒绝 `limit <= 0`：原实现里 `limit: 0` 是"空结果但成功"，负值更糟——
   直接落到 SQLite `LIMIT -1`（**无界读取**）。回归：
   `read_shared_rejects_malformed_paging_arguments` 新增 `limit: -1` 断言。
5. `pbool` 改用文件内既有的 `truthy()`（该函数的注释早已写明是
   `payload.get(k) or payload.get(j)` 的 Python 真值语义）。旧实现
   `as_bool().unwrap_or(!is_null() && != Json::from(0))` 把 `[]`、`{}`、`""` 判真，
   `0.0` 也判真（serde_json 的 `Number(0.0) != Number(0)`），与注释矛盾。
   **wire 行为变化**：仅畸形输入的真值判定变严；`reject` / `remove` 的线上写入方
   （工具 schema `"type":"boolean"`、引擎与 TUI）都发布尔值，不受影响。
   回归：`control::tests::pbool_uses_json_truthiness`。
6. 顺带清理：`shared_entries(&[s.id.clone()])` → `std::slice::from_ref(&s.id)`（少一次克隆）；
   `from_agents` 的 `if/else if` 合并为单条件（等价）。

## 验证（2026-09-15）

```bash
cargo test --offline --manifest-path core/Cargo.toml     # 57 = 22 库 + 35 集成
cargo test --offline --manifest-path engine/Cargo.toml   # 189（未改，重跑确认）
cargo test --offline --manifest-path tui/Cargo.toml      # 91（未改，重跑确认）
git diff --check                                        # 无行尾空格
```

core 由 55 增至 57：本次新增两个用例（库级 `pbool_uses_json_truthiness`、
集成级 `action_id_reuse_with_different_payload_is_rejected`）。`docs/ACCEPTANCE.md`
的基线表已同步。

## 仍未做 / 已知天花板

- 本轮未跑真实模型评测与 PTY 冒烟（改动集中在 core 的事务与 wire 校验，与模型/终端无关）。
- `server.rs` 的 `shared_entries` 内部方法仍不校验 `limit`（`-1` 即无界）：它的调用方
  全是 core 内部（TUI 固定 1000、视图 50、引擎不传用默认 200），模型侧的 `read_shared`
  走的是已加校验的 action 路径。若将来把该方法直接暴露给模型输入，需补同一个 `limit > 0` 校验。
- `prior_receipt` 比对不含 `run_id`：同 id、同 payload、同 actor 但不同 run_id 的重放仍回旧回执
  （不执行、无副作用，仅回执归属旧回合）。补上它只会让这类重放报错，不影响安全边界，故未加。
