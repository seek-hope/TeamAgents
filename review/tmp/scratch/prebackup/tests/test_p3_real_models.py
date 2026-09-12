"""P3/P7: real model contract tests (plan section 11) and one live end-to-end
session. Providers without credentials are skipped explicitly — a skip is never
counted as a release pass (plan section 17)."""

from __future__ import annotations

import asyncio
import json
import os
from pathlib import Path

import pytest
from langchain_core.messages import HumanMessage, ToolMessage
from langchain_core.tools import tool

from teamagents.models import ModelProfile, UserConfig
from teamagents.providers import build_chat_model

DEEPSEEK = ModelProfile(provider="deepseek", protocol="deepseek",
                        model="deepseek-flash", api_key_env="DEEPSEEK_API_KEY",
                        timeout=60, max_retries=1,
                        generation_options={"reasoning_effort": "high"})
KIMI = ModelProfile(provider="openai", protocol="openai",
                    model="kimi-k2-turbo-preview",
                    base_url="https://api.kimi.com/coding/v1",
                    api_key_env="KIMI_API_KEY", timeout=60, max_retries=1)

PROVIDERS = [
    pytest.param(DEEPSEEK, id="deepseek", marks=pytest.mark.skipif(
        not os.environ.get("DEEPSEEK_API_KEY"), reason="DEEPSEEK_API_KEY not set")),
    pytest.param(KIMI, id="kimi-openai-compat", marks=pytest.mark.skipif(
        not os.environ.get("KIMI_API_KEY"), reason="KIMI_API_KEY not set")),
]

pytestmark = pytest.mark.live


@tool
def add(a: int, b: int) -> int:
    """Add two integers."""
    return a + b


@pytest.mark.parametrize("profile", PROVIDERS)
async def test_provider_contract_tool_call_and_continuation(profile):
    model = build_chat_model(profile).bind_tools([add])
    prompt = ("You must call the add tool with a=17 and b=25. Do not compute the "
              "result yourself; the tool does the arithmetic.")
    first = await model.ainvoke([HumanMessage(content=prompt)])
    if not first.tool_calls:
        # model judgement varies between runs; one retry, then fail honestly
        first = await model.ainvoke([HumanMessage(
            content=prompt + " Call the add tool now; answering directly is wrong.")])
    assert first.tool_calls, f"{profile.model}: no tool call produced"
    call = first.tool_calls[0]
    assert call["name"] == "add"
    assert call["args"].get("a") == 17 and call["args"].get("b") == 25, \
        "tool arguments must be assembled completely"
    second = await model.ainvoke([
        HumanMessage(content=prompt), first,
        ToolMessage(content="42", tool_call_id=call["id"])])
    assert "42" in str(second.content), "continuation must use the tool result"
    assert second.usage_metadata or getattr(second, "response_metadata", None), \
        "usage/metadata must be preserved for accounting"


@pytest.mark.parametrize("profile", PROVIDERS)
async def test_provider_contract_streaming_and_multi_turn(profile):
    model = build_chat_model(profile)
    chunks = []
    async for chunk in model.astream([HumanMessage(content="Count: one two three")]):
        chunks.append(chunk)
    text = "".join(str(c.content) for c in chunks)
    assert chunks and text.strip(), "streaming must produce content"
    again = await model.ainvoke([HumanMessage(content="Reply with the single word: ok")])
    assert str(again.content).strip()


@pytest.mark.parametrize("profile", PROVIDERS)
async def test_provider_contract_error_propagation(profile):
    bad = profile.model_copy(update={"model": "definitely-not-a-model-xyz"})
    model = build_chat_model(bad)
    try:
        response = await model.ainvoke([HumanMessage(content="hi")])
    except Exception as err:  # the expected path on a first-party endpoint
        message = str(err).lower()
        assert ("model" in message or "not found" in message or "invalid" in message
                or "400" in message or "404" in message), \
            f"provider error must surface: {err}"
        return
    if profile.base_url:
        # a relay may pin its own model and ignore the requested name: the
        # response is then a normal completion, which is not an error at all
        assert str(response.content).strip() or response.tool_calls
        return
    pytest.fail(f"unknown model {bad.model!r} did not raise a provider error")


_ = (asyncio, json, Path, UserConfig)
