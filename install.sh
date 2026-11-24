#!/bin/sh
# Build only the local checkout. Cargo may fetch its locked dependencies.
set -eu
case "${1:-}" in
    -h|--help) printf '%s\n' 'Usage: ./install.sh (from a local TeamAgents source checkout)' 'Set CARGO_INSTALL_ROOT to choose the installation prefix.'; exit 0 ;;
esac
if [ "$#" -ne 0 ]; then
    printf 'Unexpected argument; use --help.\n' >&2
    exit 2
fi
# Select the checkout's pinned Rust toolchain even when invoked from another directory.
CDPATH='' cd -- "$(dirname -- "$0")"
exec cargo install --locked --path . --bin teamagents
