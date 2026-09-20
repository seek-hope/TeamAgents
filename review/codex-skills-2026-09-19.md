# Codex 成员 Skills/指令注入（2026-09-19）

## 目标

方案 §10.2 与 §12.1 要求成员选择的 Skills/指令文件能够进入成员执行上下文。此前
`session.rs::make_runner_factory` 在 Codex 分支提前构造 `CodexOptions`，因此同一份
`member_context` 只进入 Chat 的 system prompt，Codex 仅收到成员 `instructions`。

本批没有扩大 Codex 的工具、工作目录、批准策略或团队身份，只补齐已有的受限文本注入路径。

## 实现

- Chat 与 Codex 现在共同调用 `member_context`。
- 该函数继续使用原有边界：每文件最多 8,000 字符、每成员合计最多 32,000 字符；选定
  Skills 按用户→项目→成员根覆盖；项目根和用户配置目录的 `AGENTS.md` 自动加入；符号链接
  逃逸的 Skill 被拒绝。
- Codex 将成员专属 `instructions`、随后每个已选上下文块合并到
  `thread/start` 与 `thread/resume` 的 `developerInstructions`。没有设置
  `baseInstructions`，因此不替换 Codex 原生 system prompt。
- `CodexOptions` 保留原有仅成员指令的测试辅助路径；生产构造使用带 context 的变体。

## 证据

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml session::tests::
cargo test --offline --locked --manifest-path engine/Cargo.toml --test session_identity \
  codex_member_receives_the_same_bounded_skills_and_project_instructions_as_chat
```

新增/更新检查确认：

1. 本地假 `codex app-server` 在 `thread/start` 收到 Worker 环境、成员指令、选定 Skill 和
   项目 `AGENTS.md`，顺序为环境层→成员指令→Skill→项目指令。
2. 同一会话重开走 `thread/resume`，仍收到相同注入，不复制到回合输入。
3. 注入仍受成员选择约束；未知/逃逸 Skill 不会通过 Codex prompt 泄露外部文件。
4. 原有 Chat Skills、指令文件与 Codex 协议回归保持通过。

## 边界

- 这不是 Codex 完整外部历史、工具日志或内部推理的本地镜像。
- Codex 仍不使用其自身的 Skills 发现协议；TeamAgents 只分发已选且受限的文本。
- 本批使用本地假 app-server，不能替代真实 Codex CLI 或五家供应商验收。
