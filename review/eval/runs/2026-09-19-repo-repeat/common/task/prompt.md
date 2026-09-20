请修复这份 TeamAgents 真实 Rust 仓库快照中的会话分叉与切换问题。用户在已有团队里调整过 `/model`，也会自动创建会话级模型 profile；fork 后配置可能丢失、旧对话不能继续，失败的切换还会使原会话不可用。

需要满足以下行为：

1. `serve` 的 `fork_session` 创建新会话，继承当前 TeamSpec、Leader 对话树及旧版线性对话、会话 `profiles.json` 与 `/model` 覆盖。自动 profile 只在源会话存在时，也必须能 fork；分叉后 `model` 报告的 profile、模型与思考档位应与源会话相同，重开后仍然相同。
2. Leader 可能已经重置过上下文，当前 `context_epoch` 不一定为 1。对话按 `ctx:<leader>:<epoch>` 存放，新会话使用自己的 epoch；当前历史必须映射到新会话实际线程。树的全部分支、节点与 leaf 保留。旧线性历史遵循同样的映射，源文件不被修改。
3. 新会话不继承源会话的任务、运行记录、共享事实、成员私有历史或 Shell 状态，不回滚或复制项目工作文件。源会话的磁盘事实和用户文件保持完整。
4. `open`、`switch_session` 或 fork 打开新会话失败时，旧会话应继续可用，保留原来的工作目录、团队配置和权限模式。fork 的新会话若打开失败，清理本次未完成的目标目录，不删除或归档源会话。
5. 任何成员有 QUEUED/RUNNING 回合时仍须拒绝 fork，不能通过取消成员来强行分叉。保留现有 JSON-lines 协议与错误回执，不增加新命令、依赖或产品范围。

本次明确授权以上修复，优先于快照内文档对旧限制的描述。只允许修改 `engine/src/worker.rs` 与 `engine/src/session.rs`；可以在这两个文件里增加局部测试。其他源码、Cargo 清单和锁文件、既有测试、文档以及 `.config/teamagents/config.toml` 必须逐字节保留，不新增其他文件。Cargo 生成的各 crate `target/` 构建产物不计入源码范围。

仓库含 core、engine、tui 三个独立 crate。依赖已缓存，请使用离线 Cargo；`.config/teamagents/config.toml` 是供旧测试使用的无凭据配置，必须保留。Shell HOME 是独立目录，运行旧测试时须显式使用该配置。在仓库根目录至少运行：

```bash
XDG_CONFIG_HOME="$PWD/.config" cargo test --offline --manifest-path engine/Cargo.toml --test fork_rewind --test model_override --test session_boot --test worker_protocol -- --test-threads=1
```

请先定位调用链，再修复并实际验证，最后报告行为变化、修改位置与测试结果。结束后会在仓库之外进行独立隐藏验收，公开测试通过不能代替以上要求。
