# /model 自定义供应商

用户要求与范围见 D-40。新增 `/model` 供应商页入口与 `/model add` 向导，复用现有三种
HTTP 适配器；手填模型 ID 即可保存和选择，不要求模型目录接口。认证只接受环境变量名。
配置跨会话保存，注释及其他设置保留，同名配置不覆盖；Codex 可选 Responses，明确的
Chat Completions/Anthropic 配置不作为 Codex 候选。旧 `openai` 配置保持原有兼容行为。

## 验证

```bash
make check
make pty
cargo test --offline --locked --manifest-path engine/Cargo.toml --test custom_providers
cargo test --offline --locked --manifest-path engine/Cargo.toml --test worker_protocol worker_adds_custom_provider
```

本机全部通过：core 152、engine 378（另有 3 项显式 ignored）、tui 109。
严格 Clippy、格式和仓库检查通过。`make pty` 的输入/退出、鼠标命中、工作区审查、
新增供应商四项检查通过。新增的 Python 脚本只用于隔离配置与状态下的终端测试。

- `custom_provider_validation_and_atomic_config_preservation`：普通/内联 TOML、注释和无关段保留，
  重名、非法输入、损坏配置、目录目标和文件锁冲突时拒绝保存。
- `added_providers_call_each_api_and_survive_reopen_without_model_discovery`：生产会话入口新增配置、
  选择模型、本地 HTTP 请求路径/认证/请求体、关闭并重开后的历史续接；三种 API 分别检查，
  假服务只接收生成请求。凭据值没有写入配置。
- `worker_adds_custom_provider_and_restores_selection_after_restart`：worker 参数校验、写入失败后
  内存目录不变、修复后重试、worker 进程重启恢复选择、新会话可见。
- `model_override`：Codex 选择/恢复 Responses；拒绝明确的 Chat Completions/Anthropic 配置。
- TUI：添加入口、输入/粘贴、协议选择、取消、原生窗口校验、失败重试、防重复提交、
  迟到结果过滤、小屏渲染；真终端验证保存并选择后写入会话覆盖。

测试开发时曾把 worker 的恢复参数写成 `session_id`，该探针实际新建了会话；
改用既有协议的 `resume` 后通过，不属于产品恢复缺陷。

本批未调用真实模型服务；本地回归与 Cargo passed 不扩大真实供应商验收范围。
