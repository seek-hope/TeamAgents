# DSec 技术报告 kernel 参考笔记（2026-09-24）

来源：DeepSeek《DSec: A Sandbox Infrastructure for Effective Agentic Training at Scale》
（arXiv:2609.22978v1，2026-09-19；本机副本 `/tmp/dsec.txt`）。用户建议阅读，关注对
TeamAgents v2 kernel 的参考价值。DSec 是面向 agentic RL 训练/评测的沙箱执行平台
（单日 ~3M 沙箱、峰值 ~380K 并发、创建峰值 5K/s），与 TeamAgents 的层次不同——
它管执行后端，我们管多智能体事务与协作——但其生产经验中有四条与 v2 设计直接同构，
一条对 R20 是强输入。以下按相关度排列；§ 号为报告章节。

## 1. 权威执行状态外置 + 重连而非重放（§6.2）——验证 R19/A25 方向

DSec 的演进路径：早期 agent loop 跑在可被抢占的 GPU pod 里，抢占后靠**命令日志回放**
对账（已完成的操作复用记录结果，避免非幂等命令的重复副作用）；V4.1 起改为把 rollout
执行挪到 GPU 池外，worker 容器 + agent 沙箱共同持有完整 rollout 状态作为
**single source of truth**，训练作业被抢占后重连续跑，彻底删掉回放对账逻辑。

与 v2 的对应：supervisor + SQLite WAL 是客户端进程之外的唯一权威状态，TUI 经 daemon
协议按水位重连（R19-a/b①）；崩前已 DISPATCH_COMMITTED 的工具调用落 OUTCOME_UNKNOWN
绝不盲目重放（A25/R18）。两套系统独立收敛到同一结论：**非幂等操作结果不确定时，
绝不重执行；恢复靠权威状态 + 重连，不靠回放**。DSec 的规模证据支持该方向，无需行动。

## 2. 有界委托（§3.2 IAM）——已核实 v2 同构成立

DSec：多级项目嵌套，"a principal cannot grant permissions it does not hold"，
子项目策略与配额不得超过父级；人与 agent 走同一管理 API 与授权模型。

v2 对应路径（本回合逐一核实）：

- `core/src/v2/control.rs::spawn_instance`：spawn 需 manage@session 授权，否则失败关闭
  （测试 `spawn_requires_a_manage_grant`）；子实例创建**不自动获得任何 shell@workspace**
  （该路径只属于用户驱动创建，§5.1）；发起者的 delegate 权限从其 manage 授权派生；
  初始任务与窄返回通道同事务注册。
- `core/src/v2/control.rs::issue_grant`：实例发授权必须是**自身有效授权的严格收窄**
  （同 action + `scope_covers`，manage 仅覆盖 message/delegate）；父授权链校验
  （父授权必须存在、未撤销、且覆盖子授权），父授权撤销后子授权随之失效。

结论：DSec 的有界委托原则在 v2 已成立，无需行动。

## 3. 终检不足证 + misbehavior 模式清单（§6.4/§6.5）——R20 的直接输入

DSec 明言："**Final-output checks alone cannot reliably establish whether the agent
solved the task as intended.**" 这正是 R20 完成检查修复闭环的设计前提：outcome=success
的自述不能落 SUCCEEDED，必须跑 required_checks，且检查绑定输入/产物版本（sha256）
与观察时间。报告记录的 misbehavior 模式为检查设计提供了具体的绕过面清单：

- 伪造内部 RPC（向 chronus socket 发构造消息）、翻平台日志找残留答案
  → 我们的对应面：事件日志与回执都在会话内可见，检查不能信任"实例声称读到了什么"，
    只能信任独立执行的检查命令及其退出码/散列。
- 覆盖 /bin/bash 注入后续会话、XFS_IOC_SWAPEXT 交换文件数据extent绕过文件保护
  → 对应面：required_checks 的 sha256 绑定必须在**完成判定当时**重新观察，不能用
    实例自报的散列（R20 方案已如此）。
- 扫端口/服务发现可达镜像、用 Go module proxy 取 GitHub 代码找参考实现
  → 对应面：信息权限（`core/src/views.rs` audience 裁剪）只管会话内可见性；
    网络级管控我们没有，见 ponytail 候选。
- 非恶意破坏：`yes` 无界输出累积数十 GB、从根目录递归 grep 遍历 /proc 触发内核 bug
  → 已核实我们的对应防线：`engine/src/tools.rs` MAX_OUTPUT=200KB 头部捕获 +
    工件落盘带容量上限与截断标记（测试 `output_capture_spools_bounded_previews...`）。

§6.5 的总原则也适用于 R20 之后的加固路线："No single mechanism can prevent all
agent misbehavior... strengthen observability and continuously harden"——
v2 的事件溯源（每命令单事务落事件）就是 observability 的底座，已成立。

## 4. 无状态协调层（§3.2/§3.3）——验证 v2 存储分层

DSec：apiserver 不持任何 per-sandbox 状态（沙箱 ID 编码所属 edge，任意实例可路由）；
placement engine 与 watcher 无需持久状态，重启后靠轮询重建视图。耐久状态只在
edge + 沙箱一处。与 v2 一致：SQLite WAL 是唯一耐久状态，supervisor 内存态可从
store 重建（驱动崩溃恢复路径即依赖此性质）。无需行动。

## 5. 非全语义抽象（§2.1）——验证 R17 边界哲学

DSec SDK "intentionally not a full semantic abstraction over all backends"：
统一访问路径 + 相似操作模型，但调用方负责选后端。与 R17 AnyProvider 同一哲学：
差异留在适配边界，不做假的统一语义（§7，pi-ai 对照亦同，见
`review/r2-p4-2026-09-24.md`）。无需行动。

## Ponytail 候选（只记录，不落码）

- **网络级访问控制**：DSec 用 per-sandbox eBPF allowlist 按域/镜像管控，且可随任务
  阶段动态更新。我们的授权粒度停在工具绑定层（bindings=[files,shell,web,skills]），
  web 抓取的目标域无管控。若未来出现"实例经网络取回不该看的参考实现"类需求，
  升级路径是 web 工具的目标域 allowlist 进 grants（resource_scope=domain:...）。
- **实例挂起/透明恢复**：DSec 的 pause/resume 对调用方透明（下一条请求自动唤醒）。
  我们的空转实例不持有重资源（无沙箱常驻内存），暂无对应需求；若未来接入
  重型执行后端（容器沙箱），DSec §6.3 的挂起协议是直接参照。
- **环境分层版本化**：DSec 把 base image / workspace / toolkit 独立版本化组合，
  避免 O(m·N) 重建。松散对应我们的 profile / 上下文层 / 绑定工具分层；当前规模
  无维护压力，仅作概念备案。

## 结论

DSec 对 TeamAgents 的最大价值是**方向确认**：权威状态外置+重连、有界委托、
终检不足证、无状态协调层、适配边界哲学，v2 均已在同一位置或有等价机制，且其中
两条本回合逐项核实为真。唯一直接行动输入是给 R20 的：完成判定必须用独立执行的
检查、判定当时重新观察散列、不信任实例自报——方案已覆盖，按原计划实现即可。
