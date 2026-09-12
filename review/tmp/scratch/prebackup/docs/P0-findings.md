# P0 接口验证结果（2026-09-11）

依据：`TeamAgents-Implementation-Plan.zh-CN.md` 第 18 节「开工前检查」。
环境：Arch Linux，Python 3.13.15，bubblewrap 0.12.0，git 2.55.0，codex-cli 0.154.0。

## 依赖锁定（`uv.lock`，95 个包，含 sha256）

| 包 | 版本 | 包 | 版本 |
|---|---|---|---|
| deepagents | 0.7.13 | langchain | 1.4.0 |
| langgraph | 1.2.11 | langchain-core | 1.6.2 |
| langgraph-checkpoint-sqlite | 3.1.1 | langchain-openai | 1.6.2 |
| langchain-anthropic | 1.7.2 | langchain-deepseek | 1.1.0 |
| langchain-mcp-adapters | 0.3.2 | pydantic | 2.13.5 |
| textual | 8.2.8 | | |

## 1. Deep Agents middleware 注入（P0-1）✅

`abefore_model` 返回 `{"messages": [...]}` 可在回合中注入新消息；注入发生在下一次模型调用前，
工具调用/结果（`tool_calls` ↔ `ToolMessage`）配对保持完整。并发线程互不阻塞（B 完整跑完时 A 仍阻塞在工具里）。

## 2. 独立线程持久化暂停/恢复（P0-2）✅

- 同一 SQLite checkpointer 上不同 `thread_id` 独立执行、独立暂停；
- `interrupt_on` + `Command(resume={"decisions":[{"type":"approve"}]})` 暂停/恢复可用，
  暂停期间工具未执行，恢复后执行；
- 进程级重建（新 saver + 新 agent 实例，同一 db 文件）后历史续接为 7 条消息。

## 3. Codex app-server 协议（P0-3）✅（CLI 0.154.0）

实际握手验证：`initialize` → `thread/start` → `turn/start` → `turn/interrupt`，
批准请求 `item/commandExecution/requestApproval`，回复 `{"decision":"accept"}` 后回合正常完成。
中断表现为 `turn/completed` 且 `turn.status="interrupted"`（没有单独的 interrupted 通知）。
断线核对使用 `thread/read {includeTurns:true}`（返回 turns[].items[]）。

Schema **不入库**：`teamagents doctor` 用 `codex app-server generate-json-schema` 从**本机实际 CLI**
生成并校验所需方法集合（`initialize`、`thread/start`、`thread/resume`、`turn/start`、`turn/interrupt`、
三类审批请求），依赖升级后自动复验，避免 4.3MB 生成物与 CLI 版本漂移。

注意：本机 `~/.codex/config.toml` 为 `sandbox_mode=workspace-write`、`approvals_reviewer=auto_review`，
因此 read-only 沙箱下 /tmp 写入仍被放行。适配器必须显式传 `approvalsReviewer:"user"`（已验证）才能拿到
用户级批准请求；「现有配置更宽松时以显式参数收敛」记录在此。

## 4. Linux 隔离（P0-4）✅

bwrap 探针（只挂载 /usr /etc /opt + 授权目录，`--unshare-pid --unshare-net --new-session`）：

| 越界尝试 | 结果 |
|---|---|
| 授权目录内写入 | ✅ 允许 |
| 读取 $HOME（含 `~/.codex/auth.json`） | ✅ 不可见 |
| 写 /etc | ✅ 只读 |
| 网络（/dev/tcp 1.1.1.1:80） | ✅ unreachable |
| `/proc/1/root` 逃逸 | ✅ 不可见 |
| 授权目录内指向 $HOME 的符号链接 | ✅ 目标未挂载，读取失败 |

## 5. 模型 profile 冒烟（P0-5）⚠️ 部分通过

| 服务 | 真实工具调用 + 续接 | 说明 |
|---|---|---|
| DeepSeek | ✅ | `deepseek-flash`，add 工具调用→回填→续接 |
| Kimi（OpenAI 兼容路径） | ✅ | `https://api.kimi.com/coding/v1` + KIMI_API_KEY |
| OpenAI（官方） | ✗ | 环境内 `OPENAI_API_KEY` 实为中转站 `https://tokens.byteseek.ai` 的 key，在 api.openai.com 401；该中转站当前无额度 |
| apexin 中转 | ✗（待密钥） | key 位于 `~/.codex/others.config.toml`（`experimental_bearer_token`），未导出为环境变量 |
| Anthropic | ✗ | 无密钥 |
| GLM | ✗ | 无密钥 |

按方案允许：缺密钥期间用假模型跑契约测试；**发布前必须补齐五家真实冒烟**。

### 5.1 已完成的真实验证（2026-09-11，P3 阶段）

`tests/test_p3_real_models.py`（契约：工具参数拼接、工具/结果配对、多轮续接、流式、用量、错误透传）
与 `tests/test_p3_live_session.py`（产品链路实时端到端）在 DeepSeek 与 Kimi 上运行通过：

- 单 Leader 实时会话：用户目标 → Leader 调用 `signal_done` → `goal_done`、回合 COMPLETED；
- 实时组队：Leader 用 `apply_topology_patch` 新增成员并建通道 → `assign_task` 委派 →
  新成员（同为真实模型）执行 → 目标完成（同一会话内，无需重启）。

Anthropic / GLM / OpenAI 官方：无可用密钥，相关测试显式 skip（skip 不计入发布验收，见方案 §17）。
补齐方式（无需改代码）：

```toml
# ~/.config/teamagents/config.toml
[models.leader_main]
provider = "openai"
protocol = "openai"
model = "gpt-6-astra"
base_url = "https://api.apexin.ai/v1"     # 或 https://tokens.byteseek.ai/v1 等中转站
api_key_env = "APEXIN_API_KEY"            # 用户自行 export，不写入仓库

[models.research]
provider = "openai"
protocol = "openai"
model = "kimi-k2-turbo-preview"
base_url = "https://api.kimi.com/coding/v1"
api_key_env = "KIMI_API_KEY"

[models.coding]
provider = "deepseek"
protocol = "deepseek"
model = "deepseek-flash"
api_key_env = "DEEPSEEK_API_KEY"
```
