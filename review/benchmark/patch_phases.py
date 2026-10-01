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
# Optional: raise every task's own `[agent] timeout_sec` to a fixed cap. The 12-hour experiment sets this; leaving
# it unset keeps each task's own timeout, which is what "the official protocol" means.
AGENT_TIMEOUT_SEC = os.environ.get("TEAMAGENTS_AGENT_TIMEOUT_SEC")


def _set_agent_timeout(text: str, seconds: int) -> str:
    """Rewrite the `timeout_sec` in the `[agent]` section only (the `[verifier]` section has its own)."""
    import re
    match = re.search(r"(\[agent\]\n)(.*?)(\n\[)", text, re.S)
    if not match:
        return text + f"\n[agent]\ntimeout_sec = {seconds}.0\n"
    body = re.sub(r"^timeout_sec\s*=.*$", f"timeout_sec = {seconds}.0", match.group(2), flags=re.M)
    if "timeout_sec" not in body:
        body = f"timeout_sec = {seconds}.0\n" + body
    return text[: match.start(2)] + body + text[match.end(2):]


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
    if AGENT_TIMEOUT_SEC:
        text = _set_agent_timeout(text, int(AGENT_TIMEOUT_SEC))
    path.write_text(text)

if __name__ == "__main__":
    for name in sys.argv[1:]:
        for p in pathlib.Path(name).glob("*/task.toml"):
            patch(p)
        print("patched", name)
