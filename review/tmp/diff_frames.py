"""Diff the Python and Rust TUI frames across panels/sizes/languages."""
import pathlib, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
PY = ROOT / ".venv/bin/python"
DUMP = ROOT / "review/tmp/dump_py_frame.py"


def run(cmd, **kw):
    return subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, **kw)


def load(path):
    return [l.rstrip() for l in pathlib.Path(path).read_text().split("\n")]


def compare(name, size, panel, lang):
    w, h = size
    py_out, rs_out = f"/tmp/py-{name}.txt", f"/tmp/rs-{name}.txt"
    r = run([str(PY), str(DUMP), py_out, str(w), str(h), str(panel), lang])
    if r.returncode:
        print(f"[{name}] python dump failed: {r.stderr.strip()[:400]}")
        return 1
    env = {"TEAMAGENTS_DUMP_FRAME": rs_out, "TEAMAGENTS_DUMP_SIZE": f"{w}x{h}",
           "TEAMAGENTS_DUMP_PANEL": str(panel), "TEAMAGENTS_DUMP_LANG": lang}
    import os as _os
    full_env = dict(_os.environ)
    full_env.update(env)
    r = subprocess.run(["cargo", "test", "--offline", "--test", "render_tests", "frame_dump"],
                       cwd=ROOT / "tui", capture_output=True, text=True, env=full_env)
    if r.returncode:
        print(f"[{name}] rust dump failed: {r.stdout[-400:]}{r.stderr[-400:]}")
        return 1
    py, rs = load(py_out), load(rs_out)
    diffs = []
    for i in range(max(len(py), len(rs))):
        a, b = (py[i] if i < len(py) else ""), (rs[i] if i < len(rs) else "")
        if a != b:
            diffs.append((i, a, b))
    print(f"[{name}] {size[0]}x{size[1]} panel={panel} lang={lang}: {len(diffs)} diff lines")
    for i, a, b in diffs[:14]:
        print(f"   !!{i:02d} py|{a}")
        print(f"        rs|{b}")
    return len(diffs)


if __name__ == "__main__":
    total = 0
    for panel in range(7):
        total += compare(f"p{panel}", (110, 32), panel, "en")
    total += compare("zh-team", (110, 32), 0, "zh-CN")
    total += compare("narrow", (90, 24), 0, "en")
    print("TOTAL DIFF LINES:", total)
    sys.exit(0)
