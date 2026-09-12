/**
 * TeamAgents terminal UI — zero-dependency ANSI port of the Textual TUI.
 * Layout: centered title row, tab row, divider, panel body, chat log, composer.
 * Keys mirror tui/app.py: Ctrl+Q quit, Ctrl+P pause, Ctrl+R refresh,
 * Ctrl+F full-auto, Ctrl+T cycle panel, Ctrl+G approvals, Esc stop Leader,
 * ↑/↓ composer history (persisted), a/s/d decide approvals, Enter selects.
 */
import readline from "node:readline";
import { existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { openSession, type OpenedSession } from "../session.ts";
import { sessionsDir } from "../config.ts";
import { archiveSession, deleteSession, listSessions, newSessionId } from "../sessions.ts";

const ESC = "\x1b";
const PANELS = ["team", "tasks", "approvals", "sessions", "log", "settings"] as const;
type Panel = (typeof PANELS)[number];

function historyFile() {
  const dir = sessionsDir();
  mkdirSync(dir, { recursive: true });
  return join(dir, "ui.json");
}

export function loadHistory(): string[] {
  try {
    return JSON.parse(readFileSync(historyFile(), "utf8")).history ?? [];
  } catch {
    return [];
  }
}

export function saveHistory(history: string[]) {
  // composer history cap: 500 entries, persisted across sessions
  writeFileSync(historyFile(), JSON.stringify({ history: history.slice(-500) }));
}

export class TuiApp {
  private panel: Panel = "team";
  private running = true;
  private chatLines: string[] = [];
  private cursor = 0; // event cursor
  private history = loadHistory();
  private histIdx = -1;
  private input = "";
  private cursorPos = 0;
  private selectedRow = 0;
  private state: any = null;
  private memberFilter: string | null = null;

  private opened: OpenedSession;
  private out: NodeJS.WritableStream;
  private input_stream: NodeJS.ReadableStream;
  constructor(opened: OpenedSession, out: NodeJS.WritableStream = process.stdout, input_stream: NodeJS.ReadableStream = process.stdin) {
    this.opened = opened;
    this.out = out;
    this.input_stream = input_stream;
  }

  static async run(opts: { cwd?: string; resume?: string; fullAuto?: boolean; team?: string; coreBin?: string }) {
    const opened = await openSession({
      cwd: opts.cwd,
      sessionId: opts.resume,
      fullAuto: opts.fullAuto,
      initialSpec: opts.team ? JSON.parse(readFileSync(opts.team, "utf8")) : undefined,
      coreBin: opts.coreBin,
    });
    const app = new TuiApp(opened);
    try {
      await opened.runtime.start();
      await app.main();
    } finally {
      await opened.close();
    }
  }

  async main() {
    const { out } = this;
    out.write(`${ESC}?1049h${ESC}?25l`); // alt screen, hide cursor
    readline.emitKeypressEvents(this.input_stream);
    if ((this.input_stream as any).isTTY) this.input_stream.setRawMode(true);
    this.input_stream.on("keypress", (ch, key) => this.onKey(ch, key ?? {}));
    const timer = setInterval(() => void this.refresh(), 300);
    timer.unref();
    await this.refresh();
    while (this.running) await new Promise((r) => setTimeout(r, 100));
    clearInterval(timer);
    out.write(`${ESC}?25h${ESC}?1049l`);
  }

  quit() {
    saveHistory(this.history);
    this.running = false;
  }

  // -- input -------------------------------------------------------------------

  private async onKey(ch: string | undefined, key: { name?: string; ctrl?: boolean }) {
    const name = key.name ?? ch ?? "";
    if (key.ctrl) {
      switch (name) {
        case "q":
          return this.quit();
        case "p":
          return this.pauseSession();
        case "r":
          return this.refresh();
        case "f":
          return this.toggleFullAuto();
        case "t":
          this.panel = PANELS[(PANELS.indexOf(this.panel) + 1) % PANELS.length];
          this.selectedRow = 0;
          return this.render();
        case "g":
          this.panel = "approvals";
          return this.render();
        case "a": // Ctrl+A: line start
          this.cursorPos = 0;
          return this.render();
        case "e": // Ctrl+E: line end
          this.cursorPos = this.input.length;
          return this.render();
      }
    }
    switch (name) {
      case "escape":
        return this.interruptLeader();
      case "return":
        return this.submitInput();
      case "backspace":
        if (this.cursorPos > 0) {
          this.input = this.input.slice(0, this.cursorPos - 1) + this.input.slice(this.cursorPos);
          this.cursorPos--;
        }
        return this.render();
      case "up":
        if (this.history.length) {
          this.histIdx = this.histIdx < 0 ? this.history.length - 1 : Math.max(0, this.histIdx - 1);
          this.input = this.history[this.histIdx];
          this.cursorPos = this.input.length;
        }
        return this.render();
      case "down":
        if (this.histIdx >= 0) {
          this.histIdx++;
          if (this.histIdx >= this.history.length) {
            this.histIdx = -1;
            this.input = "";
          } else {
            this.input = this.history[this.histIdx];
          }
          this.cursorPos = this.input.length;
        }
        return this.render();
      case "left":
        this.cursorPos = Math.max(0, this.cursorPos - 1);
        return this.render();
      case "right":
        this.cursorPos = Math.min(this.input.length, this.cursorPos + 1);
        return this.render();
    }
    if (this.panel === "approvals" && this.state?.pending_approvals?.length) {
      const approval = this.state.pending_approvals[this.selectedRow];
      if (approval && ["a", "s", "d"].includes(ch ?? "")) {
        const decision = { a: "once", s: "session", d: "deny" }[ch!]!;
        await this.opened.runtime.submit({
          action_id: `ui-appr-${Date.now()}`,
          session_id: this.opened.sessionId,
          actor_id: "user",
          kind: "approval_decision",
          payload: { approval_id: approval.approval_id, decision },
        });
        return this.refresh();
      }
    }
    if (this.panel === "sessions") {
      const rows = listSessions({ includeArchived: true });
      const row = rows[this.selectedRow];
      if (ch === "n") {
        await this.switchSession(newSessionId(this.opened.runtime.sessionId && process.cwd()));
        return;
      }
      if (row && name === "return") return this.switchSession(row.sessionId);
      if (row && ch === "a") {
        archiveSession(row.sessionId);
        return this.refresh();
      }
      if (row && ch === "d") {
        deleteSession(row.sessionId);
        return this.refresh();
      }
    }
    if (name === "tab") {
      this.panel = PANELS[(PANELS.indexOf(this.panel) + 1) % PANELS.length];
      this.selectedRow = 0;
      return this.render();
    }
    if (name === "pagedown" || (key.ctrl && name === "d")) return; // reserved
    if (ch && ch >= " " && !key.ctrl) {
      this.input = this.input.slice(0, this.cursorPos) + ch + this.input.slice(this.cursorPos);
      this.cursorPos++;
      return this.render();
    }
  }

  // -- actions -----------------------------------------------------------------

  private async submitInput() {
    const text = this.input.trim();
    if (!text) return;
    this.history.push(text);
    this.histIdx = -1;
    this.input = "";
    this.cursorPos = 0;
    await this.opened.runtime.userMessage(text);
    await this.refresh();
  }

  private async pauseSession() {
    const session = this.state?.session;
    if (session?.status === "PAUSED") {
      await this.opened.runtime.userMessage("继续执行");
      return;
    }
    const receipt = await this.opened.runtime.submit({
      action_id: `ui-pause-${this.cursor}`,
      session_id: this.opened.sessionId,
      actor_id: "user",
      kind: "pause_session",
      payload: {},
    });
    if (receipt.ok) this.chatLines.push("[system] 会话已暂停（输入新消息即恢复）");
    await this.refresh();
  }

  private async toggleFullAuto() {
    const mode = this.state?.session?.permissions_mode !== "full_auto" ? "full_auto" : "approved_scope";
    const receipt = await this.opened.runtime.submit({
      action_id: `ui-mode-${mode}-${this.cursor}`,
      session_id: this.opened.sessionId,
      actor_id: "user",
      kind: "set_permission_mode",
      payload: { mode },
    });
    if (receipt.ok) this.chatLines.push(`[system] 权限模式切换为 ${mode}`);
    await this.refresh();
  }

  private async interruptLeader() {
    const leaderId = this.state?.leader_id;
    const run = this.state?.runs?.find((r: any) => r.agent_id === leaderId && ["QUEUED", "RUNNING", "WAITING_TASK", "WAITING_APPROVAL"].includes(r.status));
    if (!run) return;
    const receipt = await this.opened.runtime.submit({
      action_id: `ui-stop-${this.cursor}`,
      session_id: this.opened.sessionId,
      actor_id: "user",
      kind: "cancel_run",
      payload: { run_id: run.run_id },
    });
    this.chatLines.push(receipt.ok ? "[system] 已请求停止 Leader，等待执行结束" : `[system] 停止失败：${receipt.error}`);
    await this.refresh();
  }

  private async switchSession(_sessionId: string) {
    // session switch re-opens; keep it simple: note and ask for CLI resume
    this.chatLines.push(`[system] 切换会话：退出后用 teamagents --resume ${_sessionId}`);
    await this.render();
  }

  // -- rendering ---------------------------------------------------------------

  async refresh() {
    const core = this.opened.core;
    this.state = await core.call("state", { session_id: this.opened.sessionId, after_sequence: this.cursor });
    for (const event of this.state.events) {
      this.cursor = event.sequence;
      this.chatLines.push(...this.formatEvent(event));
    }
    this.render();
  }

  private formatEvent(e: any): string[] {
    const p = e.payload ?? {};
    const one = (s: string) => [`  ${s}`];
    switch (e.kind) {
      case "user_message":
        return [`\x1b[36myou>\x1b[0m ${p.text}`];
      case "leader_reply":
        return [`\x1b[32m[Leader]\x1b[0m ${p.text}`];
      case "message":
        return one(`[message] ${e.actor_id} -> ${p.target}: ${String(p.text ?? "").slice(0, 160)}`);
      case "task_created":
        return one(`[task+] ${String(p.task_id).slice(0, 18)}… -> ${p.assignee}: ${String(p.description).slice(0, 80)}`);
      case "task_completed":
        return one(`[task✓] ${p.task_id} ${p.summary ?? ""}`);
      case "task_failed":
      case "task_blocked":
        return one(`[${e.kind}] ${p.task_id} ${p.error ?? p.reason ?? ""}`);
      case "run_failed":
        return one(`[run failed] ${e.actor_id}: ${String(p.error ?? "").slice(0, 200)}`);
      case "run_waiting":
        return one(`[waiting] ${p.agent_id} waits on ${(p.waiting_on ?? []).join(",")}`);
      case "approval_requested":
        return one(`[approval?] ${p.agent_id}: ${JSON.stringify(p.scope).slice(0, 160)}`);
      case "approval_decided":
        return one(`[approval=${p.status}] ${p.approval_id}`);
      case "goal_done":
        return one(`[goal done] ${p.summary ?? ""}`);
      case "limit_reached":
        return one(`[limit] ${p.kind}`);
      case "session_status":
        return one(`[session] ${JSON.stringify(p)}`);
      default:
        return [];
    }
  }

  render() {
    if (!this.state) return;
    const { out } = this;
    const cols = (out as any).columns ?? 100;
    const rows = (out as any).rows ?? 30;
    const lines: string[] = [];
    // row 1: centered status (height 1, no padding)
    const status = `TeamAgents  ${this.opened.sessionId}  ${this.state.session?.status ?? "?"}  ${this.state.session?.permissions_mode ?? ""}`;
    const pad = Math.max(0, Math.floor((cols - stripLen(status)) / 2));
    lines.push(" ".repeat(pad) + status);
    // row 2: tabs (padding-top 1, flush to divider)
    lines.push("");
    const tabs = PANELS.map((p) => (p === this.panel ? `\x1b[7m ${p} \x1b[0m` : ` ${p} `)).join("");
    lines.push(tabs);
    lines.push("─".repeat(cols));
    // body: active panel summary
    lines.push(...this.panelBody(cols, Math.max(3, Math.floor(rows / 3))));
    lines.push("─".repeat(cols));
    // chat log tail
    const chatRoom = Math.max(3, rows - lines.length - 3);
    lines.push(...this.chatLines.slice(-chatRoom));
    // composer
    lines.push("─".repeat(cols));
    const prompt = `you> ${this.input.slice(0, this.cursorPos)}\x1b[7m \x1b[0m${this.input.slice(this.cursorPos)}`;
    lines.push(prompt);
    out.write(`${ESC}H${ESC}J` + lines.slice(0, rows).join("\n"));
  }

  private panelBody(cols: number, maxRows: number): string[] {
    const s = this.state;
    const rows: string[] = [];
    switch (this.panel) {
      case "team": {
        rows.push("Members (Enter=筛选日志/取消):");
        for (const a of s.agents) {
          const specA = s.spec?.agents?.find((x: any) => x.id === a.id) ?? {};
          rows.push(`  ${a.id.padEnd(16)} ${String(a.status ?? "?").padEnd(9)} ${specA.role ?? ""} ${specA.runtime_kind ?? ""}`);
        }
        break;
      }
      case "tasks": {
        rows.push("Tasks:");
        for (const t of s.tasks) rows.push(`  ${String(t.task_id).padEnd(20)} ${t.status.padEnd(9)} ${t.assignee.padEnd(12)} ${t.description.slice(0, 60)}`);
        break;
      }
      case "approvals": {
        rows.push("Pending approvals (a=once s=session d=deny):");
        for (const a of s.pending_approvals) rows.push(`  ${a.approval_id.slice(0, 20)} ${a.agent_id} ${JSON.stringify(a.requested_scope).slice(0, 80)}`);
        if (!s.pending_approvals.length) rows.push("  (none)");
        break;
      }
      case "sessions": {
        rows.push("Sessions (Enter=切换提示 n=新建 a=归档 d=删除):");
        for (const r of listSessions({ includeArchived: true }).slice(0, maxRows - 1))
          rows.push(`  ${r.sessionId.padEnd(24)} ${r.status.padEnd(7)} ${r.goalState.padEnd(7)} ${r.events} ev  ${r.cwd}${r.archived ? " [归档]" : ""}`);
        break;
      }
      case "log": {
        rows.push("Events (tail):");
        for (const e of this.state.events.slice(-(maxRows - 1))) rows.push(`  #${e.sequence} ${e.kind} ${e.actor_id}`);
        break;
      }
      case "settings": {
        rows.push(`  session: ${this.opened.sessionId}`);
        rows.push(`  mode: ${s.session?.permissions_mode} (Ctrl+F 切换)`);
        rows.push(`  status: ${s.session?.status} (Ctrl+P 暂停/继续)`);
        rows.push(`  history: ${this.history.length} entries (persisted)`);
        break;
      }
    }
    return rows.slice(0, maxRows);
  }
}

function stripLen(s: string): number {
  return s.replace(/\x1b\[[0-9;]*m/g, "").length;
}

export async function runTuiApp(opts: { cwd?: string; resume?: string; fullAuto?: boolean; team?: string; coreBin?: string }) {
  await TuiApp.run(opts);
}
