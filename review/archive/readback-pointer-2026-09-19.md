# 真实编码对照发现的历史读回指针缺陷（2026-09-19）

本轮沿用 D-35 的复杂编码/长任务优先级、D-28 的历史遮蔽与读回合约。
先使用同一 `rust-ledger` 任务进行真实 TeamAgents / Codex CLI 对照，再针对实测行为修复，
不是增加工具数量或仅以模拟服务通过作为产品进展。

## 实测发现与复现

首轮 TeamAgents 完整完成并通过隐藏 11/11、公开测试及文件保护，但耗时约 497.7 秒，
151 次工具调用中有 29 次 `read_history`，其中 9 次读取另一次读回的回执，最长达到 4 层。
同模型的 Codex CLI 本次约 133.5 秒结束，也通过相同验收。
任务、配置口径、原始数据和不可直接比较的客户端差异见[对照记录](eval/runs/2026-09-19-ledger-comparison/REPORT.md)。

检查 `mask_old_tool_outputs` 发现：当较早的 `read_history` 返回页被遮蔽时，
占位符会推荐其自身调用 ID，而没有保留原始工具 ID 和分页参数。
模型遵照提示读回时，获得的是序列化后的上一层读回结果，再次遮蔽会产生更深一层引用。
这解释了实测中的一种无效重复；不能据此将全部耗时差归因于该缺陷。

失败回归 `chat::tests::masking_history_pages_preserves_the_source_and_pagination_recipe`
验证分页读回经过其他工具工作后被遮蔽的场景。修复前提示为
`call read_history with tool_call_id="read-page-0"`，丢失 `source-output` 和 `offset=7900/limit=500`，
断言失败。日志：`/tmp/teamagents-readback-pointer-red-20260919.log`。

## 修复

- 从原 assistant 工具调用中识别 `read_history` 的来源及 `offset/limit`。
- 遮蔽或截断这类回执时，提示重复原来的分页请求，不让引用链指向新包装层。
- 普通工具输出仍使用自身调用 ID；原 tool_call_id 配对、完整检查点、历史树及执行次数不变。
- 工具说明明确：各页均使用原始来源 ID。

本轮保持既有 50K 工具预览、16K 旧输出保留预算和摘要阈值，没有通过改变模型窗口来隐藏问题。
不删除既有读回记录，也不自动重写历史。

## 验证

```bash
cargo test --offline --locked --manifest-path engine/Cargo.toml --lib masking_history_pages_preserves
cargo test --offline --locked --manifest-path engine/Cargo.toml --test chat_e2e masked_history_readback
make check
```

新增一项单测与一项实际 HTTP 请求回归：

- 同一页重复四次读回后被遮蔽，提示仍携带正确来源与页码，私有回执原样保留。
- 生产 ChatRunner 路径读取原始工具结果，执行其他工具、遮蔽旧页、按原页码再次读回；
  请求中两次结果完全相同，原始执行工具仅执行两次，不因读回而重做。

`make check` 通过：core **84**、engine **298**、tui **104**；包含格式、严格 Clippy 和卫生检查。
Engine 另有 1 项显式 ignored，未开启 `live_codex` 的提前返回仍不计为真实服务验收。
未修改 TUI 交互，本轮不重复 PTY。

日志：

```text
/tmp/teamagents-readback-pointer-unit-20260919.log
/tmp/teamagents-readback-pointer-wire-20260919.log
/tmp/teamagents-readback-check-20260919.log
```

修复后使用同一模型与原生 1M 窗口、同一思考档位、同一任务/时限，在新工作区另跑一次真实任务，
完整完成并通过隐藏 11/11、公开测试与文件保护。历史读回为 12 次，读取其他读回回执为 0 次，
最长链由修复前的 4 层变为本次的 1 层。总耗时 **554.9 秒**，高于修复前的 497.7 秒，
所以只确认缺陷修复与本次行为观测，不宣称整体提速。
原始结果归入上述对照记录。前后两次属于开发过程，不能作为统计性能提升率或通用排名。
