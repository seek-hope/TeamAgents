# Use the same commands locally and in CI; override CARGO_FLAGS to allow downloads.
.DEFAULT_GOAL := help
CRATES := core engine tui
CARGO_FLAGS ?= --offline --locked

.PHONY: help check fmt fmt-check lint test build pty hygiene

help:
	@echo 'make check     格式、Clippy、回归测试及仓库卫生检查（默认离线）'
	@echo 'make fmt       按统一规则格式化三个 crate'
	@echo 'make build     构建 CLI 与 TUI'
	@echo 'make pty       在隔离配置下运行真终端检查（需要 Python 3）'
	@echo '首次下载依赖：make check CARGO_FLAGS=--locked'

check: fmt-check lint test hygiene

fmt:
	@set -eu; for crate in $(CRATES); do cargo fmt --manifest-path $$crate/Cargo.toml; done

fmt-check:
	@set -eu; for crate in $(CRATES); do cargo fmt --manifest-path $$crate/Cargo.toml -- --check; done

lint:
	@set -eu; for crate in $(CRATES); do \
		cargo clippy $(CARGO_FLAGS) --all-targets --manifest-path $$crate/Cargo.toml -- -D warnings; \
	done

test:
	@set -eu; for crate in $(CRATES); do cargo test $(CARGO_FLAGS) --manifest-path $$crate/Cargo.toml; done

build:
	cargo build $(CARGO_FLAGS) --manifest-path engine/Cargo.toml --bin teamagents
	cargo build $(CARGO_FLAGS) --manifest-path tui/Cargo.toml --bin teamagents-tui

pty: build
	@set -eu; check_dir=$$(mktemp -d); trap 'rm -rf "$$check_dir"' EXIT HUP INT TERM; \
		export XDG_CONFIG_HOME="$$check_dir/config" XDG_STATE_HOME="$$check_dir/state"; \
		unset TEAMAGENTS_ENGINE TEAMAGENTS_TUI TEAMAGENTS_PTY_MISSING_KEY; \
		mkdir -p "$$XDG_CONFIG_HOME/teamagents"; \
		printf '%s\n' '[models.leader_main]' 'provider = "openai"' 'model = "test"' \
			'api_key_env = "TEAMAGENTS_PTY_MISSING_KEY"' 'base_url = "http://127.0.0.1:1/v1"' \
			> "$$XDG_CONFIG_HOME/teamagents/config.toml"; \
		python3 tui/scripts/pty_smoke.py; \
		XDG_STATE_HOME="$$check_dir/click-state" python3 tui/scripts/pty_click_check.py; \
		python3 tui/scripts/pty_review_check.py

hygiene:
	git diff --check
	git submodule status
	@test -z "$$(git ls-files '*.pyc' '*/__pycache__/*')" || \
		{ echo '仓库包含已跟踪的 Python 缓存，请移除生成物。'; exit 1; }
	sh -n install.sh
