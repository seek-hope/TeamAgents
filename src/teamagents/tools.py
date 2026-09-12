"""MCP and web tool bindings (plan section 12.1).

MCP uses the existing LangChain adapter; stdio and remote HTTP are both
supported. Tool names are prefixed with the service name so same-named tools
from different services never collide. A required service that is unreachable
blocks the member's task with a clear error; an optional service only loses
its capability.
"""

from __future__ import annotations

import ipaddress
import json
import os
import socket
from datetime import datetime, timezone
from html.parser import HTMLParser
from typing import Any
from urllib.parse import urlparse

import httpx
from langchain_core.tools import BaseTool, tool

from .models import ToolBinding, UserConfig

BUILTIN_TOOL_BINDINGS = {"files", "shell", "web"}


class ToolServiceUnavailable(RuntimeError):
    """A required MCP/web service could not be reached."""


def connection_for(binding: ToolBinding) -> dict[str, Any] | None:
    """Turn one configured binding into an MCP connection spec."""
    transport = binding.mcp_transport
    if transport in ("http", "streamable_http", "sse"):
        if not binding.url:
            raise ValueError(f"binding {binding.mcp_server!r} needs a url")
        if transport == "sse":
            return {"transport": "sse", "url": binding.url}
        return {"transport": "streamable_http", "url": binding.url}
    if not binding.command:
        raise ValueError(f"binding {binding.mcp_server!r} needs a command")
    spec: dict[str, Any] = {"transport": "stdio", "command": binding.command,
                            "args": list(binding.args), "env": binding.env or None}
    return spec


async def build_bound_tools(catalog: UserConfig, binding_names: list[str]) -> list[BaseTool]:
    """Resolve a member's tool bindings into concrete tools.

    'files' and 'shell' are native (Deep Agents backend + isolated shell);
    'web' expands to every configured web binding; anything else is an MCP
    service from user config.
    """
    selected: list[tuple[str, ToolBinding]] = []
    for name in binding_names:
        if name in BUILTIN_TOOL_BINDINGS:
            continue  # built-in capabilities, not catalog services
        binding = catalog.tools.get(name)
        if binding is None:
            raise ValueError(f"unknown tool binding {name!r}")
        if binding.kind in ("files", "shell"):
            continue
        selected.append((name, binding))
    for name, binding in catalog.tools.items():
        if ("web" in binding_names and binding.kind in ("web_search", "web_fetch")
                and (name, binding) not in selected):
            selected.append((name, binding))

    tools: list[BaseTool] = []
    for name, binding in selected:
        try:
            if binding.kind == "web_search":
                tools.append(_web_search_tool(name, binding))
            elif binding.kind == "web_fetch":
                tools.append(_web_fetch_tool(name, binding))
            else:
                tools.extend(await _load_service(name, binding))
        except Exception as e:
            if binding.required:
                raise ToolServiceUnavailable(
                    f"required tool service {name!r} is unavailable: {e}") from e
            # optional service failure only removes that capability
            continue
    return tools


# ---------------------------------------------------------------------------
# web tools (plan section 12.1: keep title, source URL, fetch time, body)
# ---------------------------------------------------------------------------


def _web_search_tool(name: str, binding: ToolBinding) -> BaseTool:
    provider = binding.provider or "anysearch"
    if provider != "anysearch":
        raise ValueError(f"unsupported web_search provider {provider!r}")

    @tool("web_search")
    async def web_search(query: str, max_results: int = 5,
                         include_content: bool = False) -> str:
        """Search the web and return title, source URL, snippet, fetch time
        (and full content when include_content=true)."""
        url = binding.url or "https://api.anysearch.com/v1/search"
        key = os.environ.get(binding.api_key_env or "") or None
        headers = {"Authorization": f"Bearer {key}"} if key else {}
        count = max(1, min(int(max_results), 20))
        async with httpx.AsyncClient(timeout=30) as client:
            response = await client.post(url, headers=headers,
                                         json={"query": query, "max_results": count})
            response.raise_for_status()
            payload = response.json()
        data = payload.get("data") or {}
        results = []
        for item in (data.get("results") or [])[:count]:
            entry = {"title": item.get("title", ""), "url": item.get("url", ""),
                     "snippet": item.get("snippet", ""),
                     "fetched_at": datetime.now(timezone.utc).isoformat(timespec="seconds")}
            if include_content and item.get("content"):
                entry["content"] = item["content"]
            results.append(entry)
        return json.dumps({"query": query, "provider": provider, "results": results},
                          ensure_ascii=False)

    return web_search


class _TextExtractor(HTMLParser):
    """Minimal HTML -> text (stdlib): title + visible text."""

    def __init__(self) -> None:
        super().__init__()
        self.parts: list[str] = []
        self.title = ""
        self._skip = 0
        self._in_title = False

    def handle_starttag(self, tag: str, attrs) -> None:
        if tag in ("script", "style", "noscript"):
            self._skip += 1
        elif tag == "title":
            self._in_title = True

    def handle_endtag(self, tag: str) -> None:
        if tag in ("script", "style", "noscript") and self._skip:
            self._skip -= 1
        elif tag == "title":
            self._in_title = False

    def handle_data(self, data: str) -> None:
        if self._in_title:
            self.title += data.strip()
        elif not self._skip and data.strip():
            self.parts.append(data.strip())


def guard_url(url: str, *, allow_private: bool = False) -> str:
    """SSRF guard for fetch tools: http(s) only, no localhost/private targets."""
    parsed = urlparse(url)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError(f"only http(s) URLs can be fetched, got {url!r}")
    if allow_private:
        return url
    try:
        infos = socket.getaddrinfo(parsed.hostname, parsed.port or
                                   (443 if parsed.scheme == "https" else 80))
    except socket.gaierror as e:
        raise ValueError(f"cannot resolve {parsed.hostname!r}: {e}") from e
    for info in infos:
        address = ipaddress.ip_address(info[4][0])
        if (address.is_private or address.is_loopback or address.is_link_local
                or address.is_reserved or address.is_multicast):
            raise ValueError(f"refusing to fetch internal address {address} for {url!r}")
    return url


def _web_fetch_tool(name: str, binding: ToolBinding) -> BaseTool:
    @tool("web_fetch")
    async def web_fetch(url: str, max_bytes: int = 2_000_000) -> str:
        """Fetch a web page and return title, source URL, fetch time and the
        readable text body (HTML only; capped)."""
        guard_url(url, allow_private=binding.env.get("allow_private") == "1")
        async with httpx.AsyncClient(timeout=30, follow_redirects=True) as client:
            response = await client.get(url, headers={"User-Agent": "TeamAgents/0.1"})
            response.raise_for_status()
            content_type = response.headers.get("content-type", "")
            if "html" not in content_type and "text/" not in content_type:
                raise ValueError(f"unsupported content type {content_type!r} for {url!r}")
            raw = response.content[:max_bytes]
        text = raw.decode(response.encoding or "utf-8", errors="replace")
        parser = _TextExtractor()
        parser.feed(text)
        body = "\n".join(parser.parts)
        return json.dumps({
            "title": parser.title or url,
            "url": str(response.url),
            "fetched_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
            "content": body[:200_000],
            "truncated": len(body) > 200_000 or len(raw) < len(response.content),
        }, ensure_ascii=False)

    return web_fetch


async def _load_service(name: str, binding: ToolBinding) -> list[BaseTool]:
    from langchain_mcp_adapters.client import MultiServerMCPClient

    connection = connection_for(binding)
    if connection is None:
        return []
    service = binding.mcp_server or name
    # ponytail: one short-lived MCP session per tool call (no leaked processes);
    # switch to a long-lived session per service if call latency matters.
    client = MultiServerMCPClient({service: connection}, tool_name_prefix=True)
    tools = list(await client.get_tools())
    if binding.tool_names:
        allowed = set(binding.tool_names)
        tools = [t for t in tools
                 if t.name in allowed or t.name.split("_", 1)[-1] in allowed]
    return tools
