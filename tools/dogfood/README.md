# Current product probes

These scripts drive the real CLI, daemon, sockets, terminal, runners and state stores. Each probe needs its own
assertions and a reproducible command; a passing scripted probe is not real-provider acceptance.

## Offline set

the 18 credential-free ones are `boundary.py`, `budget.py`, `truncation.py`, `max_retries.py`,
`project_config.py`, `sessions_prune.py`, `instructions.py`, `input_latency.py`, `tui_panels.py`,
`tui_sessions.py`, `geometry.py`, `tui_reconnect.py`, `providers.py --self-check`, `shutdown.py`,
`daemon_stop.py`, `stream_json.py`, `runners.py` and `artifacts.py`.

Run them with:

```bash
make probe-offline
# or
python3 tools/dogfood/probes.py --set offline
```

They cover state-root refusal, pre-request budget refusal, stream truncation, retry configuration, project config,
session pruning, instructions, composer latency, management TUI, session controls, geometry, reconnect, task-result
classification, graceful shutdown, daemon stop, streaming JSON, runner retirement and artifact GC.

## Model-dependent set

The model set contains `crash.py`, `unknown_outcome.py`, `cancel.py`, `lifecycle_run.py`, `deadline.py`,
`job_identity.py`, `checks.py`, `stale_check.py`, `two_gates.py`, `exec_check.py`, `queued_input.py`,
`runtime_note.py`, `hooks.py`, `skills.py`, `web.py`, `mcp.py`, `mcp_http.py`, `workspace.py`, `authority.py`,
`tui.py`, `team_ring.py`, `providers.py` and `protocols.py`.

Run only with separately bounded credentials and scope:

```bash
python3 tools/dogfood/probes.py --set models
```

Every real-model run uses the profile's declared native context window. This set is not part of `make check` and
does not by itself establish statistical task-success, cost or general environment acceptance.

## Selection and environment-specific checks

```bash
python3 tools/dogfood/probes.py --list
python3 tools/dogfood/probes.py --self-check
python3 tools/dogfood/probes.py --only checks.py
```

`ssh_pty.py` is an explicit environment-specific probe, not part of either generic set. It starts an isolated
loopback sshd and nested PTYs for its declared terminal profiles; it does not prove public-host SSH behavior.

Probes use independent scratch roots and stop only the processes they started, by pid. Keep failed-run state when
needed for diagnosis; do not kill by filename pattern or delete user sessions to make a probe clean.
