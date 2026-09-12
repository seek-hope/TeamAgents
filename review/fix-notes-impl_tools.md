# impl_tools 修复笔记（runners.py / execution.py）

## P0-1 文件工具逃出授权 workdir —— 已修复并验证

**改动**
- `src/teamagents/execution.py`
  - 导入 `WriteResult/EditResult/DeleteResult/FileUploadResponse`（第 19-26 行）。
  - `GuardedFilesystemBackend._resolve_path`（186-210 行）：基类 `ValueError`
    （`..`/符号链接越界）归一为 `PermissionError`，所有 backend 方法把它当普通错误
    结果返回，不抛异常；仍保留真实路径 containment 检查。
  - 新增 `ReadOnlyFilesystemBackend`（213-247 行）：复用 Guarded 防护；覆盖
    `write/edit/delete/upload_files`，返回清晰错误结果（不抛异常），读取可用；
    `virtual_prefix` 回显模型实际调用的路径（如 `/memory/0/config.toml`）。
- `src/teamagents/runners.py`
  - 导入 `GuardedFilesystemBackend / ReadOnlyFilesystemBackend`（22-30 行）。
  - `_build_backend()`（445-462 行）：`/artifacts/` → `GuardedFilesystemBackend`
    （可写但受真实路径约束）；`/skills/{i}/`、`/memory/{i}/` →
    `ReadOnlyFilesystemBackend`；workdir 默认 `IsolatedShellBackend` 不变。

**验证**
- `review/tmp/exp_escape.py`：`config.toml written: False`、`skill overwritten:
  False`、write_file ToolMessage = `Error: /memory/0/config.toml is read-only for
  members...`（skills 同理）、settled 正常、0 批准、goal_done。
- `tests/test_p3_backend_guard.py`（4 用例，全过）：只读路由拒绝写/编辑/删/上传且
  读取正常；Guarded 符号链接与 `..` 越界拒绝、根内读写正常；端到端真实 graph：
  memory/skills 写拒绝 + artifacts 可写 + 符号链接逃逸拒绝 + workdir 正常写 +
  memory 读取正常 + 无批准 + goal done；artifacts 可写回归。

## P0-2 private general-purpose subagent 绕过批准 —— 已修复并验证（硬拒绝方案）

**侦察结论（deepagents 0.7.13）**
- `graph.py:796` 自动加 GP 的条件：profile 未禁用且 `inline_subagents` 中无同名 spec
  → 显式传 `subagents=[spec(name="general-purpose", ...)]` 即可替换自动版。
- GP 只继承「命中默认 GP 槽位」的父 middleware，所以必须显式挂自己的 middleware。
- 试过「同一 `TeamAgentMiddleware` 实例挂进 GP（同一批准闸门 + 共享步数）」：子代理内
  `interrupt()` 能被父图挂起（复现脚本观察到 WAITING/第二个批准行），但 **resume
  不可靠**：replay 后 `approvals.recheck()` 失败（实测子代理拿到 “Approval no longer
  matches this call”），且嵌套 replay 会重复子代理模型调用；暂无法在预算内做稳。

**最终实现（按任务书允许的退化路径：硬拒绝，不退化为移除能力）**
- `src/teamagents/runners.py`
  - 新增 `SubagentGate(AgentMiddleware)`（338-385 行）：`awrap_tool_call` 用**同一
    `PermissionPolicy` + 同一 store 的 session 批准**判定——策略允许或已有 session
    批准 → 放行；需**新**批准的调用 → 返回明确拒绝 ToolMessage（不创建 PENDING 批准，
    避免 `signal_done` 死锁）；`TEAM_TOOLS` 对子代理不可用；绑定工具（MCP/web）照常。
    `awrap_model_call` 与成员共享 `TeamAgentMiddleware.model_steps` 计数并在超限时抛
    `TurnLimitExceeded`（计入成员资源使用，方案 §6.4）。
  - `_general_purpose_spec()`（487-504 行）：显式 GP spec，tools = 成员执行工具
    （去 team 工具），middleware = `[SubagentGate]`；`_ensure_graph` 以
    `subagents=[...]`（531 行）传入，替换 deepagents 自动版。
- 取舍：需批准的操作在子代理内**确定被拒**（不挂起）；模型被提示改由成员本回合处理。
  产品能力（私有子代理）保留，未做需请示的产品变更。

**验证**
- `review/tmp/exp_subagent.py`：子代理 `shell(network=True)` 收到
  `Blocked by team permissions: shell needs explicit user approval...`；DB 批准行仍只有
  父 `task` 那 1 条（无零批准执行、无新增待批准）；run COMPLETED、goal_done。
- `tests/test_p3_subagent_gate.py`（1 用例，过）：子代理内 in-scope `shell` 正常执行
  （能力保留），网络升级被拒且输出 `NET-ESCAPED` 不出现；无新批准行；子代理模型调用
  计入成员步数（5 ≥ 成员自身 2 次）；goal done。

## 全套件

`.venv/bin/python -m pytest tests/ -q` → **107 passed, 2 failed, 12 deselected**；
2 failed 即基线环境失败（`test_config_cli.py::test_xhigh_maps_to_max_for_models_without_xhigh`
缺 DEEPSEEK_API_KEY、`test_p3_web_tools.py::test_ssrf_guard_blocks_internal_targets`
无 DNS）。无新增失败。

## 遗留风险 / 待办

- 子代理内需批准的操作只能被拒、不能就地获批；成员可在自己回合重试（有意取舍，见上）。
- 嵌套 interrupt 的 resume 不可靠属 deepagents/langgraph 层行为，若未来要「子代理就地
  获批」，需先做 replay 语义验证（并对子代理重复模型调用做幂等保护）。
- `/memory/{i}/` 路由以 memory 文件父目录为根（只读）：同目录其他文件（如
  `~/.config/teamagents/config.toml`）对模型**可读**。写入已被拒，但如需完全隔离该
  目录，可后续把 memory 路由收敛到精确文件（属加固增强，本次未做）。
- `ReadOnlyFilesystemBackend` 的拒绝消息不含 backend 真实路径，只回显虚拟路径（避免
  泄露宿主机布局）。
