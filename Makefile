# Use the same commands locally and in CI; override CARGO_FLAGS to allow downloads.
.DEFAULT_GOAL := help
CRATES := core engine tui
CARGO_FLAGS ?= --offline --locked

.PHONY: help check fmt fmt-check lint test build pty probe-offline probe-models hygiene language-check \
        check-nobwrap check-broken-sandbox verify-tools verify-model verify-model-all \
        verify-model-counterexamples verify-model-wide verify-model-sim verify-kani release-rehearsal

help:
	@echo 'make check     format, Clippy, regression tests and repository hygiene (offline by default)'
	@echo 'make check-nobwrap  the same gate with the GitHub runner condition: no bwrap in PATH (D-113)'
	@echo 'make check-broken-sandbox  ... with a bwrap that cannot start a sandbox (Ubuntu 24.04 default, D-114)'
	@echo 'make fmt       format the three crates'
	@echo 'make build     build the CLI and the TUI'
	@echo 'make release-rehearsal  rehearse the release workflow locally, short of publishing (a musl release build; minutes)'
	@echo 'make pty       real-terminal smoke check with an isolated config (needs Python 3)'
	@echo 'make probe-offline  the credential-free dogfood probes: the real product, no model, no credential'
	@echo 'make probe-models   the model-requiring probes: every live half, one after another (~7 min)'
	@echo 'make language-check  reject non-English characters in code and docs (AGENTS.md rule)'
	@echo 'make fmt-check  the formatting check alone (what make check runs)'
	@echo 'make lint      Clippy on all targets, warnings as errors'
	@echo 'make test      the three crate suites under the leak guard, plus the ledger check'
	@echo 'make hygiene   the audits and the generated-document checks (make check runs it too)'
	@echo 'make verify-tools  fetch the pinned TLC jar (the verify targets do this themselves)'
	@echo 'make verify-model  the small control-plane configuration in TLC (~seconds)'
	@echo 'make verify-model-all  every TLA+ configuration, exhaustively (~4 min; verification/README.md)'
	@echo 'make verify-model-counterexamples  the negative controls, every one of which must be refuted'
	@echo 'make verify-model-wide  the wide control-plane configuration (slow, best effort)'
	@echo 'make verify-model-sim  any configuration by random simulation (minutes; invariants only)'
	@echo 'make verify-kani  the Kani proofs of the paging arithmetic'
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

# The three ways a test can leak — a live session daemon (D-111), a command runner that outlives it
# (D-226) and a scratch directory (D-131) — are checked
# by `review/leak_guard.py` around the run: it names and stops whatever appeared, instead of reporting a count
# that leaves the reader guessing and a process that poisons the next run's baseline (D-147).
test:
	@set -eu; guard=$$(mktemp); trap 'rm -f "$$guard"' EXIT HUP INT TERM; \
		python3 review/leak_guard.py snapshot "$$guard"; \
		for crate in $(CRATES); do cargo test $(CARGO_FLAGS) --manifest-path $$crate/Cargo.toml; done; \
		python3 review/leak_guard.py audit "$$guard"; \
		python3 review/test_counts.py

build:
	cargo build $(CARGO_FLAGS) --manifest-path engine/Cargo.toml --bin teamagents
	cargo build $(CARGO_FLAGS) --manifest-path tui/Cargo.toml --bin teamagents-tui

# The release path, short of publishing (D-217): the version gate, the musl-static release build of both binaries,
# the archive the workflow packages, SHA256SUMS over it and the installer, and the workflow's own smoke — install
# from the exact archive, `init` writing a config and a state root, the TUI executable. A build, not a check: the
# release profile is its own cache and full musl release builds cost minutes. `review/build_references.py` (in
# `make hygiene`) covers the files the workflow copies; this covers that the steps themselves run.
release-rehearsal:
	python3 review/release_rehearsal.py

# CI's condition, reproducible locally (D-113): the GitHub runner has no bubblewrap, so the sandboxed shell
# cannot start there. `review/nobwrap_path.py` builds a PATH with every tool except bwrap, and the whole gate
# runs in it — a test that only passes where bubblewrap exists would show up here instead of in CI.
check-nobwrap:
	@set -eu; farm=$$(mktemp -d); trap 'rm -rf "$$farm"' EXIT HUP INT TERM; \
		python3 review/nobwrap_path.py --verify "$$farm"; \
		PATH="$$farm" $(MAKE) check

# The other sandbox-less condition (D-114): bwrap is on PATH but cannot create a namespace — the default on
# Ubuntu 23.10+/24.04, where AppArmor restricts unprivileged user namespaces, and inside locked-down containers.
check-broken-sandbox:
	@set -eu; farm=$$(mktemp -d); trap 'rm -rf "$$farm"' EXIT HUP INT TERM; \
		python3 review/nobwrap_path.py --stub-bwrap --verify "$$farm"; \
		PATH="$$farm" $(MAKE) check

pty: build
	@set -eu; check_dir=$$(mktemp -d); trap 'rm -rf "$$check_dir"' EXIT HUP INT TERM; \
		export XDG_CONFIG_HOME="$$check_dir/config" XDG_STATE_HOME="$$check_dir/state" TMPDIR="$$check_dir/tmp"; \
		mkdir -p "$$TMPDIR"; \
		unset TEAMAGENTS_TUI TEAMAGENTS_PTY_MISSING_KEY; \
		mkdir -p "$$XDG_CONFIG_HOME/teamagents"; \
		printf '%s\n' '[models.leader_main]' 'provider = "openai"' 'model = "test"' \
			'api_key_env = "TEAMAGENTS_PTY_MISSING_KEY"' 'base_url = "http://127.0.0.1:1/v1"' \
			> "$$XDG_CONFIG_HOME/teamagents/config.toml"; \
		python3 tui/scripts/pty_v2_smoke.py

# The credential-free dogfood probes in one command (D-138): the real product driven end to end — the built
# CLI, real session daemons and the real TUI — with no model and no credential. It guards the two ways a probe
# can leak (a live daemon, a scratch directory) because D-111 and D-131 were both found that way. Not part of
# `make check`: the set takes about a minute.
probe-offline: build
	python3 review/dogfood/probes.py --self-check
	python3 -u review/dogfood/probes.py --set offline

# The probes that take a model, one after another (D-141): every live half of an acceptance item or decision
# that needs credentials. About seven minutes; each probe runs at its model's native window (D-36). Not part of
# `make check`: it spends real model calls.
probe-models: build
	python3 review/dogfood/probes.py --self-check
	python3 -u review/dogfood/probes.py --set models

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

# A positive target must *require* the success marker: TLC prints "Error: … violated." and still exits 0, so a
# recipe whose status is a `grep` for lines that include violations can pass while a property is broken (D-122).
verify-model: verify-tools
	@cd verification/tla && out=$$(java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
		tlc2.TLC -config MC.cfg -fp 64 -workers 4 V2Control.tla 2>&1); \
		echo "$$out" | grep -E "No error|violation|violated|states generated"; \
		printf '%s' "$$out" | grep -q "No error has been found" || { \
			echo "MC.cfg did not verify (see above)" >&2; exit 1; }

# The module each configuration runs, in one place: the exhaustive target and `verify-model-sim` both
# expand this, so they cannot disagree, and it must name *every* configuration in the directory — a
# configuration that fell through the `*)` default would silently run the wrong module (the rule
# `review/verification_catalogue.py` enforces). The control families are matched by prefix, after the
# exact arms, because shell `case` takes the first match.
CFG_CASE = case $$cfg in MC.cfg|MC_control_two.cfg) echo V2Control.tla;; MC_artifact*) echo V2Artifact.tla;; MC_wait*) echo V2Wait.tla;; MC_task*) echo V2Task.tla;; MC_approval*) echo V2Approval.tla;; MC_compress*) echo V2Compress.tla;; MC_daemon*) echo V2Daemon.tla;; MC_checks*) echo V2Checks.tla;; MC_trust*) echo V2Trust.tla;; MC_prompt*) echo V2Prompt.tla;; MC_retry*) echo V2Retry.tla;; MC_grants*) echo V2Grants.tla;; MC_authority*) echo V2Authority.tla;; MC_store.cfg|MC_store_adopt.cfg) echo V2Store.tla;; MC_retention*) echo V2Retention.tla;; MC_jobs*) echo V2Jobs.tla;; MC_inbox*) echo V2Inbox.tla;; MC_diskfull*) echo V2DiskFull.tla;; MC_coordinator*) echo V2Coordinator.tla;; MC_control*) echo V2Control.tla;; MC_wide.cfg) echo V2Control.tla;; *) echo V2Artifact.tla;; esac

# small exhaustive configurations for every module (seconds; the wide config is verify-model-wide)
verify-model-all: verify-tools
	@cd verification/tla && for cfg in MC.cfg MC_control_two.cfg MC_artifact.cfg MC_wait.cfg MC_task.cfg MC_task_two.cfg MC_approval.cfg MC_compress.cfg MC_daemon.cfg MC_daemon_stop.cfg MC_checks.cfg MC_grants.cfg MC_authority.cfg MC_store.cfg MC_retention.cfg MC_jobs.cfg MC_inbox.cfg MC_diskfull.cfg MC_coordinator.cfg MC_trust.cfg MC_prompt.cfg MC_retry.cfg; do \
		echo "== $$cfg =="; \
		out=$$(java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
			tlc2.TLC -config $$cfg -fp 64 -workers 4 $$( $(CFG_CASE) ) 2>&1); \
		echo "$$out" | grep -E "No error|violation|violated|states generated"; \
		printf '%s' "$$out" | grep -q "No error has been found" || { \
			echo "$$cfg did not verify (see above)" >&2; exit 1; }; \
		if printf '%s' "$$out" | grep -q "Warning:"; then \
			echo "$$cfg carried a TLC warning: an inconsistent model is a defect, not a note (see above)" >&2; exit 1; fi; \
	done

# The authority, inbound-boundary and store-identity claims must be *falsifiable*
# (D-61/D-63/D-87): each configuration below states a plausible mistake and must make TLC refute the
# named property. A control that verifies means the property says nothing, so it fails the target.
verify-model-counterexamples: verify-tools
	@cd verification/tla && for pair in \
		MC_authority_badview.cfg:V2Authority.tla MC_authority_trustsurface.cfg:V2Authority.tla \
		MC_authority_stalesurface.cfg:V2Authority.tla MC_control_midturninput.cfg:V2Control.tla \
		MC_control_two_disjunction.cfg:V2Control.tla MC_control_deadline.cfg:V2Control.tla \
		MC_control_reask.cfg:V2Control.tla MC_control_runtimeTail.cfg:V2Control.tla \
		MC_control_landing.cfg:V2Control.tla MC_store_adopt.cfg:V2Store.tla \
		MC_retention_evicts_live.cfg:V2Retention.tla MC_retention_evicts_evidence.cfg:V2Retention.tla \
		MC_retention_evicts_young.cfg:V2Retention.tla MC_retention_runs_disabled.cfg:V2Retention.tla \
		MC_jobs_guess_notrun.cfg:V2Jobs.tla MC_jobs_double_go.cfg:V2Jobs.tla \
		MC_jobs_late_go.cfg:V2Jobs.tla MC_jobs_spawn_first.cfg:V2Jobs.tla \
		MC_inbox_no_dedup.cfg:V2Inbox.tla MC_inbox_drop_when_full.cfg:V2Inbox.tla \
		MC_inbox_unbounded.cfg:V2Inbox.tla MC_inbox_stale_applied.cfg:V2Inbox.tla \
		MC_inbox_foreign_drain.cfg:V2Inbox.tla \
		MC_diskfull_keep_driving.cfg:V2DiskFull.tla MC_diskfull_fake_success.cfg:V2DiskFull.tla \
		MC_diskfull_park_silently.cfg:V2DiskFull.tla MC_diskfull_clear_anyway.cfg:V2DiskFull.tla \
		MC_coordinator_report.cfg:V2Coordinator.tla MC_coordinator_inherit.cfg:V2Coordinator.tla \
		MC_coordinator_shared.cfg:V2Coordinator.tla \
		MC_task_delegates_to_settled.cfg:V2Task.tla MC_task_bills_settled.cfg:V2Task.tla \
		MC_task_unordered_dependency.cfg:V2Task.tla MC_task_terminate_leaves_tasks.cfg:V2Task.tla \
		MC_compress_deletes_originals.cfg:V2Compress.tla MC_compress_lifts_coverage.cfg:V2Compress.tla \
		MC_compress_rewrites_closed.cfg:V2Compress.tla MC_daemon_rewrites_receipt.cfg:V2Daemon.tla \
		MC_daemon_rolls_back_log.cfg:V2Daemon.tla MC_daemon_reclaims_events.cfg:V2Daemon.tla \
		MC_daemon_stop_before_receipt.cfg:V2Daemon.tla \
		MC_daemon_keeps_serving_after_receipt.cfg:V2Daemon.tla \
		MC_checks_rewinds_rounds.cfg:V2Checks.tla MC_approval_drops_the_expiry.cfg:V2Approval.tla \
		MC_approval_rewrites_a_decision.cfg:V2Approval.tla \
		MC_approval_keeps_pending_on_close.cfg:V2Approval.tla \
		MC_approval_parks_without_a_row.cfg:V2Approval.tla \
		MC_artifact_gc_ignores_references.cfg:V2Artifact.tla MC_wait_closes_without_answering.cfg:V2Wait.tla \
		MC_grants_stale_offered_surface.cfg:V2Grants.tla \
		MC_trust_models_always_merge.cfg:V2Trust.tla MC_trust_project_overrides_user.cfg:V2Trust.tla \
		MC_trust_project_sets_policy.cfg:V2Trust.tla MC_trust_project_grants_itself.cfg:V2Trust.tla \
		MC_trust_refuses_silently.cfg:V2Trust.tla \
		MC_prompt_members_use_their_profile.cfg:V2Prompt.tla \
		MC_prompt_rules_frozen_at_start.cfg:V2Prompt.tla \
		MC_prompt_ignore_unreadable.cfg:V2Prompt.tla \
		MC_retry_session_constant.cfg:V2Retry.tla MC_retry_off_by_one.cfg:V2Retry.tla \
		MC_retry_parks_early.cfg:V2Retry.tla; do \
		cfg=$${pair%%:*}; spec=$${pair##*:}; \
		echo "== $$cfg (must be refuted) =="; \
		out=$$(java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
			tlc2.TLC -config $$cfg -fp 64 -workers 4 $$spec 2>&1); \
		echo "$$out" | grep -E "is violated|properties were violated" || { \
			echo "$$cfg verified instead of refuting: the property it should break may be vacuous" >&2; \
			echo "$$out" | tail -5 >&2; exit 1; }; \
		if printf '%s' "$$out" | grep -q "Warning:"; then \
			echo "$$cfg carried a TLC warning: an inconsistent model is a defect, not a note (see above)" >&2; exit 1; fi; \
	done


# Kani proofs (readback paging arithmetic; needs the Kani toolchain, not part of make check)
KANI_PATH = $(HOME)/.cargo/bin:$(PATH)
verify-kani:
	@PATH="$(KANI_PATH)" command -v cargo-kani >/dev/null || { \
		echo "the Kani toolchain is required: cargo install --locked kani-verifier && cargo kani setup" >&2; exit 1; }
	@cd verification/kani && out=$$(PATH="$(KANI_PATH)" CARGO_TARGET_DIR=target cargo kani --lib 2>&1); \
		echo "$$out" | grep -E "VERIFICATION|Complete -"; \
		printf '%s' "$$out" | grep -qE "Complete - [1-9][0-9]* successfully verified harnesses, 0 failures" || { \
			echo "the Kani proofs did not all succeed (see above)" >&2; exit 1; }

verify-model-wide: verify-tools
	@cd verification/tla && out=$$(java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
		tlc2.TLC -config MC_wide.cfg -fp 64 -workers 8 V2Control.tla 2>&1); \
		echo "$$out" | grep -E "No error|violation|violated|states generated"; \
		printf '%s' "$$out" | grep -q "No error has been found" || { \
			echo "MC_wide.cfg did not verify (see above)" >&2; exit 1; }

# Two configurations are beyond a bounded exhaustive attempt — the wide control-plane one (D-210: an hour, no
# verdict) and the task one, whose second task diverges at 43M states without converging — so this supplement
# explores a configuration by random simulation: `num` deep behaviors of depth `depth`, TLC's *invariants*
# checked (simulation checks no temporal property: that is the small configurations' job), a violation found here
# is real while *absence* proves nothing (D-211, D-215).
# `SIM_CONFIG` is the control hook: any configuration in the directory, including the refuted controls, because
# the module it runs comes from the shared `CFG_CASE`. A spec TLC refuses (an arm that fell through the default)
# is reported as "did not run", never as a violation: only the violation phrases count as one.
SIM_CONFIG ?= MC_wide.cfg
SIM_TRACES ?= 20000
SIM_DEPTH  ?= 100
SIM_SEED   ?= 11
verify-model-sim: verify-tools
	@cd verification/tla && cfg=$(SIM_CONFIG); out=$$(java -Xmx4g -XX:+UseParallelGC -cp "$(TLA_TOOLS_DIR)/tla2tools.jar" \
		tlc2.TLC -simulate num=$(SIM_TRACES) -depth $(SIM_DEPTH) -seed $(SIM_SEED) \
			-config $$cfg $$( $(CFG_CASE) ) 2>&1); \
		echo "$$out" | grep -E "states checked|violated|Error|Finished in"; \
		if printf '%s' "$$out" | grep -qE "is violated|properties were violated"; then \
			echo "$$cfg found a violation under simulation (see above)" >&2; exit 1; fi; \
		if printf '%s' "$$out" | grep -q "Error"; then \
			echo "$$cfg did not run under simulation: its module or constants were refused (see above)" >&2; exit 1; fi; \
		printf '%s' "$$out" | grep -q "states checked" || { \
			echo "the simulation printed no progress line: it did not run" >&2; exit 1; }; \
		if printf '%s' "$$out" | grep -q "Warning:"; then \
			echo "$$cfg carried a TLC warning: an inconsistent model is a defect, not a note (see above)" >&2; exit 1; fi

# Repository language rule (AGENTS.md): code and documentation are English only.
# The two exceptions are README.zh-CN.md (the Chinese README) and the frozen
# evaluation material under review/eval (task prompts, fixtures and recorded
# trial output keep their original bytes and are pinned by the run manifests).
# The pattern needs `grep -P` (GNU grep, true on the CI image and dev machines).
LANGUAGE_EXCLUDES := ':(exclude)README.zh-CN.md' ':(exclude)review/eval/**' ':(exclude)review/tmp/**'
language-check:
	@if git grep -n -I -P '[\x{3000}-\x{303f}\x{3400}-\x{4dbf}\x{4e00}-\x{9fff}\x{f900}-\x{faff}\x{ff01}-\x{ff60}\x{ffe0}-\x{ffe6}]' \
		-- . $(LANGUAGE_EXCLUDES); then \
		echo 'non-English characters above: translate them (AGENTS.md) or add a documented exception'; \
		exit 1; \
	fi

# Repository shape rule: `docs/DECISIONS.md` is the binding record, so its shape is checked (D-107):
# one heading per entry, newest-first, each with a body. `review/decisions_log.py` states the rules.
# The documentation's citations are the evidence ledger's commands (D-110): a `crate::test` or a path that
# names nothing in the tree fails `make check`; the removed-item tables in DECISIONS.md are notes, not errors.
# A command payload field nothing reads is dropped silently (D-123 was one: `error_class`); the caller's
# fields and the command layer's reads are compared per method (D-124).
# `docs/EVENTS.md` is generated from the event call sites, so a new kind or a renamed payload key must be
# documented in the same change (D-125).
# `docs/PROTOCOL.md` is generated from the daemon's two dispatchers (read methods and commands), so a new
# method, a renamed one or a changed parameter list must be documented in the same change (D-126).
# `docs/TOOLS.md` is generated from the tool schemas, so a renamed tool, a new parameter or a reworded
# model-facing description must be documented in the same change (D-127).
# `docs/CONFIG.md` is generated from the config structs, so a new key (or a renamed one) must be documented in
# the same change (D-128); `review/config_keys.py` then says whether anything reads it.
# A test that skips must say so (D-121): a bare `if <condition> { return; }` inside a test makes it a silent
# no-op, which is how two sandbox tests contributed nothing on a machine without bubblewrap.
hygiene: language-check
	git diff --check
	git submodule status
	@test -z "$$(git ls-files '*.pyc' '*/__pycache__/*' '*/.pytest_cache/*' '*/target/*' \
		'*.sqlite-wal' '*.sqlite-shm')" || \
		{ echo 'tracked build artifacts (compile caches, Python caches, SQLite temporaries); remove them before committing.'; exit 1; }
	sh -n install.sh
	python3 review/build_references.py
	python3 review/hygiene_catalogue.py
	python3 review/flag_fields.py
	python3 review/config_keys.py
	python3 review/decisions_log.py
	python3 review/decision_queue.py
	python3 review/citations.py
	python3 review/decision_citations.py
	python3 review/silent_skips.py
	python3 review/command_params.py
	python3 review/event_catalogue.py
	python3 review/protocol_catalogue.py
	python3 review/tool_catalogue.py
	python3 review/config_reference.py
	python3 review/dead_code.py
	python3 review/eval_manifests.py
	python3 review/eval_surface.py
	python3 review/verification_catalogue.py
	python3 review/exec_report.py
	python3 review/tui_keys.py
	python3 review/env_knobs.py
	python3 review/dogfood/probes.py --self-check
	python3 review/project_config_claim.py
	python3 review/release_artifact.py
	python3 review/readme_zh.py
	python3 review/doc_flags.py
	python3 review/requirement_trace.py
	python3 review/markdown_tables.py
