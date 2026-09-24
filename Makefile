# Use the same commands locally and in CI; override CARGO_FLAGS to allow downloads.
.DEFAULT_GOAL := help
CRATES := core engine tui
CARGO_FLAGS ?= --offline --locked

.PHONY: help check fmt fmt-check lint test build pty hygiene

help:
	@echo 'make check     format, Clippy, regression tests and repository hygiene (offline by default)'
	@echo 'make fmt       format the three crates'
	@echo 'make build     build the CLI and the TUI'
	@echo 'make pty       real-terminal smoke check with an isolated config (needs Python 3)'
	@echo 'first run with downloads: make check CARGO_FLAGS=--locked'

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

# Formal verification (TLA+/TLC; not part of make check; the first run downloads the pinned tla2tools.jar)
TLA_TOOLS_DIR ?= $(HOME)/.local/share/teamagents-verify
TLA_VERSION := 1.7.1
TLA_SHA256 := d532ba31aafe17afba1130f92410d9257454ff7393d1eb2fe032f0c07f352da5

verify-tools:
	@mkdir -p "$(TLA_TOOLS_DIR)"
	@if [ ! -f "$(TLA_TOOLS_DIR)/tla2tools.jar" ]; then \
		echo "downloading TLC v$(TLA_VERSION) into $(TLA_TOOLS_DIR)"; \
		curl -fSL --connect-timeout 20 --retry 3 --retry-delay 2 -o "$(TLA_TOOLS_DIR)/tla2tools.jar" \
			https://github.com/tlaplus/tlaplus/releases/download/v$(TLA_VERSION)/tla2tools.jar; \
	fi
	@echo "$(TLA_SHA256)  $(TLA_TOOLS_DIR)/tla2tools.jar" | sha256sum -c - >/dev/null \
		|| { echo "tla2tools.jar failed its checksum (wrong version or content)" >&2; exit 1; }

verify-model: verify-tools
	@cd verification/tla && java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
		tlc2.TLC -config MC.cfg -fp 64 -workers 4 V2Control.tla

# small exhaustive configurations for every module (seconds; the wide config is verify-model-wide)
verify-model-all: verify-tools
	@cd verification/tla && for cfg in MC.cfg MC_artifact.cfg MC_wait.cfg MC_task.cfg MC_compress.cfg MC_daemon.cfg MC_checks.cfg; do \
		echo "== $$cfg =="; \
		java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
			tlc2.TLC -config $$cfg -fp 64 -workers 4 $$(case $$cfg in MC.cfg) echo V2Control.tla;; MC_wait.cfg) echo V2Wait.tla;; MC_task.cfg) echo V2Task.tla;; MC_compress.cfg) echo V2Compress.tla;; MC_daemon.cfg) echo V2Daemon.tla;; MC_checks.cfg) echo V2Checks.tla;; *) echo V2Artifact.tla;; esac) \
			| grep -E "No error|violation|violated|states generated"; \
	done


# Kani proofs (readback paging arithmetic; needs the Kani toolchain, not part of make check)
KANI_PATH = $(HOME)/.cargo/bin:$(PATH)
verify-kani:
	@PATH="$(KANI_PATH)" command -v cargo-kani >/dev/null || { \
		echo "the Kani toolchain is required: cargo install --locked kani-verifier && cargo kani setup" >&2; exit 1; }
	@cd verification/kani && PATH="$(KANI_PATH)" CARGO_TARGET_DIR=target cargo kani --lib \
		| grep -E "VERIFICATION|Complete -|failed"

verify-model-wide: verify-tools
	@cd verification/tla && java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
		tlc2.TLC -config MC_wide.cfg -fp 64 -workers 8 V2Control.tla

hygiene:
	git diff --check
	git submodule status
	@test -z "$$(git ls-files '*.pyc' '*/__pycache__/*' '*/.pytest_cache/*' '*/target/*' \
		'*.sqlite-wal' '*.sqlite-shm')" || \
		{ echo 'tracked build artifacts (compile caches, Python caches, SQLite temporaries); remove them before committing.'; exit 1; }
	sh -n install.sh
