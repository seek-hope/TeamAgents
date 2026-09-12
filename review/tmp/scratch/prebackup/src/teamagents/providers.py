"""Model profiles -> chat model instances (plan section 11).

Profiles are logical names; provider, protocol, address and key reference live
in user config. Provider-specific fields needed for tool-call continuation are
preserved by the vendor integrations; nothing here can change team permissions.
"""

from __future__ import annotations

import os
from typing import Any

from langchain_core.language_models.chat_models import BaseChatModel

from .models import ModelProfile, UserConfig


class ModelConfigError(RuntimeError):
    pass


#: protocols whose models do not advertise an `xhigh` reasoning level
NO_XHIGH_PROTOCOLS = {"deepseek"}


def normalize_effort(profile: ModelProfile) -> tuple[dict[str, Any], str | None]:
    """Map a requested reasoning effort onto what this provider can accept.

    Rule (user decision 2026-09-12): when a model does not support `xhigh`,
    the effort maps to `max` instead of being dropped or erroring out.
    """
    options = dict(profile.generation_options)
    effort = options.get("reasoning_effort")
    if isinstance(effort, str) and effort.lower() == "xhigh" and \
            profile.protocol in NO_XHIGH_PROTOCOLS:
        options["reasoning_effort"] = "max"
        return options, f"reasoning_effort xhigh -> max ({profile.protocol})"
    return options, None


def resolve_profile(catalog: UserConfig, name: str) -> ModelProfile:
    if name not in catalog.models:
        raise ModelConfigError(
            f"unknown model profile {name!r}; configure it under [models.{name}] "
            "in ~/.config/teamagents/config.toml (copy examples/config.toml from the "
            "repository as a starting point)")
    return catalog.models[name]


def _api_key(profile: ModelProfile) -> str | None:
    if profile.api_key_env:
        key = os.environ.get(profile.api_key_env)
        if not key:
            raise ModelConfigError(
                f"model profile {profile.provider!r} needs environment variable "
                f"{profile.api_key_env}; export it (secrets never enter TeamSpec)")
        return key
    return None


def build_chat_model(profile: ModelProfile,
                     overrides: dict[str, Any] | None = None) -> BaseChatModel:
    """Build the chat model for one profile. Raises ModelConfigError on gaps."""
    normalized, _note = normalize_effort(profile)
    options = {**normalized, **(overrides or {})}
    common: dict[str, Any] = {
        "model": profile.model,
        "timeout": profile.timeout,
        "max_retries": profile.max_retries,
        **options,
    }
    key = _api_key(profile)
    if key is not None:
        common["api_key"] = key
    if profile.base_url:
        common["base_url"] = profile.base_url
    match profile.protocol:
        case "anthropic":
            from langchain_anthropic import ChatAnthropic
            return ChatAnthropic(**common)
        case "deepseek":
            from langchain_deepseek import ChatDeepSeek
            return ChatDeepSeek(**common)
        case _:
            from langchain_openai import ChatOpenAI
            return ChatOpenAI(**common)
