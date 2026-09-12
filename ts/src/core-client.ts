/**
 * Client for the teamagents-core stdio JSON-lines service.
 * Spawns the Rust binary, one request per line, one response per line.
 */
import { spawn, type ChildProcess } from "node:child_process";
import readline from "node:readline";

export interface Receipt {
  action_id: string;
  ok: boolean;
  kind: string;
  result: unknown;
  error: string | null;
}

export interface TeamAction {
  action_id: string;
  session_id: string;
  actor_id: string;
  run_id?: string | null;
  kind: string;
  payload?: Record<string, unknown>;
}

export class CoreClient {
  private proc: ChildProcess;
  private nextId = 1;
  private pending = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();

  constructor(coreBin: string, dbPath = ":memory:") {
    this.proc = spawn(coreBin, [dbPath], { stdio: ["pipe", "pipe", "inherit"] });
    const rl = readline.createInterface({ input: this.proc.stdout! });
    rl.on("line", (line) => {
      if (!line.trim()) return;
      const msg = JSON.parse(line);
      const slot = this.pending.get(msg.id);
      if (!slot) return;
      this.pending.delete(msg.id);
      if (msg.error) slot.reject(new Error(msg.error));
      else slot.resolve(msg.result);
    });
  }

  call<T = any>(method: string, params: unknown = {}): Promise<T> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.proc.stdin!.write(JSON.stringify({ id, method, params }) + "\n");
    });
  }

  createSession(sessionId: string, cwd: string, permissionsMode = "approved_scope") {
    return this.call("create_session", {
      session_id: sessionId,
      cwd,
      permissions_mode: permissionsMode,
    });
  }

  submit(action: TeamAction): Promise<Receipt> {
    return this.call("submit", action);
  }

  close() {
    this.proc.kill();
  }
}
