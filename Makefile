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
		python3 tui/scripts/pty_v2_smoke.py

# 形式化验证（TLA+/TLC；不进 make check，首次运行会下载固定版本的 tla2tools.jar）
TLA_TOOLS_DIR ?= $(HOME)/.local/share/teamagents-verify
TLA_VERSION := 1.7.1
TLA_SHA256 := d532ba31aafe17afba1130f92410d9257454ff7393d1eb2fe032f0c07f352da5

verify-tools:
	@mkdir -p "$(TLA_TOOLS_DIR)"
	@if [ ! -f "$(TLA_TOOLS_DIR)/tla2tools.jar" ]; then \
		echo "下载 TLC v$(TLA_VERSION) 到 $(TLA_TOOLS_DIR)"; \
		curl -fSL --connect-timeout 20 --retry 3 --retry-delay 2 -o "$(TLA_TOOLS_DIR)/tla2tools.jar" \
			https://github.com/tlaplus/tlaplus/releases/download/v$(TLA_VERSION)/tla2tools.jar; \
	fi
	@echo "$(TLA_SHA256)  $(TLA_TOOLS_DIR)/tla2tools.jar" | sha256sum -c - >/dev/null \
		|| { echo "tla2tools.jar 校验失败（版本或内容不符）" >&2; exit 1; }

verify-model: verify-tools
	@cd verification/tla && java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
		tlc2.TLC -config MC.cfg -fp 64 -workers 4 V2Control.tla

# 全部模块的小配置穷举（秒级；宽配置另跑 verify-model-wide）
verify-model-all: verify-tools
	@cd verification/tla && for cfg in MC.cfg MC_artifact.cfg; do \
		echo "== $$cfg =="; \
		java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
			tlc2.TLC -config $$cfg -fp 64 -workers 4 $$(case $$cfg in MC.cfg) echo V2Control.tla;; *) echo V2Artifact.tla;; esac) \
			| grep -E "No error|violation|violated|states generated"; \
	done

verify-model-wide: verify-tools
	@cd verification/tla && java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
		tlc2.TLC -config MC_wide.cfg -fp 64 -workers 8 V2Control.tla

hygiene:
	git diff --check
	git submodule status
	@test -z "$$(git ls-files '*.pyc' '*/__pycache__/*')" || \
		{ echo '仓库包含已跟踪的 Python 缓存，请移除生成物。'; exit 1; }
	sh -n install.sh
