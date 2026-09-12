#!/usr/bin/env node
/** A tiny `codex app-server` stand-in (tests/fake_codex_app_server.py port).
 *  Modes (env FAKE_CODEX_MODE): simple | approval | slow */
import readline from "node:readline";

const MODE = process.env.FAKE_CODEX_MODE ?? "simple";
const threads = {};
const turns = {};

const send = (obj) => process.stdout.write(JSON.stringify(obj) + "\n");
const notify = (method, params) => send({ method, params });

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", async (line) => {
  if (!line.trim()) return;
  const message = JSON.parse(line);
  const method = message.method;
  const params = message.params ?? {};
  const id = message.id;
  // a response to our server->client request (approval decision)
  if (message.method == null && id === 9001) {
    const decision = (message.result ?? {}).decision;
    const turnId = Object.keys(turns).find((t) => turns[t].awaitingApproval);
    const turn = turns[turnId];
    if (turn) {
      const tid = turn.tid;
      const item = { type: "agentMessage", text: `approval=${decision}` };
      turn.items.push(item);
      notify("item/completed", { threadId: tid, turnId, item });
      turn.status = "completed";
      notify("turn/completed", { threadId: tid, turn });
    }
    return;
  }
  if (id == null || message.method == null) return;
  if (method === "initialize") {
    send({ id, result: { userAgent: "fake-codex/0.0.1" } });
  } else if (method === "thread/start") {
    const tid = `thr-${Object.keys(threads).length + 1}`;
    threads[tid] = { id: tid, turns: [] };
    send({ id, result: { thread: { id: tid } } });
    notify("thread/started", { thread: { id: tid } });
  } else if (method === "turn/start") {
    const tid = params.threadId;
    const turnId = `turn-${Object.keys(turns).length + 1}`;
    const turn = { id: turnId, status: "inProgress", items: [] };
    turn.tid = tid;
    turns[turnId] = turn;
    threads[tid].turns.push(turn);
    send({ id, result: { turn } });
    notify("turn/started", { threadId: tid, turn });
    if (MODE === "approval") {
      send({ id: 9001, method: "item/commandExecution/requestApproval",
            params: { threadId: tid, turnId, itemId: "exec-1", command: "echo probe", reason: "fake approval" } });
      // the answer arrives as a response line; handled below
      turns[turnId].awaitingApproval = true;
    } else if (MODE === "slow") {
      turns[turnId].slow = true; // completed on turn/interrupt
    } else {
      const item = { type: "agentMessage", text: "fake work done" };
      turn.items.push(item);
      notify("item/completed", { threadId: tid, turnId, item });
      turn.status = "completed";
      notify("turn/completed", { threadId: tid, turn });
    }
  } else if (method === "turn/interrupt") {
    const turn = turns[params.turnId];
    send({ id, result: {} });
    if (turn) {
      turn.status = "interrupted";
      notify("turn/completed", { threadId: params.threadId, turn });
    }
  } else {
    send({ id, result: {} });
  }
});

// approval decision responses arrive as {"id":9001,"result":{"decision":...}}
process.stdin.on("data", () => {});
// second readline for responses is unnecessary: handle in main loop via id check
