"""Adapter regression: real shell pipeline, fake agent exit, no model or Docker.

Run with a Python environment containing Harbor 0.23.0:
    python review/eval/terminal-bench/check_adapter.py
"""
import asyncio, os, subprocess, tempfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
import teamagents_agent as adapter
from harbor.models.agent.context import AgentContext

class ProbeAgent(adapter.TeamAgentsAgent):
    @property
    def model_connection(self):
        return SimpleNamespace(env={})

class LocalEnvironment:
    async def exec(self, command, cwd=None, env=None, **kwargs):
        result = subprocess.run(command, shell=True, cwd=cwd, env={**os.environ, **(env or {})}, capture_output=True, text=True, timeout=5)
        return SimpleNamespace(stdout=result.stdout, stderr=result.stderr, return_code=result.returncode)

async def main():
    with tempfile.TemporaryDirectory(prefix='ta-adapter-exit-') as directory:
        root=Path(directory)
        for code in [0,1,3,124]:
            logs=root / str(code);logs.mkdir()
            binary=root / 'fake-teamagents'
            binary.write_text('#!/bin/sh\ncat >/dev/null\nprintf \'%s\\n\' \'{"type":"result","status":"probe","usage":[]}\'\nexit '+str(code)+'\n')
            binary.chmod(0o755)
            agent=ProbeAgent(logs_dir=logs,model_name='deepseek/deepseek-flash',workdir=str(root))
            with patch.object(adapter,'REMOTE_BINARY',str(binary)), patch.object(adapter,'EnvironmentPaths',SimpleNamespace(agent_dir=logs)):
                await agent.run('input with quotes and newlines\n',LocalEnvironment(),AgentContext())
            summary=agent.parse_logs()
            assert summary['exit_code']==code,summary
            print(f'TeamAgents exit {code} -> adapter exit {summary["exit_code"]}: PASS')
if __name__ == "__main__":
    asyncio.run(main())
