/**
 * TeamAgents TUI on Ink — visual parity with the Textual original
 * (Codex palette: #0d0d0d bg, #5d5d5d secondary, #3B82F6 accent, #00ff00 ok).
 */
import React, { useEffect, useReducer, useRef } from "react";
import { Box, Text, useApp, useInput, useStdout } from "ink";
import { existsSync, readFileSync } from "node:fs";
import { openSession, type OpenedSession } from "../session.ts";
import { loadHistory, saveHistory } from "./history.ts";
import { archiveSession, deleteSession, listSessions, newSessionId } from "../sessions.ts";

const h = React.createElement;

// palette (tui/theme.py)
const FG = "#ffffff";
const DIM = "#5d5d5d";
const DIM2 = "#afafaf";
const ACCENT = "#3B82F6";
const GREEN = "#00ff00";
const PANEL = "#181818";

const PANELS = ["team", "tasks", "shared", "approvals", "sessions", "log", "settings"] as const;
type Panel = (typeof PANELS)[number];
const PANEL_LABELS: Record<Panel, string> = {
  team: "Team", tasks: "Tasks", shared: "Shared", approvals: "Approvals",
  sessions: "Sessions", log: "Log", settings: "Settings",
};

interface ChatEntry {
  icon: string;
  label: string;
  labelColor: string;
  body: string;
  bodyColor?: string;
}

interface Model {
  state: any | null;
  chat: ChatEntry[];
  cursor: number;
}

export function formatEvent(e: any): ChatEntry[] {
  const p = e.payload ?? {};
  const sys = (body: string): ChatEntry[] => [{ icon: "•", label: "System", labelColor: DIM, body }];
  switch (e.kind) {
    case "user_message":
      return [{ icon: "›", label: "You", labelColor: ACCENT, body: p.text, bodyColor: FG }];
    case "leader_reply":
      return [{ icon: "●", label: "Leader", labelColor: GREEN, body: p.text ?? "" }];
    case "message":
      return sys(`[message] ${e.actor_id} → ${p.target}: ${String(p.text ?? "").slice(0, 160)}`);
    case "task_created":
      return sys(`Task ${String(p.task_id).slice(0, 20)} → ${p.assignee}: ${String(p.description).slice(0, 80)}`);
    case "task_completed":
      return [{ icon: "✓", label: "System", labelColor: GREEN, body: `${p.task_id} ${p.summary ?? ""}` }];
    case "task_failed":
    case "task_blocked":
    case "run_failed":
      return [{ icon: "⚠", label: "System", labelColor: "yellow", body: `${e.kind} ${p.task_id ?? ""} ${p.error ?? p.reason ?? ""}`, bodyColor: "red" }];
    case "run_waiting":
      return sys(`${p.agent_id} 等待 ${(p.waiting_on ?? []).join(", ")}`);
    case "approval_requested":
      return [{ icon: "⚠", label: "Approval", labelColor: "yellow", body: `${p.agent_id}: ${JSON.stringify(p.scope).slice(0, 160)}` }];
    case "approval_decided":
      return sys(`approval ${p.status} (${p.approval_id})`);
    case "goal_done":
      return [{ icon: "✓", label: "Goal", labelColor: GREEN, body: p.summary ?? "" }];
    case "limit_reached":
      return [{ icon: "⚠", label: "Limit", labelColor: "yellow", body: p.kind }];
    case "session_status":
      return sys(JSON.stringify(p));
    default:
      return [];
  }
}

export async function runInkTui(opts: { cwd?: string; resume?: string; fullAuto?: boolean; team?: string; coreBin?: string }) {
  if (!process.stdout.isTTY) {
    console.error("TUI 需要真实终端；哑终端请用 --plain");
    process.exitCode = 1;
    return;
  }
  const opened = await openSession({
    cwd: opts.cwd,
    sessionId: opts.resume,
    fullAuto: opts.fullAuto,
    initialSpec: opts.team ? JSON.parse(readFileSync(opts.team, "utf8")) : undefined,
    coreBin: opts.coreBin,
  });
  const { render } = await import("ink");
  const app = render(h(App, { opened }), { stdout: process.stdout, exitOnCtrlC: false });
  await opened.runtime.start();
  await app.waitUntilExit();
  await opened.close();
}

export const AppForTest = App;

function App({ opened }: { opened: OpenedSession }) {
  const { stdout } = useStdout();
  const termRows = stdout.rows ?? 30;
  const { exit } = useApp();
  const [, force] = useReducer((x) => x + 1, 0);
  const model = useRef<Model>({ state: null, chat: [], cursor: 0 });
  const panel = useRef<Panel>("team");
  const sel = useRef(0);
  const input = useRef("");
  const pos = useRef(0);
  const hist = useRef<string[]>(loadHistory());
  const histIdx = useRef(-1);

  const refresh = async () => {
    const m = model.current;
    const st = await opened.core.call("state", { session_id: opened.sessionId, after_sequence: m.cursor });
    m.state = st;
    for (const e of st.events) {
      m.cursor = e.sequence;
      m.chat.push(...formatEvent(e));
    }
    force();
  };
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;

  useEffect(() => {
    void refreshRef.current();
    const t = setInterval(() => void refreshRef.current(), 400);
    return () => clearInterval(t);
  }, []);

  const submit = async (kind: string, payload: Record<string, unknown>, idPrefix: string) => {
    const receipt = await opened.runtime.submit({
      action_id: `${idPrefix}-${model.current.cursor}-${Date.now()}`,
      session_id: opened.sessionId,
      actor_id: "user",
      kind,
      payload,
    });
    return receipt;
  };

  useInput((ch, key) => {
    const m = model.current;
    const P = panel.current;
    if (key.ctrl) {
      switch (ch) {
        case "q":
          saveHistory(hist.current);
          exit();
          return;
        case "p":
          if (m.state?.session?.status === "PAUSED") void opened.runtime.userMessage("继续执行");
          else void submit("pause_session", {}, "ui-pause");
          return;
        case "r":
          void refresh();
          return;
        case "f": {
          const mode = m.state?.session?.permissions_mode !== "full_auto" ? "full_auto" : "approved_scope";
          void submit("set_permission_mode", { mode }, "ui-mode");
          return;
        }
        case "t":
          panel.current = PANELS[(PANELS.indexOf(P) + 1) % PANELS.length];
          sel.current = 0;
          force();
          return;
        case "g":
          panel.current = "approvals";
          sel.current = 0;
          force();
          return;
        case "a":
          pos.current = 0;
          force();
          return;
        case "e":
          pos.current = input.current.length;
          force();
          return;
        case "j":
          insert("\n");
          return;
      }
    }
    if (key.escape) {
      const leaderId = m.state?.leader_id;
      const run = m.state?.runs?.find((r: any) => r.agent_id === leaderId && ["QUEUED", "RUNNING", "WAITING_TASK", "WAITING_APPROVAL"].includes(r.status));
      if (run) void submit("cancel_run", { run_id: run.run_id }, "ui-stop");
      return;
    }
    // panel-local keys
    if (key.upArrow) {
      if (P === "approvals" || P === "sessions" || P === "team") {
        sel.current = Math.max(0, sel.current - 1);
      } else if (hist.current.length) {
        histIdx.current = histIdx.current < 0 ? hist.current.length - 1 : Math.max(0, histIdx.current - 1);
        input.current = hist.current[histIdx.current];
        pos.current = input.current.length;
      }
      force();
      return;
    }
    if (key.downArrow) {
      if (P === "approvals" || P === "sessions" || P === "team") {
        sel.current = sel.current + 1;
      } else if (histIdx.current >= 0) {
        histIdx.current++;
        input.current = histIdx.current >= hist.current.length ? "" : hist.current[histIdx.current];
        if (histIdx.current >= hist.current.length) histIdx.current = -1;
        pos.current = input.current.length;
      }
      force();
      return;
    }
    if (key.leftArrow) {
      pos.current = Math.max(0, pos.current - 1);
      force();
      return;
    }
    if (key.rightArrow) {
      pos.current = Math.min(input.current.length, pos.current + 1);
      force();
      return;
    }
    if (key.return) {
      if (P === "approvals") return; // approvals decided by a/s/d
      const text = input.current.trim();
      if (text) {
        hist.current.push(text);
        histIdx.current = -1;
        input.current = "";
        pos.current = 0;
        void opened.runtime.userMessage(text).then(() => refresh());
      }
      return;
    }
    if (key.backspace || key.delete) {
      if (pos.current > 0) {
        input.current = input.current.slice(0, pos.current - 1) + input.current.slice(pos.current);
        pos.current--;
      }
      force();
      return;
    }
    if (P === "approvals" && ["a", "s", "d"].includes(ch ?? "")) {
      const list = m.state?.pending_approvals ?? [];
      const a = list[Math.min(sel.current, list.length - 1)];
      if (a) {
        const decision = { a: "once", s: "session", d: "deny" }[ch as "a" | "s" | "d"];
        void submit("approval_decision", { approval_id: a.approval_id, decision }, "ui-appr").then(() => refresh());
      }
      return;
    }
    if (P === "sessions" && ch) {
      const rows = listSessions({ includeArchived: true });
      const row = rows[Math.min(sel.current, Math.max(0, rows.length - 1))];
      if (ch === "a" && row && !row.locked) archiveSession(row.sessionId);
      if (ch === "d" && row && !row.locked) deleteSession(row.sessionId);
      if (ch === "n") newSessionId(process.cwd());
      force();
      return;
    }
    if (ch && ch >= " " && !key.ctrl && !key.meta) insert(ch);
  });

  const insert = (t: string) => {
    input.current = input.current.slice(0, pos.current) + t + input.current.slice(pos.current);
    pos.current += t.length;
    force();
  };

  const m = model.current;
  const session = m.state?.session;
  const statusLine = `TeamAgents · ${opened.sessionId}  |  ${session?.permissions_mode === "full_auto" ? "Full auto" : "Pre-authorized"}${session?.status === "PAUSED" ? " · PAUSED" : ""}`;

  return h(
    Box,
    { flexDirection: "column", height: termRows },
    // header: centered status (panel background)
    h(Box, { justifyContent: "center", backgroundColor: PANEL }, h(Text, { color: DIM2 }, statusLine)),
    h(Tabs, { active: panel.current }),
    h(PanelBody, { model: m, panel: panel.current, sel: sel.current }),
    h(Box, { borderStyle: "single", borderColor: DIM, flexGrow: 1, flexDirection: "column", paddingX: 1 },
      h(StatusLine, { model: m }),
      h(ChatView, { chat: m.chat, termRows })),
    h(Composer, { input: input.current, pos: pos.current }),
    h(Footer, null),
  );
}

function Tabs({ active }: { active: Panel }) {
  return h(
    Box,
    { gap: 2, paddingX: 1, marginTop: 1 },
    ...PANELS.map((p) =>
      h(Text, { key: p, bold: p === active, color: p === active ? FG : DIM, underline: p === active }, PANEL_LABELS[p]),
    ),
  );
}

function StatusLine({ model }: { model: Model }) {
  const s = model.state;
  const busy = s?.runs?.filter((r: any) => ["QUEUED", "RUNNING"].includes(r.status)) ?? [];
  const leader = s?.spec?.agents?.find((a: any) => a.id === s.leader_id);
  const text = busy.length
    ? `● Working · ${busy.map((r: any) => r.agent_id).join(", ")} executing`
    : `○ Ready · No turns executing  ·  Latest: ${model.chat.length ? "see log" : "Waiting for input"}`;
  const sub = s?.session?.status === "PAUSED" ? "‖ Paused" : `○ ${leader ? `${leader.name} / ${leader.model_profile}` : ""}`;
  return h(Box, { flexDirection: "column" }, h(Text, { color: DIM2 }, text), h(Text, { color: DIM }, sub));
}

function ChatView({ chat, termRows }: { chat: ChatEntry[]; termRows: number }) {
  const rows = termRows - 12;
  const shown = chat.slice(-Math.max(3, rows));
  return h(
    Box,
    { flexDirection: "column", flexGrow: 1, justifyContent: "flex-end" },
    ...shown.map((e, i) =>
      h(Box, { key: i, flexDirection: "column" },
        h(Text, { color: e.labelColor, bold: true }, `${e.icon} ${e.label}`),
        h(Text, { color: e.bodyColor ?? DIM2, wrap: "wrap" }, e.body)),
    ),
  );
}

function PanelBody({ model, panel, sel }: { model: Model; panel: Panel; sel: number }) {
  const s = model.state;
  if (!s) return h(Text, { color: DIM }, "loading…");
  const head = (cols: string[]) => h(Box, null, ...cols.map((c, i) => h(Text, { key: i, color: DIM, bold: true }, c.padEnd(colW[i]).slice(0, colW[i]))));
  let colW: number[] = [];
  const rowLine = (cells: string[], i: number) =>
    h(Box, { key: i, backgroundColor: i === sel ? PANEL : undefined },
      ...cells.map((c, j) => h(Text, { key: j, color: i === sel ? FG : DIM2 }, String(c).padEnd(colW[j]).slice(0, colW[j]))));
  let body: any = null;
  switch (panel) {
    case "team": {
      colW = [14, 10, 12, 16, 10, 12, 24];
      const rows = (s.spec?.agents ?? []).map((a: any) => {
        const rt = s.agents.find((x: any) => x.id === a.id);
        const canTask = (s.spec.channels ?? []).filter((c: any) => c.source === a.id && c.mode === "task").flatMap((c: any) => c.targets);
        return [a.id, a.role, a.runtime_kind, a.model_profile, rt?.status ?? "IDLE", a.workspace_policy ?? "shared", canTask.length ? `Tasks → ${canTask.join(",")}` : "-"];
      });
      body = [head(["Member", "Role", "Type", "Model", "Status", "Workspace", "Access"]), ...rows.map((r: string[], i: number) => rowLine(r, i)),
              h(Text, { key: "hint", color: DIM }, "\nHighlight a member to filter the log · Enter clears the filter")];
      break;
    }
    case "tasks": {
      colW = [22, 11, 14, 60];
      const rows = (s.tasks ?? []).map((t: any) => [t.task_id, t.status, t.assignee, t.description]);
      body = [head(["Task", "Status", "Assignee", "Description"]), ...rows.map((r: string[], i: number) => rowLine(r, i)),
              h(Text, { key: "hint", color: DIM }, rows.length ? "" : "(no tasks)")];
      break;
    }
    case "shared": {
      colW = [10, 12, 14, 70];
      const spaces = (s.spec?.shared_spaces ?? []);
      const rows = spaces.map((sp: any) => [sp.id, `${sp.readers.length}r`, `${sp.writers.length}w`, `readers=${sp.readers.join(",")} writers=${sp.writers.join(",")}`]);
      body = [head(["Space", "Readers", "Writers", "ACL"]), ...rows.map((r: string[], i: number) => rowLine(r, i))];
      break;
    }
    case "approvals": {
      colW = [22, 12, 70];
      const rows = (s.pending_approvals ?? []).map((a: any) => [a.approval_id, a.agent_id, JSON.stringify(a.requested_scope)]);
      body = [head(["Approval", "Agent", "Scope"]), ...rows.map((r: string[], i: number) => rowLine(r, i)),
              h(Text, { key: "hint", color: DIM }, rows.length ? "a=once · s=session · d=deny" : "(no pending approvals)")];
      break;
    }
    case "sessions": {
      colW = [26, 10, 10, 8, 6, 40];
      const rows = listSessions({ includeArchived: true }).slice(0, 12).map((r) => [
        r.sessionId, r.status, r.goalState, String(r.events), `${r.sizeMb}M`, `${r.cwd}${r.archived ? " [归档]" : ""}${r.locked ? " [运行中]" : ""}`,
      ]);
      body = [head(["Session", "Status", "Goal", "Events", "Size", "CWD"]), ...rows.map((r: string[], i: number) => rowLine(r, i)),
              h(Text, { key: "hint", color: DIM }, "a=archive · d=delete · 恢复: teamagents --resume <id>")];
      break;
    }
    case "log": {
      const evs = (model.state.events ?? []).slice(-14);
      colW = [8, 20, 14, 60];
      body = [head(["Seq", "Kind", "Actor", "Payload"]), ...evs.map((e: any, i: number) => rowLine([`#${e.sequence}`, e.kind, e.actor_id, JSON.stringify(e.payload).slice(0, 60)], i))];
      break;
    }
    case "settings": {
      body = [
        h(Text, { key: "s1", color: DIM2 }, `session: ${model.state.session?.session_id}`),
        h(Text, { key: "s2", color: DIM2 }, `mode: ${model.state.session?.permissions_mode}  (Ctrl+F 切换)`),
        h(Text, { key: "s3", color: DIM2 }, `status: ${model.state.session?.status}  (Ctrl+P 暂停/继续)`),
        h(Text, { key: "s4", color: DIM2 }, `history: ${loadHistory().length} entries (persisted)`),
      ];
      break;
    }
  }
  return h(Box, { flexDirection: "column", paddingX: 1, marginBottom: 1 }, ...body);
}

function Composer({ input, pos }: { input: string; pos: number }) {
  const before = input.slice(0, pos);
  const cursorChar = input[pos] ?? " ";
  const after = input.slice(pos + 1);
  return h(
    Box,
    { borderStyle: "round", borderColor: DIM, paddingX: 1, flexDirection: "column" },
    h(Text, null,
      h(Text, { color: ACCENT, bold: true }, "› "),
      h(Text, null, before),
      h(Text, { inverse: true }, cursorChar),
      h(Text, null, after)),
    h(Text, { color: DIM }, "Enter send · Ctrl+J newline · ↑↓ history · Esc stop Leader"),
  );
}

function Footer() {
  const keys: [string, string][] = [
    ["^j", "Newline"], ["^q", "Quit"], ["^p", "Pause/resume"], ["^r", "Refresh"],
    ["^f", "Full auto"], ["^t", "Panels"], ["^g", "Approvals"], ["esc", "Stop Leader"],
  ];
  return h(Box, { gap: 2, paddingX: 1, backgroundColor: PANEL },
    ...keys.map(([k, label]) => h(Text, { key: k }, h(Text, { color: FG, bold: true }, `${k} `), h(Text, { color: DIM }, label))));
}
