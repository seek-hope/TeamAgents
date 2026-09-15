#!/usr/bin/env bash
# 固定任务评测：每个任务是 tasks/<id>/（prompt.md + checks.txt + 可选 fixture/）。
# 每个任务在全新的工作目录里跑一次真实模型回合，结果写进 $OUT/<id>.jsonl。
# 用法：review/eval/run.sh [--bin PATH] [--timeout S] [--only ID] [--out DIR] [--profile NAME]
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
bin=${TEAMAGENTS_BIN:-$repo/engine/target/debug/teamagents}
timeout_s=600
out=${OUT:-/tmp/teamagents-eval/$(date +%Y%m%d-%H%M%S)}
only=""; keep=0
while [ $# -gt 0 ]; do
  case $1 in
    --bin) bin=$2; shift 2 ;;
    --timeout) timeout_s=$2; shift 2 ;;
    --only) only=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --keep) keep=1; shift ;;
    *) echo "未知参数 $1" >&2; exit 2 ;;
  esac
done
[ -x "$bin" ] || { echo "找不到 $bin；先 cargo build --manifest-path engine/Cargo.toml" >&2; exit 2; }
mkdir -p "$out/work" "$out/state"

for dir in "$here"/tasks/*/; do
  id=$(basename "$dir")
  [ -n "$only" ] && [ "$only" != "$id" ] && continue
  work="$out/work/$id"
  rm -rf "$work"; mkdir -p "$work"
  [ -d "$dir/fixture" ] && cp -R "$dir/fixture/." "$work/"
  checks=()
  while IFS= read -r line; do [ -n "$line" ] && checks+=(--check "$line"); done < "$dir/checks.txt"
  # per-task overrides: mode.txt (approval|full-auto), timeout.txt (seconds),
  # expect.txt (expected exit code — the task is about the CLI contract, not a file)
  mode=$(cat "$dir/mode.txt" 2>/dev/null || echo full-auto)
  task_timeout=$(cat "$dir/timeout.txt" 2>/dev/null || echo "$timeout_s")
  expect=$(cat "$dir/expect.txt" 2>/dev/null || echo "")
  flags=(); [ "$mode" = "full-auto" ] && flags+=(--full-auto)
  # optional per-task TeamSpec and user config (e.g. a Codex member profile)
  config_env=()
  if [ -f "$dir/config.toml" ]; then
    mkdir -p "$out/config/teamagents"
    cp "$dir/config.toml" "$out/config/teamagents/config.toml"
    config_env=(XDG_CONFIG_HOME="$out/config")
  fi
  [ -f "$dir/team.yaml" ] && flags+=(--team "$dir/team.yaml")
  start=$(date +%s)
  if [ -f "$dir/resume.md" ]; then
    # two phases: interrupt on purpose, then continue the same session
    env XDG_STATE_HOME="$out/state" "${config_env[@]}" TERM=dumb "$bin" exec --json --cwd "$work" ${flags[@]+"${flags[@]}"} \
      --timeout "$task_timeout" - < "$dir/prompt.md" \
      > "$out/$id.phase1.jsonl" 2> "$out/$id.phase1.stderr"
    rc=$?
    session=$(python3 - "$out/$id.phase1.jsonl" <<'PY2'
import json, sys
for line in open(sys.argv[1]):
    record = json.loads(line)
    if record.get("type") == "session":
        print(record["session_id"]) ; break
PY2
)
    verdict=""
    if [ -n "$expect" ] && [ "$rc" != "$expect" ]; then verdict=" [阶段1 期望 rc=$expect 实际 rc=$rc]"; fi
    resume_expect=$(cat "$dir/expect-resume.txt" 2>/dev/null || echo 0)
    env XDG_STATE_HOME="$out/state" "${config_env[@]}" TERM=dumb "$bin" exec --json --cwd "$work" --resume "$session" ${flags[@]+"${flags[@]}"} \
      --timeout "$timeout_s" ${checks[@]+"${checks[@]}"} - < "$dir/resume.md" \
      > "$out/$id.jsonl" 2> "$out/$id.stderr"
    rc2=$?
    [ "$rc2" = "$resume_expect" ] || verdict="$verdict [阶段2 期望 rc=$resume_expect 实际 rc=$rc2]"
    seconds=$(( $(date +%s) - start ))
    printf '%-16s 阶段1 rc=%-3s 阶段2 rc=%-3s %4ss %s%s\n' "$id" "$rc" "$rc2" "$seconds" "$out/$id.jsonl" "$verdict"
    continue
  fi
  env XDG_STATE_HOME="$out/state" "${config_env[@]}" TERM=dumb "$bin" exec --json --cwd "$work" ${flags[@]+"${flags[@]}"} \
    --timeout "$task_timeout" ${checks[@]+"${checks[@]}"} - < "$dir/prompt.md" \
    > "$out/$id.jsonl" 2> "$out/$id.stderr"
  rc=$?
  seconds=$(( $(date +%s) - start ))
  verdict=""
  if [ -n "$expect" ]; then
    if [ "$rc" = "$expect" ]; then verdict=" [rc=$expect ok]"; else verdict=" [期望 rc=$expect 实际 rc=$rc]"; fi
  fi
  printf '%-16s rc=%-3s %4ss %s%s\n' "$id" "$rc" "$seconds" "$out/$id.jsonl" "$verdict"
done

# 汇总表：只读上面的 JSONL，不重新跑任何东西
if command -v python3 > /dev/null; then
  python3 - "$out" <<'PY'
import json, pathlib, sys
out = pathlib.Path(sys.argv[1])
print()
print("| 任务 | status | exit | 秒 | tokens(prompt/completion) | 验收 |")
print("|---|---|---|---|---|---|")
for path in sorted(p for p in out.glob("*.jsonl") if ".phase1." not in p.name):
    lines = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    result = lines[-1] if lines else {}
    usage = result.get("usage") or [{}]
    prompt = sum((agent.get("usage") or {}).get("prompt_tokens", 0) for agent in usage)
    completion = sum((agent.get("usage") or {}).get("completion_tokens", 0) for agent in usage)
    checks = result.get("verification") or []
    ok = "n/a" if not checks else ("全部通过" if all(c.get("ok") for c in checks) else "有失败")
    print("| %s | %s | %s | %.1f | %s/%s | %s |" % (
        path.stem, result.get("status"), result.get("exit_code"),
        (result.get("duration_ms") or 0) / 1000, prompt, completion, ok))
PY
fi
echo
echo "原始 JSONL 与 stderr：$out"
