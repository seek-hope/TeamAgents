/** tui-worker protocol: open → user_message → state, sessions ops, errors. */
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const stateHome = mkdtempSync(join(tmpdir(), "ta-worker-state-"));
process.env.XDG_STATE_HOME ??= stateHome;
const workerPath = new URL("../src/tui-worker.ts", import.meta.url).pathname;

class WorkerClient {
  private proc: ChildProcess;
  private buf = "";
  private nextId = 1;
  private pending = new Map<number, (m: any) => void>();
  pushes: any[] = [];

  constructor() {
    this.proc = spawn("node", [workerPath], { stdio: ["pipe", "pipe", "inherit"] });
    this.proc.stdout!.on("data", (d) => {
      this.buf += d;
      let i;
      while ((i = this.buf.indexOf("\n")) >= 0) {
        const line = this.buf.slice(0, i);
        this.buf = this.buf.slice(i + 1);
        const m = JSON.parse(line);
        if (m.push) this.pushes.push(m);
        else {
          this.pending.get(m.id)?.(m);
          this.pending.delete(m.id);
        }
      }
    });
  }

  call<T = any>(method: string, params: unknown = {}): Promise<T> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, (m) => (m.error ? reject(new Error(m.error)) : resolve(m.result)));
      this.proc.stdin!.write(JSON.stringify({ id, method, params }) + "\n");
    });
  }

  async close() {
    await this.call("close").catch(() => {});
  }
}

test("worker drives a scripted session end to end", async () => {
  const w = new WorkerClient();
  try {
    const opened = await w.call("open", {
      cwd: "/tmp",
      scripts: { leader: [["call", "send_message", { target: "leader", text: "note to self" }], ["call", "signal_done", { summary: "shipped" }], ["end"]] },
    });
    assert.ok(opened.session_id.startsWith("proj_"));
    assert.ok(opened.catalog && typeof opened.catalog === "object");
    const receipt = await w.call("user_message", { text: "build it" });
    assert.equal(receipt.ok, true);
    await new Promise((r) => setTimeout(r, 1500));
    const st = await w.call("call", { method: "state", params: { after_sequence: 0 } });
    const kinds = st.events.map((e: any) => e.kind);
    assert.ok(kinds.includes("user_message"));
    assert.ok(kinds.includes("goal_done"), kinds.join(","));
    assert.equal(st.session.session_id, opened.session_id);

    const { sessions } = await w.call("list_sessions", {});
    assert.ok(sessions.some((r: any) => r.sessionId === opened.session_id));

    // switch to a new session, then archive and delete it
    const fresh = await w.call("new_session", {});
    assert.notEqual(fresh.session_id, opened.session_id);
    const archived = await w.call("archive_session", { session_id: fresh.session_id });
    assert.equal(archived.was_current, true);
    const gone = await w.call("delete_session", { session_id: opened.session_id });
    assert.equal(gone.was_current, false);
  } finally {
    await w.close();
  }
});

test("worker reports unknown methods and missing session", async () => {
  const w = new WorkerClient();
  try {
    await assert.rejects(w.call("nope"), /unknown method/);
    await assert.rejects(w.call("call", { method: "state" }), /no open session/);
  } finally {
    await w.close();
  }
});
