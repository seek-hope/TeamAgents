# TeamAgents 代码审查协议（v1）

> 本文件由 Leader 制定，所有审查员必须遵守。基准：`TeamAgents-Implementation-Plan.zh-CN.md`。

## 目标

对 TeamAgents 仓库当前代码实现做系统审查：正确性、安全性、持久化/恢复、并发、与基准方案的一致性、测试证据可信度。产出：每位审查员一份 `review/findings-<area>.md`；Leader 汇总为 `review/REVIEW-REPORT.md`。

## 硬性规则

1. **只读**：不得修改 `src/**`、`tests/**`、`docs/**`、`examples/**`、`pyproject.toml`、`uv.lock` 等仓库文件；只能在 `review/**` 下新建/写入文件（临时脚本放 `review/tmp/`，可清理）。
2. **不跑 live 测试**（沙箱无密钥、无网络）：不要用 `-m live`；不要联网。确定性套件可运行：
   `.venv/bin/python -m pytest tests/ -q`
   已知环境性失败（沙箱造成，不算代码缺陷，除非你能证明另有原因）：
   - `tests/test_config_cli.py::test_xhigh_maps_to_max_for_models_without_xhigh`（缺 `DEEPSEEK_API_KEY`）
   - `tests/test_p3_web_tools.py::test_ssrf_guard_blocks_internal_targets`（无 DNS，无法解析 example.com）
3. **每条发现必须可核验**：给 `file:line`、代码摘录或命令、影响、建议修复、严重度。禁止无证据的泛泛结论；推测标注「待验证」。
4. 区分三类问题：
   - A 与基准方案/文档声称不符；
   - B 实现缺陷（正确性/安全/并发/恢复/错误处理）；
   - C 通用质量问题（可维护性、死代码、边界、测试薄弱）。
5. 报告用中文，固定结构：
   - 一、范围与方法（读了哪些文件、跑了什么命令）
   - 二、结论（3–5 行）
   - 三、发现清单（ID｜严重度｜标题｜证据 `file:line`｜影响｜建议）
   - 四、与方案/文档的偏差
   - 五、未验证/存疑项
   - 六、自检（实际运行过的命令与结果）
6. 完成后向 Leader 发消息（`send_message`）：一句话结论 + Top 5 发现（ID/严重度/标题）+ 报告路径。**不要**发送完整报告正文。
7. 交叉检查：其他审查员的报告会陆续出现在 `review/findings-*.md`，你可以阅读以对齐口径，但不得改写他人文件。

## 审查基准（必须对照）

- `TeamAgents-Implementation-Plan.zh-CN.md`：§2.2 八条核心约束、DP-1..12、§4 执行模型、§5 状态机、§6.4 资源上限、§7 团队动作与信息边界、§8 拓扑变更、§9 持久化/恢复/取消、§10 适配器、§11 模型、§12 工具权限与工作目录、§13 入口、§14 配置、§16–17 阶段与验收（T1–T24）。
- `docs/DECISIONS.md`：已确认偏离（D-1..D-9）；代码若偏离方案但无 DECISIONS 记录 = 发现。
- `docs/STATUS.md`、`docs/ACCEPTANCE.md`：对状态/证据的声称是否被代码与测试支撑。

## 严重度参考

- **P0**：安全边界可绕过（权限/隔离/ACL/SSRF/密钥）、数据损坏或恢复错误、把失败/取消/超限谎报为完成、审计可信度问题。
- **P1**：关键语义错误、竞态、恢复窗口漏洞、异常被吞导致状态不可恢复。
- **P2**：边界/错误处理不当、文档与实现明显不符、测试薄弱可能掩盖回归。
- **P3**：可维护性/一致性/清理类。

## 汇总约定

- 报告路径：
  - core 域 → `review/findings-core.md`
  - runtime 域 → `review/findings-runtime.md`
  - adapters 域 → `review/findings-adapters.md`
  - surface 域 → `review/findings-surface.md`
  - verify 域 → `review/findings-verify.md`
- 另将「一句话结论 + 发现计数（P0/P1/P2/P3）」发布到 `review` 共享空间（`publish_shared`），便于交叉阅读。
- 只报告**你亲自读过并理解**的代码；没读到的模块不要臆断。
