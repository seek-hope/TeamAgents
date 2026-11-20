# Daily model directory refresh on Linux

Configure and test `teamagents catalog refresh --json` first. The timer runs that same command without an
inference call or a running session. The service expects the CLI at `~/.local/bin/teamagents`; change
`ExecStart` before installation if necessary. Its user-manager environment must provide any seed credential
references. Never put tokens in the unit, command line or repository.

From the repository root:

```bash
mkdir -p ~/.config/systemd/user
cp -i examples/systemd/teamagents-catalog.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now teamagents-catalog.timer
systemctl --user list-timers teamagents-catalog.timer
```

The schedule is daily with up to 30 minutes of randomized delay. `Persistent=true` allows a missed run
to be picked up the next time the user manager starts. Review receipts with
`journalctl --user -u teamagents-catalog.service`. To stop future runs:

```bash
systemctl --user disable --now teamagents-catalog.timer
```
