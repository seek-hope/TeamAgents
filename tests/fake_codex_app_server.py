"""A tiny `codex app-server` stand-in for deterministic adapter tests.

Run as: python fake_codex_app_server.py app-server
Modes (env FAKE_CODEX_MODE): simple (one message, completed),
approval (one command-execution approval request first), slow (runs until interrupted).
"""

from __future__ import annotations

import json
import os
import sys
import time

MODE = os.environ.get("FAKE_CODEX_MODE", "simple")
threads: dict[str, dict] = {}
turns: dict[str, dict] = {}


def send(obj: dict) -> None:
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def notify(method: str, params: dict) -> None:
    send({"method": method, "params": params})


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        method = message.get("method")
        params = message.get("params") or {}
        request_id = message.get("id")
        if request_id is None:
            continue
        if method == "initialize":
            send({"id": request_id, "result": {"userAgent": "fake-codex/0.0.1",
                                               "codexHome": os.environ.get("CODEX_HOME", "")}})
        elif method == "thread/start":
            tid = f"thr-{len(threads) + 1}"
            threads[tid] = {"id": tid, "turns": []}
            send({"id": request_id, "result": {"thread": {"id": tid}}})
            notify("thread/started", {"thread": {"id": tid}})
        elif method == "thread/read":
            thread = threads.get(params.get("threadId"), {"turns": []})
            send({"id": request_id, "result": {"thread": thread}})
        elif method == "turn/start":
            tid = params["threadId"]
            turn_id = f"turn-{len(turns) + 1}"
            turn = {"id": turn_id, "status": "inProgress", "items": []}
            turns[turn_id] = turn
            threads.setdefault(tid, {"id": tid, "turns": []})["turns"].append(turn)
            send({"id": request_id, "result": {"turn": turn}})
            notify("turn/started", {"threadId": tid, "turn": turn})
            if MODE == "approval":
                send({"id": 9001, "method": "item/commandExecution/requestApproval",
                      "params": {"threadId": tid, "turnId": turn_id, "itemId": "exec-1",
                                 "startedAtMs": int(time.time() * 1000),
                                 "command": "echo probe", "cwd": os.getcwd(),
                                 "reason": "fake approval"}})
                answer = json.loads(sys.stdin.readline())
                decision = (answer.get("result") or {}).get("decision")
                item = {"type": "agentMessage", "text": f"approval={decision}"}
                turn["items"].append(item)
                notify("item/completed", {"threadId": tid, "turnId": turn_id,
                                          "item": item})
            elif MODE == "slow":
                while turn["status"] == "inProgress":
                    raw = sys.stdin.readline()
                    if not raw:
                        return
                    follow = json.loads(raw)
                    if follow.get("method") == "turn/interrupt":
                        turn["status"] = "interrupted"
                        send({"id": follow["id"], "result": {}})
                        notify("turn/completed", {"threadId": tid, "turn": turn})
                continue
            else:
                notify("item/agentMessage/delta",
                       {"threadId": tid, "turnId": turn_id, "delta": "fake reply"})
                item = {"type": "agentMessage", "text": "fake reply"}
                turn["items"].append(item)
                notify("item/completed", {"threadId": tid, "turnId": turn_id,
                                          "item": item})
            turn["status"] = "completed"
            notify("turn/completed", {"threadId": tid, "turn": turn})
        elif method == "turn/interrupt":
            send({"id": request_id, "result": {}})
        else:
            send({"id": request_id, "error": {"code": -32601, "message": method}})


if __name__ == "__main__":
    main()
