## Linux package

Download the x86-64 GNU/Linux archive and `SHA256SUMS` into the same directory. Run `sha256sum --check SHA256SUMS`, extract `teamagents` and put it on `PATH`. The shell sandbox requires `bubblewrap`, `slirp4netns` and working unprivileged user and network namespaces. Before replacing a running installation, let in-flight work finish and run `teamagents stop` in each session.

## Start a session

Run `teamagents auth login` to authorize a ChatGPT subscription, then start `teamagents chat` in the project directory. Use `/model list` to see the account's visible models and `/model MODEL [EFFORT]` to select one for new tasks. For an OpenAI-compatible Chat Completions service, configure the API provider as described in the [usage guide](https://github.com/seek-hope/TeamAgents#readme).

For a scripted task, use `teamagents exec TEXT`. Add `--check COMMAND` to require a passing run of that exact command before success. `teamagents help` lists the commands and options available in the installed binary.
