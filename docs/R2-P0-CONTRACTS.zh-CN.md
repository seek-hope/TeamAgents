# R2-P0 执行合约与退出记录

日期：2026-09-23。范围：R2-P0 的接口合约、依赖选择、隔离原型与本机探针。本文记录已经落实并能复跑的边界；探针通过不表示生产 daemon/kernel 已实现，也不表示 R2-P1 之后已交付。

## 合约

### 身份与状态

- 每个命令、输入、请求、操作和制品均有稳定 ID；重复 ID 必须带相同载荷。客户端可用原 command ID 查询同一回执。
- 每个会话数据库是业务状态的唯一权威来源。业务事实、去重回执和事件在同一 SQLite 事务内提交；磁盘满或事务错误向调用者传播，不得报告成功。
- 实例生命周期与执行位置分开记录。执行位置为 `READY`、`MODEL_PENDING`、`TOOLS_PENDING`、`WAITING`、`COMPLETION_PENDING`；可运行性为 `RUNNABLE`、`PAUSED`、`PARKED`、`TERMINATED`。未完成目标不能因普通回复或无就绪实例而自动成功。
- 任务终态为 `SUCCEEDED`、`FAILED`、`CANCELLED`；外部操作终态为成功、失败、取消或 `OUTCOME_UNKNOWN`。实例暂停、任务取消与已成功操作可以同时成立，不能用单个状态字段互相覆盖。
- 未知外部效果先核验；仍无法确认时停放并通知，不盲目重放。生命周期重置增加 epoch，迟到结果只能归属原操作和原预算。

### 事务、制品与故障

- 输入 ID 去重、上下文追加、执行位置推进和事件写入同事务。崩溃发生在提交前时整组不可见；提交后整组可恢复，重放不重复应用。
- 大型完整响应先登记稳定制品 ID、摘要及发布所有者为 `STAGING`，再同步并原子发布文件。引用写入与转为 `LIVE` 同事务；读取必须校验摘要及 ACL。
- `STAGING` 和未导入 job 的结果受保护，不按年龄或“当前无引用”回收。GC 在事务内先将无所有者对象标记 `DELETING`，从此拒绝新引用，再删文件；失败或重启后可继续。只有明确放弃的制品进入 `ABANDONED`。
- 同一 runner job 的 GO/CANCEL 串行处理。接受取消且尚未启动时持久化 `CANCELLED_BEFORE_START`，永久拒绝迟到 GO。已经越过启动边界则请求停止并单独记录实际效果；缺 PID、runner 崩溃或结果未核验都不能推断为未执行。
- 正常取消先持久化再停止进程组。存储写入失败时冻结相关新派发，使用经进程身份核验的控制路径尽力停止；回执必须分别表明取消是否持久化、进程是否停止。停止进程不能伪报为取消已保存。
- SQLite 写队列与 socket 请求有界，慢 I/O 不得阻塞控制接收。daemon 与 runner 用独立文件锁防止双主，运行工具不得继承 daemon 锁。

### 本机协议和阶段边界

- 探针的 Unix socket 请求为单行 JSON，含版本 `1`、`command_id`、方法；请求与响应上限 64 KiB。正式协议仍由后续 R 阶段定义。
- TUI 退出表示断开，不隐式停止后台任务。重连后显示相同任务、状态和事件水位；暂停阻止后续推进，恢复继续同一任务，取消是独立控制命令。
- full_auto Shell 的主机权限、后台服务和进程组取消遵循已记录的 D-41；本探针的受控 runner 用独立 job 进程验证生命周期，不替代生产 Shell 实现。
- R2-P0 不调用模型、不执行数据清理或发布。合成的 1,000,000 单位上下文用于构造/存储开销，不是真 tokenizer token，不得记入模型性能成绩。

## 锁定依赖

| 用途 | 锁定版本 | 范围与理由 |
|---|---|---|
| SQLite | `rusqlite 0.40.2`，bundled `SQLite 3.53.2` | core/engine 共用；链接版本高于 WAL-reset 修复基线 3.51.3 |
| 异步执行器 | `tokio 1.53.1` | engine P0 探针开发依赖；模型 I/O 与控制响应/取消并发对比，暂不进入生产模型循环 |
| 异步 HTTP 客户端 | `reqwest 0.12.28` | engine P0 探针开发依赖；测试流式响应和 future 取消 |
| 阻塞对照 HTTP 客户端 | `ureq 2.12.1` | engine 现有生产依赖；阻塞读取只受读超时限制，作为对照组 |

版本以各 crate 的 Cargo.lock 为准。P0 的 1/4/16 并发点和单机结果不定义生产默认并发数；默认值需在目标机器、稳定负载和预先登记的正式实验中确定。

## 退出证据

实现是 `engine/examples/rebuild_p0/` 隔离进程/SQLite 原型、`tui/examples/rebuild_p0.rs` 最小前端和 `tui/scripts/pty_rebuild_p0.py` 真终端驱动。复跑命令：

```bash
cargo build --offline --locked --manifest-path engine/Cargo.toml --example rebuild_p0
cargo build --offline --locked --manifest-path tui/Cargo.toml --example rebuild_p0
engine/target/debug/examples/rebuild_p0 suite review/tmp/r2-p0-new
python3 tui/scripts/pty_rebuild_p0.py
```

证据目录必须是新的。本机通过记录见[探针报告](../review/r2-p0-2026-09-23.md)，原始 JSON 在被忽略的 `review/tmp/r2-p0-final/report.json`。

退出范围：实际 SQLite `SQLITE_FULL` 注入；同库输入在 SIGKILL 提交前/后的恢复与重放去重；制品发布 SIGKILL、GC 保护/认领/重启；CANCEL→GO、重复 GO、GO 接受后崩溃、runner/daemon 崩溃、取消写失败尽力停止、重复回执、截止时间；daemon 锁唯一、runner 独立存活、断连后稳定回执；PTY 断开/重连/暂停/恢复/取消；1M 合成上下文的追加、请求构造和重开；阻塞模型响应下 1/4/16 活跃 I/O 的取消与控制响应。

不在此退出范围：生产内核与业务 schema、真实模型流、系统掉电/文件系统故障、完整实例执行、真实负载并发调度、配置与凭据热更新、正式性能默认值，以及 A01–A36 的产品验收。最小 UI 和演示状态只用于证明前后端生命周期边界，不得作为 R2 产品 UI 或 kernel 完成的证据。
