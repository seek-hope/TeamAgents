# Codex 成员原生记录浏览（2026-09-20）

本批继续完成方案 §9.1/§13 的用户成员记录浏览。原生历史仍由 Codex 保存；
TeamAgents 只按该成员已保存的线程 ID 读取，不复制成第二份权威记录，不将内容注入其他成员。
这是 T17/T20 的局部推进，真实供应商矩阵与整体成熟度目标仍未完成。

## 读取路径与界面

- `/history` 或团队/日志页签的 `h`，在有持久 Codex 线程 ID 的成员下增加“Codex 原生对话与工具记录”。
- 独立 App Server 客户端只调用 `initialize`、`thread/read`、`thread/items/list`，
  不调用 `thread/start`、`thread/resume` 或 `turn/start`。该客户端与正在执行的成员连接分开。
- 优先读取原生分页接口，每页 40 条。只有首页明确返回 JSON-RPC `-32601` 才允许降级；
  传输失败、权限拒绝、损坏响应及失效的后续游标均直接报错。
- 降级优先读取后端提供的 rollout 路径；没有该路径时尝试 `thread/read(includeTurns=true)`。
  原生页按线程、游标和页内容校验版本；完整历史及 JSONL 按内容版本分页，变化后要求刷新。
- TUI 保留来源、页游标和版本；条目详情分页、返回、上一页和刷新沿用对应页的身份。
  原生工具调用、文件变更、MCP 结果可查看实际记录正文。

## 已复现问题与修复

未收尾代码及本机联调中发现的四处问题均已复现：

1. 后端返回的目录外普通文件会被作为 rollout 读取。
2. 没有首条线程元数据的文件仍会被接受，不能证明记录属于所选成员。
3. JSONL 解析要求每个 `ResponseItem` 有 ID；本机生成的 Schema 允许缺省或 null，
   合法的消息、函数调用及工具输出因此被拒绝。
4. 分页存储的 rollout 可能只保存元数据；原生接口不可用时直接读取该文件，可能把不完整记录当成完整空历史。

修复与回归覆盖：

- 使用配置的 `CODEX_HOME`（缺省为 `$HOME/.codex`），只接受 `sessions/` 与
  `archived_sessions/` 内的路径。配置根可解析为实际目录；从该根起逐级固定目录句柄、
  拒绝符号链接，最后核对文件仍在允许目录中。相对返回路径、`..`、同名前缀目录与 FIFO 均拒绝。
- 只接受普通文件；打开后的大小检查与流式读取上限共同限制为 32 MiB，取消/超时在解析期间继续检查。
  首条必须有唯一且匹配的 `session_meta`，两个身份字段同时存在时必须一致；重复元数据、坏 JSON
  和截断文件报错，不返回已读到的部分正文。声明非 legacy 存储的文件不得充当旧格式回退；
  缺少存储模式字段的旧文件保持兼容。
- 原生接口使用其条目 ID；原始 JSONL 用源文件行号定位，正文保持原样，不伪造后端 ID。
  缺少回合关联时保留 null，列表显示 `—`，不编造回合。文件全文参与版本校验。
- 独立客户端单条响应限 32 MiB；关闭先结束进程组，再释放 stdin，避免管道满时在写锁上卡住。
  后端阻塞时 worker 仍可查询状态和关闭。

修复前失败日志：

- `/tmp/teamagents-native-history-red-20260920.log`：目录外读取、缺线程元数据两项失败。
- `/tmp/teamagents-native-history-raw-red-20260920.log`：合法无 ID 记录被拒绝。
- `/tmp/teamagents-native-history-store-red-20260920.log`：分页存储的 JSONL 被误作完整历史。

## 验证

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --test history_protocol
cargo test --offline --locked --manifest-path tui/Cargo.toml --test history_tests
make check
make pty
bash review/eval/check-runner.sh
```

`history_protocol` **9 项**、TUI `history_tests` **6 项**通过。新增 Engine 六项覆盖三种读取路径、
完整工具正文、移除/重开、跨线程游标、错误线程/坏响应/超大响应、路径约束、文件身份、
缺省 ID、拒绝不得降级及慢读取关闭；TUI 新增原生分页/详情/返回/刷新身份检查。

最终 `make check` 通过格式、全目标严格 Clippy、测试与卫生检查：
**core 152 / engine 375 / TUI 106，共 633 项**；engine 仍有 **3 ignored**。
三项 PTY 与 runner **30 项**契约通过。默认关闭的真实服务入口仍不计供应商验收。

日志：

- `/tmp/teamagents-native-history-check-final-20260920.log`
- `/tmp/teamagents-native-history-pty-final-20260920.log`
- `/tmp/teamagents-native-history-runner-20260920.log`

### 本机真实 CLI、人工记录联调

官方 App Server 文档说明：`thread/read` 可不恢复线程而读取；`thread/items/list` 是实验接口，
是否支持取决于活动存储。文档于 2026-09-20 抓取：
<https://developers.openai.com/codex/app-server>。
同时由本机 **Codex CLI 0.155.0** 生成实验 Schema，相关字段及哈希见
[Schema 契约摘录](eval/runs/2026-09-20-native-history/schema-contract.json)。

本机行为需要区分：在建立线程的连接中，刚创建线程的分页请求会返回“不支持”；
重新打开的持久 `paginated` 线程则可通过该接口读取。不能据前一个响应判断整个版本不支持分页。

隔离 HOME、CODEX_HOME、XDG 配置/状态，供应商地址指向本地关闭端口，不传认证。
探针先建空线程，再分别填入人工 JSONL 或原生 SQLite 条目；不启动模型回合，不执行记录中的工具。
生产 `teamagents serve` 两条路径都读出 **40+5 条**，末页工具正文正确，团队状态和原文件不变：

| 模式 | 生产读取路径 | 证据 |
|---|---|---|
| legacy | 受限 JSONL 回退，条目按源行定位 | [结果](eval/runs/2026-09-20-native-history/legacy.json)、[工具详情](eval/runs/2026-09-20-native-history/legacy-tool-detail.json) |
| paginated | 原生 `thread/items/list`，条目按原生 ID 定位 | [结果](eval/runs/2026-09-20-native-history/paginated.json)、[工具详情](eval/runs/2026-09-20-native-history/paginated-tool-detail.json) |

证据均标记 `model_invoked=false` 与 `fixture_records=true`；相关实现及测试文件的
[源码哈希](eval/runs/2026-09-20-native-history/source-sha256.json)与
[证据校验清单](eval/runs/2026-09-20-native-history/SHA256SUMS)一并保存。

本机探针按仓库约定保留在 `review/tmp/native-history-20260920/probe.py`（忽略的测试临时目录）：

```bash
python3 review/tmp/native-history-20260920/probe.py "$PWD" legacy
python3 review/tmp/native-history-20260920/probe.py "$PWD" paginated
```

前期探针错误单独保留：把人工 JSONL 附加到默认 paginated 存储不能填充其 SQLite 条目；
legacy 空线程尚未生成文件；原生接口序列化还会补入可选 null 字段。
这些均为探针假设错误，修正夹具后才取得上表结果，不计产品缺陷或真实模型失败成绩。
准备阶段调用 `thread/start` 只用于探针建夹具；产品读取路径不调用该方法。

## 保留边界

- 显示后端实际提供或已经落盘的记录。未保存的流式内容、缺失文件和未提供/加密的内部推理
  不能还原；不把原始记录拼接成虚构的完整执行历史。
- 单文件/单响应 32 MiB；旧 JSONL 每页重读有上限的文件，没有大历史索引或性能保证。
  原生分页不是整个外部线程的原子快照，内容变化可能需要刷新。
- 用户只读入口不扩大模型或观察者权限。后端初始化可能维护自己的日志/索引；
  不声称 Codex 整个数据目录逐字节不变。
- 本批真实 CLI 联调使用人工数据，不计五供应商、真实模型 TUI、陌生仓库任务或发行制品验收。
  六项部分覆盖状态保持；累计代码未提交或发布。
