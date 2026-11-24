# Run from the root of the single Cargo package. Set CARGO_NET_OFFLINE=true
# to use an already populated Cargo cache without contacting the registry.
.DEFAULT_GOAL := check
CARGO_TARGET_DIR ?= target
export CARGO_TARGET_DIR

.PHONY: check fmt fmt-check lint test build install package

check: fmt-check lint test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

lint:
	cargo clippy --locked --all-targets -- -D warnings

test:
	cargo test --locked --all-targets

build:
	cargo build --locked --bin teamagents

install:
	./install.sh

# Package for this host, not an untested cross target. SHA256SUMS names the archive.
package:
	@set -eu; \
	host=$$(rustc -vV | sed -n 's/^host: //p'); \
	version=$$(cargo pkgid --locked -p teamagents | sed 's/.*[#@]//'); \
	cargo build --locked --release --bin teamagents --target "$$host"; \
	bin="$(CARGO_TARGET_DIR)/$$host/release"; dist="$(CARGO_TARGET_DIR)/dist"; \
	"$$bin/teamagents" --help >/dev/null; \
	mkdir -p "$$dist"; name="teamagents-$$version-$$host.tar.gz"; \
	tar -C "$$bin" -czf "$$dist/$$name" teamagents; \
	(cd "$$dist" && sha256sum "$$name" > SHA256SUMS); \
	printf 'Packaged %s/%s\n' "$$dist" "$$name"
