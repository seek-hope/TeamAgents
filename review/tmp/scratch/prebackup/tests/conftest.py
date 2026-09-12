"""Shared test helpers: team spec builders, scripted members, session harness."""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

from teamagents.agents import FakeMember
from teamagents.models import (
    AgentSpec,
    ChannelMode,
    ChannelSpec,
    RuntimeKind,
    TeamSpec,
)
from teamagents.runtime import SessionRuntime, fake_session


def member(agent_id: str, *, role: str = "worker", binds: list[str] | None = None,
           runtime: RuntimeKind = RuntimeKind.DEEPAGENTS, profile: str = "test") -> AgentSpec:
    return AgentSpec(
        id=agent_id, name=agent_id.title(), role=role,
        runtime_kind=runtime, instructions=f"act as {agent_id}",
        model_profile=profile, tool_bindings=binds or ["files"],
    )


def leader() -> AgentSpec:
    return member("leader", role="leader", binds=["files", "shell", "web"])


def spec_of(*agents: AgentSpec, channels: list[ChannelSpec] | None = None,
            observers=None, shared_spaces=None, limits=None) -> TeamSpec:
    data: dict[str, Any] = {
        "leader_id": "leader",
        "agents": [a.model_dump() for a in agents],
        "channels": [c.model_dump() for c in (channels or [])],
    }
    if observers:
        data["observers"] = observers
    if shared_spaces:
        data["shared_spaces"] = shared_spaces
    if limits:
        data["limits"] = limits
    return TeamSpec.model_validate(data)


def task_channel(source: str, targets: list[str]) -> ChannelSpec:
    return ChannelSpec(source=source, targets=targets, mode=ChannelMode.TASK)


def msg_channel(source: str, targets: list[str]) -> ChannelSpec:
    return ChannelSpec(source=source, targets=targets, mode=ChannelMode.MESSAGE)


def scripts(**members: list[tuple]) -> dict[str, FakeMember]:
    return {name: FakeMember(name, script) for name, script in members.items()}


class Harness:
    def __init__(self, rt: SessionRuntime, members: dict[str, FakeMember]):
        self.rt = rt
        self.members = members

    async def start(self):
        await self.rt.start()
        return self

    async def user(self, text: str):
        receipt = self.rt.user_message(text)
        assert receipt.ok, receipt.error
        return receipt

    async def settle(self, timeout: float = 8.0) -> None:
        ok = await self.rt.settle(timeout)
        assert ok, "session did not settle: " + self.describe()

    def describe(self) -> str:
        runs = self.rt.store.runs_for_session(
            self.rt.session_id, ["QUEUED", "RUNNING", "WAITING_TASK", "WAITING_APPROVAL"])
        return "; ".join(f"{r.agent_id}:{r.status}" for r in runs)

    async def aclose(self):
        await self.rt.close()
        self.rt.store.close()


@pytest.fixture
async def harness_factory(tmp_path):
    created: list[Harness] = []

    async def make(spec: TeamSpec, member_scripts: dict[str, FakeMember],
                   catalog=None, pre_authorized=None, require_approval=None,
                   tool_executor=None, session_id: str = "s1",
                   barriers: dict[str, asyncio.Event] | None = None) -> Harness:
        for m in member_scripts.values():
            if barriers:
                m.barriers = {**m.barriers, **barriers}
        rt = fake_session(tmp_path, spec, member_scripts, catalog=catalog,
                          pre_authorized=pre_authorized, require_approval=require_approval,
                          tool_executor=tool_executor, session_id=session_id)
        h = Harness(rt, member_scripts)
        created.append(h)
        return await h.start()

    yield make

    for h in created:
        try:
            await h.aclose()
        except Exception:
            pass
