"""Scripted chat model + helpers for driving real Deep Agents graphs in tests."""

from __future__ import annotations

import json
import re
from typing import Any, Callable

from langchain_core.language_models.chat_models import BaseChatModel
from langchain_core.messages import AIMessage, ToolMessage
from langchain_core.outputs import ChatGeneration, ChatResult


def ai_tool(name: str, args: dict[str, Any], call_id: str | None = None) -> AIMessage:
    return AIMessage(content="", tool_calls=[{"name": name, "args": args,
                                              "id": call_id or f"call_{name}_{abs(hash(json.dumps(args, sort_keys=True))) % 10**6}"}])


def ai_text(text: str) -> AIMessage:
    return AIMessage(content=text)


def last_tool_result(messages: list) -> dict[str, Any]:
    """Parse the JSON payload of the most recent tool result."""
    for message in reversed(messages):
        if isinstance(message, ToolMessage):
            try:
                return json.loads(message.content)
            except Exception:
                return {"raw": message.content}
    return {}


def find_task_id(text: str) -> str:
    match = re.search(r"task_[0-9a-f]{12}", text)
    assert match, f"no task id in: {text[:400]}"
    return match.group(0)


class ScriptedChatModel(BaseChatModel):
    """Returns the next scripted AIMessage. Steps may be callables taking the
    full message list, so tests can derive arguments from tool results."""

    script: list[Any] = []
    calls: list[list] = []
    tool_names: list[str] = []

    @property
    def _llm_type(self) -> str:
        return "scripted"

    def bind_tools(self, tools, **kwargs):
        self.tool_names = [t.name if hasattr(t, "name") else str(t) for t in tools]
        return self

    def _generate(self, messages, stop=None, run_manager=None, **kwargs) -> ChatResult:
        self.calls.append(list(messages))
        index = min(len(self.calls) - 1, len(self.script) - 1)
        if not self.script:
            message = AIMessage(content="(no script)")
        else:
            step = self.script[index]
            message = step(list(messages)) if callable(step) else step
        return ChatResult(generations=[ChatGeneration(message=message)])
