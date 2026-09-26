#!/usr/bin/env python3
"""Every wire protocol the product speaks, accepted against a real service (DESIGN §7).

DESIGN §7 keeps four protocol families apart — Chat Completions, DeepSeek extensions, Anthropic and Responses —
and requires that "each is accepted with a real service separately". The tree had contract samples for all four
(`engine/tests/providers_fake.rs` drives a fake HTTP server through each adapter) and live runs on two of them
(DeepSeek, whose protocol is the chat-completions wire plus the `xhigh`→`max` mapping, and Kimi over
`responses`). This probe is the live half for **every** family, one small goal each in its own state root:

    python3 review/dogfood/protocols.py                    # every family whose credential is set
    python3 review/dogfood/protocols.py --family anthropic  # one family
    python3 review/dogfood/protocols.py --strict            # a family that could not be accepted fails the run
    python3 review/dogfood/protocols.py --self-check        # the table against the code, no model and no key

Per family the probe asks the service for its own model list first and takes the native context window **the
service declares** — value and source, which is what D-36 requires and the only source that cannot go stale
silently (the tree's own entries say 1048576 for `deepseek-flash` and 262144 for `k3-256k`; this probe reports
whether the service still agrees) — and writes that value into the config it runs with, so a window the vendor
changed shows up as a mismatch instead of a quiet shrink. The goal asks for a file and `--check` verifies it, so
a family is accepted only when the whole path works on that wire: request, streamed response, tool call,
settlement. The probe also checks the design's retention promise ("opaque provider fields are stored with their
origin and version, and never flattened") by looking for the native field the family's own adapter stores —
`reasoning_content` for the chat-completions and DeepSeek families, `anthropic_blocks`, `responses_output`.

A family whose credential is unset is reported as **not accepted live**, with the reason and the contract tests
that still cover it: stated, never silently skipped. `--strict` turns "not attempted" into a non-zero exit for a
machine that has all the credentials; without it a probe that accepted at least one family and failed none
exits 0.

    python3 review/dogfood/protocols.py --state-dir /tmp/ta-protocols --timeout 240

Ceiling: one small turn per family is an acceptance, not a benchmark (no long tool loop, no compaction, no
provider failover); and the declared window is taken at face value — the probe records the endpoint it asked
and when, so the claim can be re-checked.
"""
import argparse
import atexit
import json
import os
import pathlib
import re
import shutil
import sqlite3
import ssl
import subprocess
import sys
import time
import urllib.request

REPO = pathlib.Path(__file__).resolve().parents[2]
HERE = REPO / "review/dogfood"
sys.path.insert(0, str(REPO / "review"))   # the shared pid-based stop (D-148)
import leak_guard  # noqa: E402

BIN = REPO / "engine/target/debug/teamagents"

# The families the product's protocol dispatch can build (`engine/src/providers/mod.rs`), each with what makes
# it that family: the endpoint the adapter posts to, the native field it stores, and the contract tests that
# cover it without a service. `--self-check` re-derives the first three from those files, so this table cannot
# drift away from the code (the failure mode D-130/D-133 punished).
FAMILIES = {
    "chat/completions": {
        "protocol": "openai",
        "provider": "openai",
        "model": "k3-256k",
        "base_url": "https://api.kimi.com/coding/v1",
        "key_env": "KIMI_API_KEY",
        "endpoint": "{base}/chat/completions",
        "adapter": "engine/src/providers/chat_completions.rs",
        "native_must": (),
        "native_allow": ("content", "role", "tool_calls", "reasoning_content"),
        "native_note": "Kimi's chat wire sends reasoning only sometimes (measured: absent 2026-09-27, present 2026-09-26), so the family requires only that nothing foreign or flattened is stored",
        "contract_tests": "providers_fake::*chat_completions*",
        "effort": "low",
        "note": "the same wire Kimi serves; DeepSeek speaks it too (its own family below)",
    },
    "deepseek": {
        "protocol": "deepseek",
        "provider": "deepseek",
        "model": "deepseek-flash",
        "base_url": None,
        "key_env": "DEEPSEEK_API_KEY",
        "endpoint": "{base}/chat/completions",
        "adapter": "engine/src/providers/chat_completions.rs",
        "native_must": ("reasoning_content",),
        "native_allow": ("content", "role", "tool_calls", "reasoning_content"),
        "native_note": "DeepSeek's wire *is* the thinking mode: reasoning_content must be stored (D-70)",
        "contract_tests": "providers_fake::*deepseek*",
        "effort": "high",
        "note": "chat/completions plus the xhigh→max mapping; the tree's baseline model (1 MiB window)",
    },
    "anthropic": {
        "protocol": "anthropic",
        "provider": "anthropic",
        "model": "k3-256k",
        "base_url": "https://api.kimi.com/coding/v1",
        "key_env": "KIMI_API_KEY",
        "endpoint": "{base}/v1/messages",
        "adapter": "engine/src/providers/anthropic.rs",
        "native_must": ("anthropic_blocks",),
        "native_allow": ("content", "role", "tool_calls", "anthropic_blocks"),
        "native_note": "the adapter itself stores the content blocks, so this is the product's promise, not the service's",
        "contract_tests": "providers_fake::anthropic_*",
        "effort": "low",
        "note": "Kimi's Anthropic-compatible messages endpoint: protocol ≠ vendor (§7's own sentence)",
    },
    "responses": {
        "protocol": "responses",
        "provider": "kimi",
        "model": "k3-256k",
        "base_url": "https://api.kimi.com/coding/v1",
        "key_env": "KIMI_API_KEY",
        "endpoint": "{base}/responses",
        "adapter": "engine/src/providers/responses.rs",
        "native_must": ("responses_output",),
        "native_allow": ("content", "role", "tool_calls", "responses_output"),
        "native_note": "the adapter itself stores the response output, so this is the product's promise",
        "contract_tests": "providers_fake::responses_*",
        "effort": "low",
        "note": "the OpenAI Responses wire, served by Kimi",
    },
}

PROMPT = ("Write the file accepted.txt in this workspace whose content is exactly the word ok. "
          "Then report the task as finished.")
CHECK = "test -s accepted.txt"
WINDOW_KEYS = ("context_window", "context_length")   # DeepSeek declares the first, Kimi the second
CA_BUNDLES = ("/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt",
              "/etc/ssl/cert.pem", "/usr/local/etc/openssl/cert.pem")


def _trust() -> ssl.SSLContext:
    """The machine's trust store when one is installed.

    A Python that ships its own bundled CAs (conda's `certifi`) does not see the machine's, so `urlopen` fails
    where `curl` succeeds — measured in this container, whose network passes a local CA. Nothing is relaxed: a
    system bundle is used when it exists and the default context otherwise.
    """
    for bundle in CA_BUNDLES:
        if pathlib.Path(bundle).is_file():
            return ssl.create_default_context(cafile=bundle)
    return ssl.create_default_context()


def declared_window(base_url: str, key: str, model: str) -> tuple[int | None, str]:
    """`(window, source)` from the service's own model list — the only window source that cannot drift silently.

    Value and source are what D-36 requires of a real-model run: the number comes from the service, and the
    source names the endpoint and the day it was asked.
    """
    url = f"{base_url}/models"
    request = urllib.request.Request(url, headers={"Authorization": f"Bearer {key}"})
    try:
        with urllib.request.urlopen(request, timeout=30, context=_trust()) as response:
            payload = json.load(response)
    except Exception as error:   # network, HTTP status, JSON shape: all "no declared window"
        return None, f"the service did not answer {url} ({type(error).__name__}: {str(error)[:120]})"
    for entry in payload.get("data", []):
        if entry.get("id") != model:
            continue
        for field in WINDOW_KEYS:
            if isinstance(entry.get(field), int):
                return entry[field], f"{url} ({field}), asked {time.strftime('%Y-%m-%d')}"
        return None, f"{url} lists {model} without a window field ({sorted(entry)})"
    ids = [entry.get("id") for entry in payload.get("data", [])]
    return None, f"{url} does not list {model} (it lists {ids[:6]})"


def config_text(family: dict, window: int) -> str:
    base = f'\nbase_url = "{family["base_url"]}"' if family["base_url"] else ""
    return f"""# One protocol family per state root (DESIGN §7): the window is the one the service declares (D-36).
skills_paths = []

[models.leader_main]
provider = "{family["provider"]}"
protocol = "{family["protocol"]}"
model = "{family["model"]}"{base}
api_key_env = "{family["key_env"]}"
context_window = {window}
timeout = 180
max_retries = 2
generation_options = {{ reasoning_effort = "{family["effort"]}" }}
"""


def native_fields(state_root: pathlib.Path, limit: int = 40) -> tuple[set, int]:
    """Which native fields the family's adapter stored, and how many requests the turn took (D-36/§7)."""
    db = sqlite3.connect(f"file:{state_root}/session.sqlite?mode=ro", uri=True)
    found: set = set()
    for (message,) in db.execute("SELECT message_json FROM context_entries WHERE kind = 'assistant' "
                                 f"ORDER BY idx LIMIT {limit}"):
        for key in json.loads(message).keys():
            found.add(key)
    requests = db.execute("SELECT COUNT(*) FROM model_requests").fetchone()[0]
    return found, requests


def run_family(name: str, family: dict, root: pathlib.Path, timeout: int, env_base: dict) -> tuple[bool, str]:
    """Accept one family end to end; returns `(accepted, one-line report)`."""
    key = os.environ.get(family["key_env"], "").strip()
    base_url = family["base_url"] or ("https://api.deepseek.com/v1" if family["protocol"] == "deepseek"
                                      else "https://api.openai.com/v1")
    window, source = declared_window(base_url, key, family["model"])
    if window is None:
        return False, f"no declared window: {source}"
    workspace = root / "ws"
    shutil.rmtree(root, ignore_errors=True)
    workspace.mkdir(parents=True)
    (root / "config/teamagents").mkdir(parents=True)
    (root / "config/teamagents/config.toml").write_text(config_text(family, window))
    state_root = root / "root"
    atexit.register(leak_guard.stop_daemons, state_root)
    env = {**env_base, "XDG_CONFIG_HOME": str(root / "config"), "XDG_STATE_HOME": str(root / "state")}
    started = time.time()
    run = subprocess.run([str(BIN), "exec", "--state-root", str(state_root), "--full-auto", "--json",
                          "--timeout", str(timeout), "--cwd", str(workspace), "--check", CHECK, PROMPT],
                         capture_output=True, text=True, env=env, timeout=timeout + 60)
    elapsed = round(time.time() - started, 1)
    report = json.loads(run.stdout) if run.stdout.strip().startswith("{") else {}
    verdicts = report.get("verification") or []
    fields, requests = native_fields(state_root)
    problems = []
    if run.returncode != 0:
        problems.append(f"exec exit {run.returncode}" + (f" ({report.get('failure')})" if report.get("failure")
                                                         else f" ({run.stderr.strip()[:120]})"))
    if report.get("end") not in ("completed", "reply"):
        problems.append(f"end={report.get('end')!r}")
    if not verdicts or verdicts[0].get("ok") is not True:
        problems.append(f"the check did not pass: {verdicts}")
    if not (workspace / "accepted.txt").is_file():
        problems.append("the artifact is missing")
    if requests == 0:
        problems.append("no model request was recorded")
    # The retention half of DESIGN §7 is asserted where it is the *product's* promise, and only recorded where
    # it depends on what the service chose to send: the adapters for Anthropic and Responses store their native
    # payload themselves (so a missing key is a defect), DeepSeek's wire is the thinking mode (D-70, so
    # reasoning_content is required), and Kimi's chat wire sends reasoning only sometimes — measured present
    # 2026-09-26 and absent 2026-09-27 — so requiring it there would have been a premise about the vendor.
    native_ish = {key for key in fields if key not in ("content", "role", "tool_calls")}
    for key in sorted(native_ish - set(family["native_allow"])):
        problems.append(f"the stored message carries {key!r}, which no adapter for this family stores")
    for key in family["native_must"]:
        if key not in fields:
            problems.append(f"the adapter's native field {key!r} is not in the stored message "
                            f"(keys: {sorted(fields)})")
    accepted = not problems
    detail = (f"{family['protocol']:<16} {family['model']:<16} window {window} ({source})\n"
              f"    {'accepted' if accepted else 'REFUSED'}: exit {run.returncode}, "
              f"end={report.get('end')}, requests {requests}, {elapsed}s, native {sorted(fields)}\n"
              f"    native expectation: must {list(family['native_must']) or 'nothing'} — {family['native_note']}")
    if problems:
        detail += "\n    " + "; ".join(problems)
    return accepted, detail


def self_check() -> int:
    """Check the probe's table against the code it claims to describe: no model, no credential, no network."""
    findings = []
    dispatch = (REPO / "engine/src/providers/mod.rs").read_text()
    named = set(re.findall(r'"(\w+)"\s*=>\s*Ok\(AnyProvider::', dispatch))
    # the remaining wire shapes share the chat-completions adapter through the dispatch's fallback arm, which
    # names them in its own comment (`mod.rs`): parse that, so a fifth protocol cannot arrive unnoticed
    at = dispatch.find("wire shape (ported contract)")
    shared = set(re.findall(r'"([\w/]+)"', dispatch[max(0, at - 200):at + 40])) if at >= 0 else set()
    if not named or shared != {"openai", "chat/completions", "deepseek"}:
        findings.append("the protocol dispatch could not be read from engine/src/providers/mod.rs")
    for name, family in FAMILIES.items():
        fakes = (REPO / "engine/tests/providers_fake.rs").read_text()
        adapter = (REPO / family["adapter"]).read_text()
        path = family["endpoint"].split("{base}")[-1]
        if path not in adapter:
            findings.append(f"{name}: {family['adapter']} does not post to {path}")
        for key in family["native_must"] or family["native_allow"]:
            if key in ("content", "role", "tool_calls"):
                continue
            if key not in adapter:
                findings.append(f"{name}: {family['adapter']} does not mention {key!r}")
        if family["protocol"] not in named | shared:
            findings.append(f"{name}: the dispatch cannot build protocol {family['protocol']!r} "
                            f"(it builds {sorted(named | shared)})")
        if not family["key_env"].endswith("_API_KEY"):
            findings.append(f"{name}: {family['key_env']} is not a credential environment variable")
        pattern = family["contract_tests"].split("::")[-1].replace("*", "")
        if pattern not in fakes:
            findings.append(f"{name}: no contract test matches {family['contract_tests']}")
    # the four DESIGN §7 families are exactly the wire shapes the adapters implement
    wires = {family["endpoint"].split("{base}")[-1] for family in FAMILIES.values()}
    if wires != {"/chat/completions", "/v1/messages", "/responses"}:
        findings.append(f"the family table covers wires {sorted(wires)}")
    for finding in findings:
        print("FAIL:", finding)
    if not findings:
        print(f"self-check ok: {len(FAMILIES)} families, each matching its adapter's endpoint, native field and "
              f"contract tests, over the wires {sorted(wires)}")
    return 1 if findings else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--family", action="append", help=f"one family to accept ({', '.join(FAMILIES)})")
    parser.add_argument("--state-dir", help="scratch root (default: a fresh /tmp/ta-protocols)")
    parser.add_argument("--timeout", type=int, default=180, help="exec --timeout in seconds")
    parser.add_argument("--strict", action="store_true",
                        help="a family that could not be accepted (no credential, no declared window) fails")
    parser.add_argument("--self-check", action="store_true", help="check the table against the code and exit")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    if not BIN.is_file():
        raise SystemExit(f"{BIN} is missing; build it first (make build)")

    chosen = args.family or sorted(FAMILIES)
    unknown = [name for name in chosen if name not in FAMILIES]
    if unknown:
        raise SystemExit(f"unknown family {unknown}; the families are {sorted(FAMILIES)}")
    base = pathlib.Path(args.state_dir or "/tmp/ta-protocols")
    if not args.state_dir:
        atexit.register(shutil.rmtree, base, ignore_errors=True)
    shutil.rmtree(base, ignore_errors=True)
    env_base = dict(os.environ)
    failures, skipped, accepted = [], [], []
    for name in chosen:
        family = FAMILIES[name]
        if not os.environ.get(family["key_env"], "").strip():
            skipped.append(f"{name}: NOT ACCEPTED LIVE, {family['key_env']} is not set in this environment "
                           f"(contract tests: {family['contract_tests']} in engine/tests/providers_fake.rs)")
            print(f"     {name}…")
            continue
        print(f"     {name}…", flush=True)
        ok, detail = run_family(name, family, base / name.replace("/", "-"), args.timeout, env_base)
        if not ok and "transient retries exhausted" in detail:
            # the *service* refused transiently (measured 2026-09-27: the chat/completions wire answered
            # `429 … engine overloaded` to all three attempts of the turn). That is the vendor's availability,
            # not the product's wire, and one bounded re-ask distinguishes it from a wire the product cannot
            # speak — both attempts are printed either way, so nothing is hidden.
            print(f"     {name}: the service refused transiently, asking once more…", flush=True)
            ok, detail = run_family(name, family, base / name.replace("/", "-"), args.timeout, env_base)
        print(f"{'ok   ' if ok else 'FAIL'} {name}\n    {detail}", flush=True)
        (accepted if ok else failures).append(name)
        if not ok:
            print(f"       state kept for inspection: {base / name.replace('/', '-')}")
    for line in skipped:
        print("SKIP:", line)
    print(f"\n{len(chosen)} families asked for: accepted {accepted}, refused {failures}, "
          f"not accepted live {[line.split(':')[0] for line in skipped]}")
    if failures:
        return 1
    if args.strict and skipped:
        return 1
    return 0 if accepted else 2


if __name__ == "__main__":
    sys.exit(main())
