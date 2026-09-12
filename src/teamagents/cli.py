"""CLI entry points: run/resume, doctor, validate (plan section 13).

    teamagents                     start the TUI (P6; thin REPL until then)
    teamagents --cwd PATH          start in a working directory
    teamagents --resume SESSION    resume a session
    teamagents --full-auto         explicit user choice of full-auto mode
    teamagents doctor              check models, tools, isolation, Codex protocol
    teamagents validate TEAM_SPEC  validate an imported team definition
"""

from __future__ import annotations

import argparse
import asyncio
import importlib.metadata as md
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

from . import __version__
from .config import load_team_spec, load_user_config, sessions_dir


def version_report() -> dict[str, str]:
    pkgs = ["deepagents", "langgraph", "langgraph-checkpoint-sqlite", "langchain",
            "langchain-openai", "langchain-anthropic", "langchain-deepseek",
            "langchain-mcp-adapters", "pydantic", "textual"]
    out = {"python": sys.version.split()[0], "teamagents": __version__}
    for p in pkgs:
        try:
            out[p] = md.version(p)
        except md.PackageNotFoundError:
            out[p] = "missing"
    return out


def _check(name: str, ok: bool, detail: str = "") -> tuple[str, bool, str]:
    return name, ok, detail


def doctor() -> int:
    """Check model config, tools, isolation and Codex protocol capability."""
    results: list[tuple[str, bool, str]] = []
    versions = version_report()
    missing = [k for k, v in versions.items() if v == "missing"]
    results.append(_check("dependencies", not missing,
                          json.dumps(versions, ensure_ascii=False)
                          if not missing else f"missing: {missing}"))

    try:
        catalog = load_user_config()
        results.append(_check("user config", True,
                              f"models={sorted(catalog.models)} tools={sorted(catalog.tools)}"))
        for name, profile in catalog.models.items():
            key_env = profile.api_key_env
            if key_env:
                present = bool(os.environ.get(key_env))
                results.append(_check(f"model profile {name}", present,
                                      f"{profile.provider}/{profile.model}"
                                      + ("" if present else f" (missing env {key_env})")))
    except Exception as e:
        results.append(_check("user config", False, str(e)))

    bwrap = shutil.which("bwrap")
    if bwrap:
        probe = subprocess.run(
            ["bwrap", "--ro-bind", "/usr", "/usr", "--ro-bind", "/etc", "/etc",
             "--symlink", "usr/lib", "/lib",
             "--symlink", "usr/lib64", "/lib64", "--symlink", "usr/bin", "/bin",
             "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
             "--unshare-pid", "--unshare-net", "--die-with-parent",
             "--", "sh", "-c", "test -e /etc/hostname"],
            capture_output=True, text=True, timeout=20)
        blocked_home = subprocess.run(
            ["bwrap", "--ro-bind", "/usr", "/usr", "--ro-bind", "/etc", "/etc",
             "--symlink", "usr/lib", "/lib",
             "--symlink", "usr/lib64", "/lib64", "--symlink", "usr/bin", "/bin",
             "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
             "--unshare-pid", "--unshare-net", "--die-with-parent",
             "--", "sh", "-c", "test ! -e /home"],
            capture_output=True, text=True, timeout=20)
        results.append(_check("bubblewrap isolation", probe.returncode == 0
                              and blocked_home.returncode == 0,
                              "system files visible, home blocked under bwrap"))
    else:
        results.append(_check("bubblewrap isolation", False,
                              "bwrap not found: out-of-scope commands must ask for approval"))

    codex = shutil.which("codex")
    if codex:
        try:
            ver = subprocess.run([codex, "app-server", "--help"], capture_output=True,
                                 text=True, timeout=20)
            schema_ok, schema_detail = _codex_schema_check(codex)
            results.append(_check("codex app-server", "app-server" in ver.stdout,
                                  f"codex-cli {_codex_version(codex)}"))
            results.append(_check("codex protocol schema", schema_ok, schema_detail))
        except Exception as e:
            results.append(_check("codex app-server", False, str(e)))
    else:
        results.append(_check("codex app-server", False, "codex CLI not found"))

    state = sessions_dir()
    try:
        state.mkdir(parents=True, exist_ok=True)
        probe_file = state / ".doctor-probe"
        probe_file.write_text("ok")
        probe_file.unlink()
        results.append(_check("state directory", True, str(state)))
    except Exception as e:
        results.append(_check("state directory", False, f"{state}: {e}"))

    print(f"TeamAgents doctor (v{__version__})")
    failed = 0
    for name, ok, detail in results:
        mark = "ok  " if ok else "FAIL"
        if not ok:
            failed += 1
        print(f"  [{mark}] {name:<28} {detail}")
    return 1 if failed else 0


def _codex_version(codex: str) -> str:
    try:
        out = subprocess.run([codex, "--version"], capture_output=True, text=True, timeout=15)
        return out.stdout.strip() or out.stderr.strip()
    except Exception:
        return "unknown"


def _codex_schema_check(codex: str, out_dir: Path | None = None) -> tuple[bool, str]:
    """Generate the schema from the running CLI and confirm required methods exist."""
    import tempfile
    needed = {"initialize", "thread/start", "thread/resume", "turn/start", "turn/interrupt"}
    needed_requests = {"item/commandExecution/requestApproval",
                       "item/fileChange/requestApproval", "item/tool/requestUserInput"}
    tmp = out_dir or Path(tempfile.mkdtemp(prefix="ta-codex-schema-"))
    try:
        subprocess.run([codex, "app-server", "generate-json-schema", "--out", str(tmp)],
                       capture_output=True, text=True, timeout=90, check=True)
        client = json.loads((tmp / "ClientRequest.json").read_text())
        methods = {e["properties"]["method"]["enum"][0] for e in client["oneOf"]}
        server = json.loads((tmp / "ServerRequest.json").read_text())
        server_methods = {e["properties"]["method"]["enum"][0] for e in server["oneOf"]}
        missing = (needed - methods) | (needed_requests - server_methods)
        if missing:
            return False, f"schema from this CLI lacks: {sorted(missing)}"
        return True, f"schema generated from installed CLI ({len(methods)} methods)"
    except Exception as e:
        return False, f"schema generation failed: {e}"


def validate_spec(path: str) -> int:
    try:
        from .control import BUILTIN_TOOL_BINDINGS

        spec = load_team_spec(path)
        catalog = load_user_config()
        unknown_models = [a.model_profile for a in spec.agents
                          if a.model_profile not in catalog.models]
        unknown_tools = [t for a in spec.agents for t in a.tool_bindings
                         if t not in catalog.tools and t not in BUILTIN_TOOL_BINDINGS]
        if unknown_models or unknown_tools:
            print(f"invalid: unknown model profiles {unknown_models}, "
                  f"unknown tool bindings {unknown_tools}")
            return 1
    except Exception as e:
        print(f"invalid: {e}")
        return 1
    print(f"ok: {path} — leader={spec.leader_id} members={len(spec.agents)} "
          f"channels={len(spec.channels)} spaces={len(spec.shared_spaces)}")
    return 0


def list_sessions(verbose: bool = False) -> int:
    """`teamagents sessions`: what records exist, where, and their size."""
    from .config import sessions_dir
    from .sessions import list_sessions as inventory

    rows = inventory(include_archived=True)
    if not rows:
        print(f"没有会话记录（{sessions_dir()}）")
        return 0
    print(f"会话记录目录：{sessions_dir()}")
    for r in rows:
        updated = time.strftime("%m-%d %H:%M", time.localtime(r.updated_at)) \
            if r.updated_at else "?"
        flags = []
        if r.archived:
            flags.append("已归档")
        if r.locked:
            flags.append("运行中")
        if r.error:
            flags.append(f"读取异常:{r.error[:40]}")
        print(f"  {r.session_id:<24} {r.status:<7} 目标 {r.goal_state:<7} "
              f"事件 {r.events:<5} 任务 {r.tasks:<3} {r.size_mb:>6.1f}MB  {updated}  "
              f"{r.cwd}  {' '.join(flags)}")
        if verbose:
            print(f"      {r.path}")
    print("\n在 TUI 的“会话”面板可切换/新建/归档/删除；命令行恢复："
          "teamagents --resume <会话 id>")
    return 0


def _repl(args) -> int:
    """Line-mode fallback for dumb terminals (`--plain`)."""
    from .session import open_session

    async def run() -> int:
        rt = await open_session(cwd=Path(args.cwd) if args.cwd else Path.cwd(),
                                session_id=args.resume, full_auto=args.full_auto,
                                initial_spec=(load_team_spec(args.team) if args.team
                                              else None))
        print(f"session: {rt.session_id} (Ctrl-D to exit)")
        await rt.start()
        cursor = 0      # local event cursor for this REPL (no runtime-side cursor)
        try:
            while True:
                try:
                    line = await asyncio.to_thread(input, "you> ")
                except EOFError:
                    break
                if not line.strip():
                    continue
                receipt = rt.user_message(line)
                print(f"  [input received: {receipt.result.get('goal_id', '')}]")
                await rt.settle(timeout=600)
                for event in rt.store.events(rt.session_id, after_sequence=cursor):
                    cursor = event["sequence"]
                    _print_event(event)
        finally:
            await rt.close()
            rt.store.close()
        return 0

    return asyncio.run(run())


def _run_tui(args) -> int:
    """Product entry point: the Textual TUI over a live session."""
    from pathlib import Path as _Path

    from .session import open_session
    from .tui.app import TeamAgentsApp

    cwd = _Path(args.cwd) if args.cwd else _Path.cwd()
    initial_spec = load_team_spec(args.team) if args.team else None

    async def factory(session_id: str | None = None):
        return await open_session(cwd=cwd, session_id=session_id or args.resume,
                                  full_auto=args.full_auto, initial_spec=initial_spec)

    app = TeamAgentsApp(runtime_factory=factory, cwd=cwd)
    app.run()
    return 0


def _print_event(event) -> None:
    kind = event["kind"]
    actor = event["actor_id"]
    payload = json.loads(event["payload_json"])
    if kind == "user_message":
        return
    if kind in ("task_completed", "task_failed", "task_blocked", "task_created",
                "goal_done", "limit_reached"):
        print(f"  [{kind}] {json.dumps(payload, ensure_ascii=False)[:200]}")
    elif kind == "message":
        print(f"  [message] {actor} -> {payload.get('target')}: {payload.get('text', '')[:160]}")
    elif kind == "approval_requested":
        print(f"  [approval] {json.dumps(payload, ensure_ascii=False)[:200]}")
    elif kind == "leader_reply":
        print(f"  [Leader] {payload.get('text', '')[:2000]}")
    elif kind == "run_failed":
        error = str(payload.get("error") or "未知错误")[:400]
        print(f"  [运行失败] {actor}: {error}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="teamagents", description="TeamAgents")
    parser.add_argument("--cwd", help="working directory for the session")
    parser.add_argument("--resume", help="resume the given session id")
    parser.add_argument("--full-auto", action="store_true",
                        help="user-chosen full-auto permission mode")
    parser.add_argument("--team", help="start a new session from this TeamSpec file")
    parser.add_argument("--plain", action="store_true",
                        help="line-mode REPL instead of the full TUI")
    sub = parser.add_subparsers(dest="command")
    sub.add_parser("doctor", help="check models, tools, isolation, Codex protocol")
    val = sub.add_parser("validate", help="validate a team spec file")
    val.add_argument("spec")
    ses = sub.add_parser("sessions", help="列出本机会话记录（路径、状态、大小）")
    ses.add_argument("-v", "--verbose", action="store_true", help="显示完整路径")
    sub.add_parser("version")
    args = parser.parse_args(argv)

    match args.command:
        case "doctor":
            return doctor()
        case "validate":
            return validate_spec(args.spec)
        case "sessions":
            return list_sessions(verbose=args.verbose)
        case "version":
            print(json.dumps(version_report(), indent=2))
            return 0
        case _:
            return _repl(args) if args.plain else _run_tui(args)


if __name__ == "__main__":
    raise SystemExit(main())
