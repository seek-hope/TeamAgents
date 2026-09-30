"""Give each task the phase-scoped network policy the official protocol implies.

environment = no-network (the task/solution gets no network)
agent       = allowlist [model host] (the model API must stay reachable during the agent phase)
verifier    = public (the harness's own test setup installs pytest/uv; it is not the solution's network)
"""
import pathlib, sys

MODEL_HOST = "llmapi.paratera.com"

def patch(path: pathlib.Path) -> None:
    text = path.read_text()
    def ensure(section, lines):
        nonlocal text
        if f"[{section}]\n" in text:
            text = text.replace(f"[{section}]\n", f"[{section}]\n" + "".join(l + "\n" for l in lines), 1)
        else:
            text += f"\n[{section}]\n" + "".join(l + "\n" for l in lines)
    if 'network_mode = "no-network"' not in text:
        ensure("environment", ['network_mode = "no-network"'])
    ensure("agent", ['network_mode = "allowlist"', f'allowed_hosts = ["{MODEL_HOST}"]'])
    ensure("verifier", ['network_mode = "public"'])
    path.write_text(text)

if __name__ == "__main__":
    for name in sys.argv[1:]:
        for p in pathlib.Path(name).glob("*/task.toml"):
            patch(p)
        print("patched", name)
