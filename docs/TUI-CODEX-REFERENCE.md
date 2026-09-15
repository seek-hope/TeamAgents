# Codex TUI 交互参考

> 本 TUI（`tui/`，ratatui/crossterm）按 D-20 设计：固定上下分区、六个页签、
> `/settings` 浮层只剩界面语言；与 Codex TUI 共用一套交互契约（提交/换行/历史/草稿、
> 面板焦点、批准队列、请求停止、历史持久化文件）。
> 注意：已删除 Animations 开关（D-20 补充六，偏好文件里的 `animations` 键仅为兼容旧文件
> 保留、不再生效）。组件名对应 `tui/src/app.rs`、`tui/src/ui.rs`、`tui/src/text.rs`。

参考版本：OpenAI Codex 源码 `c4017a87aacc7558002b7cb510025e967c1d765e`（2026-09-12 获取）。

Codex 的 TUI 组件不直接复用；本 TUI 在 ratatui 之上实现同一套交互约定，不复制其整套实现。

| 上游参考 | TeamAgents 对应实现 |
|---|---|
| [chat_composer.rs](https://github.com/openai/codex/blob/c4017a87aacc7558002b7cb510025e967c1d765e/codex-rs/tui/src/bottom_pane/chat_composer.rs)：输入提交、换行、状态提示 | `tui/src/text.rs`（输入框）：› 提示符、多行编辑、自适应高度、快捷键提示 |
| [chat_composer_history.rs](https://github.com/openai/codex/blob/c4017a87aacc7558002b7cb510025e967c1d765e/codex-rs/tui/src/bottom_pane/chat_composer_history.rs)：历史导航、相邻去重、草稿恢复 | 输入历史：提交时记录（相邻去重），持久化到 `$XDG_STATE_HOME/teamagents/composer-history.json`（上限 500 条），切换会话与重启后仍可调取；切换会话只清空草稿，历史保留 |
| [history_cell.rs](https://github.com/openai/codex/blob/c4017a87aacc7558002b7cb510025e967c1d765e/codex-rs/tui/src/history_cell.rs)：角色区分与消息展示 | 对话区：角色前缀、Markdown 回复；独立流式预览，在最终事件到达后归档一次；缩放时重排 |

运行状态与输入区分开渲染；新输入仍直接交给 Leader，成员消息与权限继续由 TeamAgents 控制层处理。
预览保留最近不超过 32,000 **字节**，截断时保留 UTF-8 字符边界；最终回复仍从持久事件完整显示。
这里描述的是 TUI 的增量展示能力：Codex 后端提供增量事件，ChatRunner 通过有界 SSE 解析器提供
OpenAI/Anthropic 文本增量；工具参数完整接收后才执行。

验证：`tui/tests/render_tests.rs`、`tui/tests/app_tests.rs`；真终端脚本
`tui/scripts/pty_smoke.py`、`tui/scripts/pty_click_check.py`。
