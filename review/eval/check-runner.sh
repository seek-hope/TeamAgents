#!/usr/bin/env bash
# 无需模型或凭据；隐藏评分契约使用真实 bubblewrap 与离线 Rust 工具链。
set -euo pipefail
python3 - "$(dirname "$0")/run.sh" <<'PY'
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

runner = Path(sys.argv[1]).resolve()
with tempfile.TemporaryDirectory(prefix="teamagents-eval-contract-") as temporary:
    root = Path(temporary)
    fake = root / "fake-agent"
    fake.write_text('''#!/usr/bin/env python3
import json, os, sys
case = os.environ["TA_EVAL_PROBE_CASE"]
resumed = "--resume" in sys.argv
if case.startswith("hidden_") or case == "resume_hidden_good" and resumed:
    from pathlib import Path
    work = Path(sys.argv[sys.argv.index("--cwd") + 1])
    if case != "hidden_bad":
        (work / "src/lib.rs").write_text("pub fn add(a: i32, b: i32) -> i32 { a + b }\\n")
    if case == "hidden_tamper":
        (work / "tests/public.rs").write_text("")
    if case == "hidden_symlink":
        source = work / "src/lib.rs"
        source.unlink()
        source.symlink_to(work / "tests/public.rs")
    if case == "hidden_added":
        (work / "build.rs").write_text('compile_error!("untrusted build script");')
phase1 = case.startswith("resume_") and not resumed
code = 124 if phase1 else (3 if case == "approval" else 0)
if case == "nonzero" or case == "resume_failed" and resumed:
    code = 1
if case == "resume_wrong_phase1" and phase1:
    code = 0
if case == "bad_json":
    print("{broken")
    sys.exit(0)
if not (case == "resume_no_session" and phase1):
    print(json.dumps({"type": "session", "session_id": "probe-session"}))
if case == "missing_result":
    sys.exit(0)
checks = [{"ok": case != "check_failed", "exit_code": 0} for _ in range(sys.argv.count("--check"))]
if case == "missing_checks":
    checks = []
status = {0: "completed", 1: "failed", 3: "approval_required", 124: "timeout"}[code]
print(json.dumps({"type": "result", "status": status, "exit_code": 1 if case == "mismatched_result" else code,
                  "duration_ms": 1, "usage": [], "verification": checks}))
sys.exit(code)
''')
    fake.chmod(0o700)
    cases = [
        ("ok", "rust-fix", 0),
        ("nonzero", "rust-fix", 1),
        ("check_failed", "rust-fix", 1),
        ("missing_checks", "rust-fix", 1),
        ("bad_json", "rust-fix", 1),
        ("missing_result", "rust-fix", 1),
        ("mismatched_result", "rust-fix", 1),
        ("approval", "approval-gate", 0),
        ("resume_ok", "resume-continue", 0),
        ("resume_wrong_phase1", "resume-continue", 1),
        ("resume_failed", "resume-continue", 1),
        ("resume_no_session", "resume-continue", 1),
        ("unknown_task", "task-does-not-exist", 2),
    ]
    for case, task, expected in cases:
        output = root / case
        output.mkdir()
        # Stale files from earlier runs must neither fail nor pass this run.
        (output / "stale.jsonl").write_text("{broken")
        env = dict(os.environ, TA_EVAL_PROBE_CASE=case)
        run = subprocess.run(["bash", str(runner), "--bin", str(fake), "--only", task, "--out", str(output)],
                             env=env, capture_output=True, text=True)
        assert run.returncode == expected, (case, run.returncode, expected, run.stdout, run.stderr)
        assert "stale" not in run.stdout, (case, run.stdout)
    run = subprocess.run(["bash", str(runner), "--only"], capture_output=True, text=True)
    assert run.returncode == 2, (run.returncode, run.stdout, run.stderr)
    print(f"评测脚本契约：{len(cases) + 1} 项通过（使用假 agent，未调用模型）")

    # An isolated task catalog exercises the complete runner -> Rust grader ->
    # bwrap chain, independently of any benchmark's reference solution.
    suite = root / "suite"
    copied_runner = suite / "review/eval/run.sh"
    copied_runner.parent.mkdir(parents=True)
    shutil.copyfile(runner, copied_runner)
    (suite / "engine").symlink_to(runner.parents[2] / "engine", target_is_directory=True)
    (suite / "core").symlink_to(runner.parents[2] / "core", target_is_directory=True)
    task = copied_runner.parent / "tasks/hidden-probe"
    fixture = task / "fixture"
    (fixture / "src").mkdir(parents=True)
    (fixture / "tests").mkdir()
    (fixture / "Cargo.toml").write_text('[package]\nname = "eval_probe"\nversion = "0.1.0"\nedition = "2021"\n')
    (fixture / "src/lib.rs").write_text('pub fn add(a: i32, b: i32) -> i32 { a - b }\n')
    (fixture / "tests/public.rs").write_text('#[test] fn zero() { assert_eq!(eval_probe::add(0, 0), 0); }\n')
    (task / "prompt.md").write_text("修复加法。\n")
    (task / "checks.txt").write_text("")
    (task / "allowed-files.txt").write_text("src/lib.rs\n")
    (task / "hidden_tests.rs").write_text('#[test] fn different_values() { assert_eq!(eval_probe::add(5, 3), 8); }\n')
    hidden_cases = [("hidden_bad", 1), ("hidden_good", 0), ("hidden_tamper", 1), ("hidden_symlink", 1), ("hidden_added", 1)]
    for case, expected in hidden_cases:
        output = root / case
        run = subprocess.run(["bash", str(copied_runner), "--bin", str(fake), "--only", "hidden-probe", "--out", str(output)],
                             env=dict(os.environ, TA_EVAL_PROBE_CASE=case), capture_output=True, text=True)
        log = (output / "hidden-probe.grade.log").read_text()
        assert run.returncode == expected, (case, run.returncode, expected, run.stdout, run.stderr, log)
        assert (output / "hidden-probe.grade.json").exists(), (case, log)
        report = json.loads((output / "hidden-probe.grade.json").read_text())
        assert report["ok"] == (expected == 0), (case, report)
        assert not (output / "work/hidden-probe/tests/hidden.rs").exists(), "hidden test leaked into agent workspace"
        if case == "hidden_bad":
            assert "different_values" in report["output"], report
        if case == "hidden_tamper":
            assert "受保护文件" in report["reason"], report
        if case == "hidden_symlink":
            assert "符号链接" in report["reason"], report
        if case == "hidden_added":
            assert "新增了未允许的文件" in report["reason"] and not report["output"], report
    # The interrupted first phase still has broken code. Grading it would
    # incorrectly fail this otherwise successful resumed task.
    (task / "resume.md").write_text("继续修复并完成。\n")
    (task / "expect.txt").write_text("124\n")
    output = root / "resume_hidden_good"
    run = subprocess.run(["bash", str(copied_runner), "--bin", str(fake), "--only", "hidden-probe", "--out", str(output)],
                         env=dict(os.environ, TA_EVAL_PROBE_CASE="resume_hidden_good"), capture_output=True, text=True)
    assert run.returncode == 0, (run.returncode, run.stdout, run.stderr)
    assert run.stdout.count("隐藏评分通过") == 1, run.stdout
    assert json.loads((output / "hidden-probe.grade.json").read_text())["ok"] is True
    print(f"隐藏评分 runner 契约：{len(hidden_cases) + 1} 项通过（真实 bwrap，未调用模型）")

    # A successful grader process is insufficient without a fresh, valid,
    # explicitly successful report. Stale successes must never be reused.
    (task / "resume.md").unlink()
    (task / "expect.txt").unlink()
    fake_tools = root / "fake-tools"
    fake_tools.mkdir()
    fake_cargo = fake_tools / "cargo"
    fake_cargo.write_text('''#!/usr/bin/env python3
import json, os
from pathlib import Path
case = os.environ["TA_EVAL_PROBE_GRADE"]
output = Path(os.environ["TA_EVAL_GRADE_OUTPUT"])
if case == "malformed":
    output.write_text("{broken")
elif case == "failed":
    output.write_text(json.dumps({"ok": False}))
elif case == "numeric":
    output.write_text(json.dumps({"ok": 1}))
elif case == "good":
    output.write_text(json.dumps({"ok": True}))
''')
    fake_cargo.chmod(0o700)
    report_cases = [("missing", 1), ("malformed", 1), ("failed", 1), ("numeric", 1), ("good", 0)]
    for case, expected in report_cases:
        output = root / f"report_{case}"
        output.mkdir()
        (output / "hidden-probe.grade.json").write_text(json.dumps({"ok": True}))
        env = dict(os.environ, TA_EVAL_PROBE_CASE="hidden_good", TA_EVAL_PROBE_GRADE=case,
                   PATH=str(fake_tools) + os.pathsep + os.environ["PATH"])
        run = subprocess.run(["bash", str(copied_runner), "--bin", str(fake), "--only", "hidden-probe", "--out", str(output)],
                             env=env, capture_output=True, text=True)
        assert run.returncode == expected, (case, run.returncode, expected, run.stdout, run.stderr)
        if case == "missing":
            assert not (output / "hidden-probe.grade.json").exists(), "stale grade report survived"
    print(f"评分报告 runner 契约：{len(report_cases)} 项通过（使用假评分进程）")
PY
