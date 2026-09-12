"""P3/T19: web search (AnySearch HTTP API) and web fetch tools."""

from __future__ import annotations

import json
import os

import pytest

from teamagents.models import ToolBinding, UserConfig
from teamagents.tools import build_bound_tools, guard_url


def web_config() -> UserConfig:
    return UserConfig(tools={
        "web": ToolBinding(kind="web_search", provider="anysearch",
                           url="https://api.anysearch.com/v1/search",
                           api_key_env="ANYSEARCH_API_KEY"),
        "fetch": ToolBinding(kind="web_fetch", provider="anysearch"),
    })


def test_ssrf_guard_blocks_internal_targets():
    for bad in ("http://127.0.0.1/", "http://localhost:8080/", "http://10.0.0.5/",
                "http://169.254.169.254/latest/meta-data/", "file:///etc/passwd",
                "http://[::1]/"):
        with pytest.raises(Exception):
            guard_url(bad)
    assert guard_url("https://example.com/").startswith("https://")


@pytest.mark.live
@pytest.mark.skipif(not os.environ.get("ANYSEARCH_API_KEY"),
                    reason="ANYSEARCH_API_KEY not set")
async def test_anysearch_live_search():
    tools = await build_bound_tools(web_config(), ["web"])
    search = next(t for t in tools if t.name == "web_search")
    raw = await search.ainvoke({"query": "Python programming language", "max_results": 2})
    payload = json.loads(raw)
    assert payload["results"], "live search must return results"
    first = payload["results"][0]
    assert first["url"].startswith("http")
    assert first["title"]
    assert first["fetched_at"], "output keeps the fetch time"


@pytest.mark.live
async def test_web_fetch_live_example_com():
    tools = await build_bound_tools(web_config(), ["fetch"])
    fetch = next(t for t in tools if t.name == "web_fetch")
    payload = json.loads(await fetch.ainvoke({"url": "https://example.com/"}))
    assert "Example Domain" in payload["title"]
    assert payload["url"].startswith("https://")
    assert "Example Domain" in payload["content"]
    assert payload["fetched_at"]


async def test_member_with_web_binding_gets_both_tools_without_approval(tmp_path):
    """Web tools exist only because the user configured the service: no per-call
    approval, and an unconfigured member never sees them."""
    from conftest import leader, spec_of
    from scripted_model import ScriptedChatModel, ai_text, ai_tool
    from test_p3_deepagents_runner import build_runtime
    from conftest import Harness

    spec = spec_of(leader(), channels=[])
    spec.agents[0].tool_bindings = ["files", "web"]
    model = ScriptedChatModel(script=[ai_tool("signal_done", {}), ai_text("ok")])
    rt = build_runtime(tmp_path, spec, {"leader": model})
    rt.catalog = web_config()
    rt.runners["leader"].catalog = web_config()
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("just finish")
        assert await rt.settle(20)
        assert "web_search" in model.tool_names and "web_fetch" in model.tool_names
        assert rt.store.pending_approvals("s1") == []
    finally:
        await rt.close()
        rt.store.close()
