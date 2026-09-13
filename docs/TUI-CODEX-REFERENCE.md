# Codex TUI 参考与 Textual 适配

> 下面的 Textual 映射描述 **Python（main）版**；Rust 版（`tui/`，ratatui/crossterm）按 D-20
> 重新设计了界面（固定上下分区、六个页签、`/settings` 浮层只剩界面语言），共用同一套交互
> 契约（提交/换行/历史/草稿、面板焦点、批准队列、请求停止、历史持久化文件）。
> 注意：Rust 版已删除 Animations 开关（D-20 补充六，偏好文件里的 `animations` 键仅为兼容旧文件
> 保留、不再生效）。组件名对应 `tui/src/app.rs`、`tui/src/ui.rs`、`tui/src/text.rs`。

参考版本：OpenAI Codex 源码 `c4017a87aacc7558002b7cb510025e967c1d765e`（2026-09-12 获取）。

TeamAgents 继续使用方案约定的 Python/Textual。Codex 的 Rust TUI 组件不能直接作为 Textual Widget 加载；本次复用其交互约定，在现有 TextArea、RichLog、Static 组件上实现，不引入 Rust 构建依赖，也不复制整套 TUI。

| 上游参考 | TeamAgents 对应实现 |
|---|---|
| [chat_composer.rs](https://github.com/openai/codex/blob/c4017a87aacc7558002b7cb510025e967c1d765e/codex-rs/tui/src/bottom_pane/chat_composer.rs)：输入提交、换行、状态提示 | `tui/panels.py::PromptInput` / `InputRow`：› 提示符、多行编辑、自适应高度、快捷键提示 |
| [chat_composer_history.rs](https://github.com/openai/codex/blob/c4017a87aacc7558002b7cb510025e967c1d765e/codex-rs/tui/src/bottom_pane/chat_composer_history.rs)：历史导航、相邻去重、草稿恢复 | `PromptInput.record_submission` / `recall`：提交时记录（相邻去重），持久化到 `$XDG_STATE_HOME/teamagents/composer-history.json`（上限 500 条），切换会话与重启后仍可调取；切换会话只清空草稿（`clear_composer`），历史保留 |
| [history_cell.rs](https://github.com/openai/codex/blob/c4017a87aacc7558002b7cb510025e967c1d765e/codex-rs/tui/src/history_cell.rs)：角色区分与消息展示 | `ChatLog`：角色前缀、Markdown 回复；独立流式预览，在最终事件到达后归档一次；缩放时重排 |

运行状态与输入区分开渲染；新输入仍直接交给 Leader，成员消息与权限继续由 TeamAgents 控制层处理。预览只保留最近 32,000 字符，最终回复仍从持久事件完整显示。

验证（Rust 版）：`tui/tests/render_tests.rs`、`tui/tests/app_tests.rs`；真终端脚本
`tui/scripts/pty_smoke.py`、`tui/scripts/pty_click_check.py`。
（Python（main）版对应 `tests/test_tui_composer.py`、`tests/test_p6_tui.py`；预览生成脚本 `review/tmp/render_tui.py`。）
