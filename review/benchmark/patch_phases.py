"""Give each task the phase-scoped network policy the official protocol implies.

environment = no-network (the task/solution gets no network)
agent       = allowlist [model host(s)] (the model API must stay reachable during the agent phase)
verifier    = public (the harness's own test setup installs pytest/uv; it is not the solution's network)

The agent allowlist defaults to both known DeepSeek routes (`TEAMAGENTS_MODEL_HOSTS` overrides).
"""
import json
import os
import pathlib
import sys

# The endpoint(s) the agent's model route needs; both known DeepSeek routes by default so a task tree is valid
# whichever one a run points at.
MODEL_HOSTS = os.environ.get("TEAMAGENTS_MODEL_HOSTS", "api.deepseek.com,llmapi.paratera.com").split(",")


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
    ensure("agent", ['network_mode = "allowlist"', f"allowed_hosts = {json.dumps(MODEL_HOSTS)}"])
    ensure("verifier", ['network_mode = "public"'])
    path.write_text(text)

if __name__ == "__main__":
    for name in sys.argv[1:]:
        for p in pathlib.Path(name).glob("*/task.toml"):
            patch(p)
        print("patched", name)
