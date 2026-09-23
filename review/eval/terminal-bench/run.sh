#!/usr/bin/env bash
# 用 TeamAgents 跑 Terminal-Bench 2.1。默认只做真实的 Harbor 调用，参数透传给 harbor。
#
# 依赖：harbor（HARBOR_BIN 或 PATH）、Docker 守护进程、DEEPSEEK_API_KEY、musl 静态二进制。
# 静态二进制缺失时用 --build 生成：
#   CC_x86_64_unknown_linux_musl=musl-gcc \
#   cargo build --offline --release --manifest-path engine/Cargo.toml --bin teamagents \
#     --target x86_64-unknown-linux-musl            # 静态构建参数见下面 --build
# 用法：
#   review/eval/terminal-bench/run.sh -i build-cython-ext -n 1
#   review/eval/terminal-bench/run.sh --build -i build-cython-ext -n 1   # 同时重建二进制
# 结果目录：$JOBS_DIR（默认 review/eval/terminal-bench/jobs，可用 -o 覆盖）。
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)

harbor_bin=${HARBOR_BIN:-harbor}
model=${MODEL:-deepseek/deepseek-flash}
dataset=${DATASET:-terminal-bench/terminal-bench-2-1@latest}
binary=${TEAMAGENTS_BIN:-$repo/engine/target/x86_64-unknown-linux-musl/release/teamagents}
jobs_dir=${JOBS_DIR:-$repo/review/eval/terminal-bench/jobs}
# 基础设施类失败（镜像拉取 EOF、装包 404）用重试抹平；AgentTimeoutError 是真实测量结果，不重试。
max_retries=${MAX_RETRIES:-0}

if [ "${1:-}" = "--build" ]; then
  shift
  RUSTFLAGS="-C target-feature=+crt-static -C relocation-model=static" \
    CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
    cargo build --offline --release --manifest-path "$repo/engine/Cargo.toml" \
      --bin teamagents --target x86_64-unknown-linux-musl || exit 2
fi

[ -x "$binary" ] || { echo "缺少静态二进制：$binary（可用 --build 生成）" >&2; exit 2; }
if ! command -v "$harbor_bin" >/dev/null 2>&1 && [ ! -x "$harbor_bin" ]; then
  echo "找不到 harbor：$harbor_bin（uv tool install harbor / 设置 HARBOR_BIN）" >&2
  exit 2
fi

mkdir -p "$jobs_dir"
# harbor 只接受 "module:Class" 形式，适配器目录靠 PYTHONPATH 暴露
export PYTHONPATH="$here${PYTHONPATH:+:$PYTHONPATH}"
# Only historical sandboxed full-auto binaries need this capability overlay.
overlay_args=()
if [ "${SANDBOX_OVERLAY:-0}" = "1" ]; then
  overlay_args=(--extra-docker-compose "$here/teamagents-caps.yaml" --ak install_bubblewrap=true)
fi
exec "$harbor_bin" run \
  -d "$dataset" \
  -m "$model" \
  -a "teamagents_agent:TeamAgentsAgent" \
  --ak "binary_path=$binary" \
  "${overlay_args[@]}" \
  -o "$jobs_dir" \
  --max-retries "$max_retries" \
  --retry-exclude AgentTimeoutError \
  "$@"
