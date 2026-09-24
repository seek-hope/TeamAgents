# Coding Agent 工具可靠性审查（2026-09-17）

范围：`engine/src/tools.rs`、`engine/src/bound.rs`、`engine/tests/tools_sandbox.rs`。
依据：方案 §12 与已确认决策 D-19、D-21、D-32。仅使用本地回归，不调用付费模型。

## 已复现并修复

### CT-01：多文件编辑可能只提交一部分（高）

`edit_files` 在所有匹配校验通过后，逐个调用 `atomic_write`；路径锁也逐个获取、释放。
后一个文件的输出大小限制、临时文件写入错误、版本冲突或取消发生时，前一个文件已经提交。
这与 `docs/ACCEPTANCE.md` 的“任一失败则一个字节都不落盘”不符。
确定性复现：第二个文件的新内容超过 10 MiB，第一个文件已经被改为 `changed`。
修复前新增的 `batch_edits_reject_oversized_results_before_changing_any_file` 实跑失败：
断言左值 `changed`、右值 `alpha`。

修复：所有输出大小先校验；按照路径排序后同时持有全部路径锁；将新内容和原内容的恢复副本
都暂存、同步，再复核全部版本；逐个提交。后续提交失败或取消时撤销已提交编辑，恢复权限。
回滚前再验新内容 hash，避免覆盖第三方刚写入的内容；若回滚也失败，保留恢复副本并明确给出
路径。单文件写入沿用同一个暂存实现，临时文件由 RAII 清理，不增加依赖。

额外回归：`batch_edits_recheck_all_versions_after_waiting_for_locks` 持有第二条路径锁，
在批处理等锁期间模拟外部修改，确认第一条路径锁一直持有且没有提前提交；
`batch_commit_rolls_back_when_a_later_rename_fails` 删除第二条暂存文件来注入确定的提交错误，
确认第一条已经执行重命名后仍恢复原字节、可执行权限，并清除暂存文件。

### CT-02：读取特殊文件可无限阻塞回合（高）

`open_member_file` 先 `File::open` 再检查是否普通文件；命名管道在 `open` 阶段即可阻塞。
`read_file`/`edit_file`/图像读取共用这条路径。文件工具不经过 shell 超时，取消也无法让
阻塞的 `open` 返回。确定性复现：在工作目录创建无写入端的 FIFO，再调用 `read_file`；
修复前 `file_tools_reject_fifo_without_waiting_for_a_writer` 实跑失败，2 秒等待返回 `Timeout`。
探针会主动打开写入端解除阻塞后再报告失败，避免自身遗留阻塞线程。

修复：普通文件使用非阻塞方式打开后检查文件类型；父路径打开显式要求目录，防止路径并发
替换成 FIFO。原有工作目录范围和符号链接校验继续生效。沿用已有 Linux 平台依赖。

## 验证

```bash
cargo test --offline --manifest-path engine/Cargo.toml --test tools_sandbox
cargo test --offline --manifest-path engine/Cargo.toml --lib tools::tests::
git diff --check -- engine/src/tools.rs engine/tests/tools_sandbox.rs
```

实际结果：`tools_sandbox` 16 个通过；工具模块单测 21 个通过；差异空白检查通过。
包括真实 bubblewrap 下的输出排空、超时子进程清理、中断输出保存、持久 shell 状态和
离线 Rust 构建。没有使用外部真实模型。这些是工具可靠性的回归证据，不构成同竞品
端到端 coding agent 成功率的比较结果。

## 仍需明确的能力边界

- Shell 任意外部写入不遵守结构化文件锁，方案 §12.3 已明确该限制。
- 多路径文件系统重命名无法提供进程崩溃时的全有或全无事务，也无法让外部读取者观察到
  同时切换的文件集合。已修复提交前校验和普通运行失败回滚；进程崩溃恢复需要持久日志，
  本轮未引入该能力。磁盘故障或外部程序并发写入也可能使回滚失败，工具会明确报告。
- 路径锁以会话目录存放，同一个会话里的成员可相互排斥；不同会话的相同项目目录不共享
  锁目录，只能依赖版本复核，无法承诺跨会话的无竞争读改写。宜继续使用 worktree 隔离。
- 长命令当前为同步执行，并提供超时、中断、制品；尚无可供模型持续读取并交互的后台终端
  会话工具，也没有通用的交互式 stdin/PTY 接口。新增接口属于能力扩展，需独立设计验收。
- Shell 输出保留的是 stdout/stderr 到达顺序，独立管道原始交错不可恢复。
- `bound.rs` 已检查绑定白名单、required 服务启动失败、无用连接清理及按服务前缀命名；
  本轮未改动 MCP 分发。不同服务配置使用相同前缀时的命名冲突尚未建立复现证据，未记为
  已确认缺陷。

没有改动用户已有的 `AGENTS.md`、`README.md`、`docs/DECISIONS.md` 和 `docs/USER-GUIDE.md`。
