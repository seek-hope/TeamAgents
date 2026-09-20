#!/usr/bin/env bash
# 固定任务评测：每个任务是 tasks/<id>/（prompt.md + checks.txt + 可选 fixture/）。
# 每个任务在全新的工作目录里跑一次真实模型回合，结果写进 $OUT/<id>.jsonl。
# 用法：review/eval/run.sh [--bin PATH] [--timeout S] [--only ID] [--out DIR]
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
bin=${TEAMAGENTS_BIN:-$repo/engine/target/debug/teamagents}
timeout_s=600
out=${OUT:-/tmp/teamagents-eval/$(date +%Y%m%d-%H%M%S)}
only=""; keep=0
while [ $# -gt 0 ]; do
  case $1 in
    --bin|--timeout|--only|--out)
      [ $# -ge 2 ] && [ -n "$2" ] || { echo "$1 需要非空参数" >&2; exit 2; } ;;
  esac
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
command -v python3 > /dev/null || { echo "评测需要 python3 校验 JSONL" >&2; exit 2; }
selected=()
for dir in "$here"/tasks/*/; do
  [ -d "$dir" ] || continue
  id=$(basename "$dir")
  [ -n "$only" ] && [ "$only" != "$id" ] && continue
  selected+=("$dir")
done
[ ${#selected[@]} -gt 0 ] || { echo "没有匹配的评测任务：${only:-全部}" >&2; exit 2; }
mkdir -p "$out/work" "$out/state" || exit 2
failed=0
reports=()

# Hidden tests run only after the agent exits, in a fresh trusted fixture copy.
# The Rust grader validates protected files and executes candidate code in bwrap.
grade_task() {
  [ -f "$dir/hidden_tests.rs" ] || return 0
  if ! rm -f "$out/$id.grade.json"; then
    echo "$id 无法清理旧评分报告" >&2
    failed=1
    return
  fi
  local grade_rc=0
  env TA_EVAL_TASK_DIR="$dir" TA_EVAL_CANDIDATE_DIR="$work" TA_EVAL_GRADE_OUTPUT="$out/$id.grade.json" \
    cargo test --offline --manifest-path "$repo/engine/Cargo.toml" --test eval_grader grade_candidate -- --exact --ignored --nocapture \
    > "$out/$id.grade.log" 2>&1 || grade_rc=$?
  if [ "$grade_rc" -eq 0 ] && python3 - "$out/$id.grade.json" >> "$out/$id.grade.log" 2>&1 <<'PYG'
import json, sys
try:
    with open(sys.argv[1]) as report:
        result = json.load(report)
    if not isinstance(result, dict) or result.get("ok") is not True:
        raise ValueError("评分报告未明确通过")
except (OSError, ValueError) as error:
    print(f"隐藏评分报告缺失或无效：{error}", file=sys.stderr)
    sys.exit(1)
PYG
  then
    printf '%-16s 隐藏评分通过：%s\n' "$id" "$out/$id.grade.json"
  else
    printf '%-16s 隐藏评分失败：%s（执行日志：%s）\n' "$id" "$out/$id.grade.json" "$out/$id.grade.log" >&2
    failed=1
  fi
}

for dir in "${selected[@]}"; do
  id=$(basename "$dir")
  work="$out/work/$id"
  rm -rf "$work"; mkdir -p "$work"
  if [ -f "$dir/fixture-source.toml" ]; then
    if ! env TA_EVAL_TASK_DIR="$dir" TA_EVAL_STAGE_OUTPUT="$work" \
      cargo test --offline --locked --manifest-path "$repo/engine/Cargo.toml" \
      --test eval_grader stage_fixture -- --exact --ignored --nocapture \
      > "$out/$id.fixture.log" 2>&1; then
      echo "$id 固定仓库输入准备失败：$out/$id.fixture.log" >&2
      failed=1
      continue
    fi
  elif [ -d "$dir/fixture" ]; then
    cp -pR "$dir/fixture/." "$work/" || { failed=1; continue; }
  fi
  checks=()
  while IFS= read -r line; do [ -n "$line" ] && checks+=(--check "$line"); done < "$dir/checks.txt"
  # per-task overrides: mode.txt (approval|full-auto), timeout.txt (seconds),
  # expect.txt (expected exit code — the task is about the CLI contract, not a file)
  mode=$(cat "$dir/mode.txt" 2>/dev/null || echo full-auto)
  task_timeout=$(cat "$dir/timeout.txt" 2>/dev/null || echo "$timeout_s")
  expect=$(cat "$dir/expect.txt" 2>/dev/null || echo 0)
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
    reports+=("$out/$id.phase1.jsonl" "$rc" "$expect" 0)
    session=$(python3 - "$out/$id.phase1.jsonl" <<'PY2'
import json, sys
for line in open(sys.argv[1]):
    record = json.loads(line)
    if record.get("type") == "session":
        print(record["session_id"]) ; break
PY2
)
    verdict=""
    if [ "$rc" != "$expect" ]; then verdict=" [阶段1 期望 rc=$expect 实际 rc=$rc]"; failed=1; fi
    if [ -z "$session" ]; then
      echo "$id 阶段1没有有效 session_id，不能恢复" >&2
      failed=1
      continue
    fi
    resume_expect=$(cat "$dir/expect-resume.txt" 2>/dev/null || echo 0)
    env XDG_STATE_HOME="$out/state" "${config_env[@]}" TERM=dumb "$bin" exec --json --cwd "$work" --resume "$session" ${flags[@]+"${flags[@]}"} \
      --timeout "$timeout_s" ${checks[@]+"${checks[@]}"} - < "$dir/resume.md" \
      > "$out/$id.jsonl" 2> "$out/$id.stderr"
    rc2=$?
    reports+=("$out/$id.jsonl" "$rc2" "$resume_expect" "$((${#checks[@]} / 2))")
    if [ "$rc2" != "$resume_expect" ]; then verdict="$verdict [阶段2 期望 rc=$resume_expect 实际 rc=$rc2]"; failed=1; fi
    seconds=$(( $(date +%s) - start ))
    printf '%-16s 阶段1 rc=%-3s 阶段2 rc=%-3s %4ss %s%s\n' "$id" "$rc" "$rc2" "$seconds" "$out/$id.jsonl" "$verdict"
    grade_task
    continue
  fi
  env XDG_STATE_HOME="$out/state" "${config_env[@]}" TERM=dumb "$bin" exec --json --cwd "$work" ${flags[@]+"${flags[@]}"} \
    --timeout "$task_timeout" ${checks[@]+"${checks[@]}"} - < "$dir/prompt.md" \
    > "$out/$id.jsonl" 2> "$out/$id.stderr"
  rc=$?
  reports+=("$out/$id.jsonl" "$rc" "$expect" "$((${#checks[@]} / 2))")
  seconds=$(( $(date +%s) - start ))
  verdict=""
  if [ "$rc" = "$expect" ]; then verdict=" [rc=$expect ok]"; else verdict=" [期望 rc=$expect 实际 rc=$rc]"; failed=1; fi
  printf '%-16s rc=%-3s %4ss %s%s\n' "$id" "$rc" "$seconds" "$out/$id.jsonl" "$verdict"
  grade_task
done

# Validate only this invocation's results, including both resume phases.
# Existing JSONL files in --out must not count as freshly executed tasks.
python3 - "${reports[@]}" <<'PY'
import json, pathlib, sys
failed = False
print()
print("| 任务 | status | exit | 秒 | tokens(prompt/completion) | 验收 |")
print("|---|---|---|---|---|---|")
args = sys.argv[1:]
for offset in range(0, len(args), 4):
    path = pathlib.Path(args[offset])
    try:
        actual, expected, check_count = map(int, args[offset + 1:offset + 4])
        lines = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
        if not lines or not isinstance(lines[-1], dict) or lines[-1].get("type") != "result":
            raise ValueError("缺少最终 result 记录")
        result = lines[-1]
        if actual != expected or result.get("exit_code") != actual:
            raise ValueError("进程退出码、期望退出码与 result.exit_code 不一致")
        if actual == 0 and result.get("status") != "completed":
            raise ValueError("退出码为 0，但任务未完成")
        checks = result.get("verification") or []
        if len(checks) != check_count or not all(c.get("ok") is True for c in checks):
            raise ValueError("验收命令缺失或失败")
        if ".phase1." in path.name:
            continue
        usage = result.get("usage") or [{}]
        prompt = sum((agent.get("usage") or {}).get("prompt_tokens", 0) for agent in usage)
        completion = sum((agent.get("usage") or {}).get("completion_tokens", 0) for agent in usage)
        print("| %s | %s | %s | %.1f | %s/%s | %s |" % (
            path.stem, result.get("status"), result.get("exit_code"),
            (result.get("duration_ms") or 0) / 1000, prompt, completion,
            "全部通过" if checks else "n/a"))
    except (OSError, ValueError, TypeError, AttributeError, KeyError) as error:
        failed = True
        print("| %s | 失败 | — | — | — | %s |" % (path.stem, error))
sys.exit(1 if failed else 0)
PY
[ $? -eq 0 ] || failed=1
echo
echo "原始 JSONL 与 stderr：$out"
exit "$failed"
