# TUI 时间排序、语言与运行反馈验收

日期：2026-09-12。对应用户提出的三项改进，授权记录见 `docs/DECISIONS.md` 的 D-12。

## 已实现

1. 上区任务按创建时间倒序显示，新增创建时间列；混合状态、更新时刻均不影响时间排序。刷新后仍选中原任务。
2. 默认英文，设置提供 English/中文切换并即时应用到标签、表头、状态、提示与快捷键。保留输入草稿和聊天内容；历史消息正文保留写入时的语言。偏好独立保存于 `$XDG_STATE_HOME/teamagents/ui.json`。
3. 增加持续运行指示、活动成员数与 ID、回合已用时间和最近活动。Leader 空闲时仍可感知成员运行；团队和任务行显示旋转指示与计时。等待批准、等待任务、失败、完成各有状态提示；无执行中回合时停止旋转。设置可关闭旋转动效。

使用既有 Textual 控件和 Python 标准库；没有新增依赖。动效帧只更新显示和缓存状态，不驱动团队调度或权限决策。回合时间包含排队和等待。

## 验证

- 修改前确定性套件：160 passed, 12 deselected（`tmp/tui-enhancements-baseline.txt`）。
- 最终确定性套件：**164 passed, 12 deselected**，57.86 秒（`tmp/tui-enhancements-full.txt`）。12 项真实服务测试按仓库默认标记未运行，本次不声称完成真实模型联调。
- 新增 4 项验收覆盖：跨状态时间排序与选中保持；键盘语言切换/保存/草稿与聊天保留；成员运行、动效开关、等待批准与停止；无效偏好回退与英文文案覆盖。
- Textual 实际渲染检查：120×40 英文任务面板、120×40 中文设置面板、80×30 英文等待批准状态。截图用假成员构造运行状态，不调用模型服务。

复跑：`.venv/bin/python -m pytest tests/ -q`；生成界面证据：`.venv/bin/python review/tmp/render_tui_enhancements.py`。

## 界面证据

- [英文任务与运行状态](tui-tasks-english.png)
- [中文设置](tui-settings-chinese.png)
- [窄屏与等待批准](tui-waiting-english.png)

变更限于 TUI、对应测试和说明文档；源码备份位于 `.pre-fix-backup/tui-language-activity-20260912/`，差异证据见 `tui-enhancements.patch`。

差异文件覆盖本批次已备份文件及新增源码/测试；此外 `tests/test_p6_sessions_ui.py` 与 `tests/test_p6_sessions_ui_dupkey.py` 各有一处既有断言改为默认英文文案，未包含在该备份差异中。
