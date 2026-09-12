/** Session inventory/lock housekeeping, ported from sessions.py + session.py paths. */
import { DatabaseSync } from "node:sqlite";
import { createHash } from "node:crypto";
import { constants, existsSync, mkdirSync, openSync, readFileSync, rmSync, renameSync, writeFileSync, closeSync, unlinkSync } from "node:fs";
import { readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { sessionsDir } from "./config.ts";

export class SessionInUse extends Error {}
export class SessionDeleteBlocked extends Error {}

export function defaultSessionId(cwd?: string): string {
  const base = resolve(cwd ?? process.cwd());
  return "proj_" + createHash("sha256").update(base).digest("hex").slice(0, 12);
}

export function sessionPaths(sessionId: string) {
  const base = join(sessionsDir(), sessionId);
  return { base, db: join(base, "team.db"), artifacts: join(base, "artifacts"), locks: join(base, "locks"), lock: join(base, "session.lock") };
}

/** pid-based lock file (Python uses flock; node has no flock binding). */
export function acquireSessionLock(sessionId: string): () => void {
  const paths = sessionPaths(sessionId);
  mkdirSync(paths.base, { recursive: true });
  try {
    const fd = openSync(paths.lock, constants.O_CREAT | constants.O_EXCL | constants.O_WRONLY, 0o600);
    writeFileSync(fd, String(process.pid));
    closeSync(fd);
  } catch {
    // stale lock? if the recorded pid is gone, reclaim
    try {
      const pid = Number(readFileSync(paths.lock, "utf8").trim());
      process.kill(pid, 0);
      throw new SessionInUse(`session ${sessionId} is already running (pid ${pid})`);
    } catch (e) {
      if (e instanceof SessionInUse) throw e;
      unlinkSync(paths.lock);
      return acquireSessionLock(sessionId);
    }
  }
  return () => {
    try {
      unlinkSync(paths.lock);
    } catch {}
  };
}

export function isSessionLocked(sessionId: string, base?: string): boolean {
  const lock = join(base ?? sessionsDir(), sessionId, "session.lock");
  if (!existsSync(lock)) return false;
  try {
    const pid = Number(readFileSync(lock, "utf8").trim());
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export interface SessionInfo {
  sessionId: string;
  path: string;
  cwd: string;
  status: string;
  goalState: string;
  permissionsMode: string;
  updatedAt: number;
  events: number;
  tasks: number;
  sizeMb: number;
  archived: boolean;
  locked: boolean;
  error?: string;
}

function readMeta(path: string): Record<string, any> {
  const db = join(path, "team.db");
  if (!existsSync(db)) return {};
  try {
    const conn = new DatabaseSync(db, { readOnly: true });
    const row = conn
      .prepare("SELECT status, cwd, permissions_mode, goal_state, updated_at FROM sessions ORDER BY updated_at DESC LIMIT 1")
      .get() as any;
    const events = conn.prepare("SELECT COUNT(*) c FROM events").get() as any;
    const tasks = conn.prepare("SELECT COUNT(*) c FROM tasks").get() as any;
    conn.close();
    return { ...(row ?? {}), events: events?.c ?? 0, tasks: tasks?.c ?? 0 };
  } catch (e) {
    return { error: String(e) };
  }
}

function dirSizeMb(path: string): number {
  let total = 0;
  const walk = (dir: string) => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      const st = statSync(p);
      if (st.isDirectory()) walk(p);
      else total += st.size;
    }
  };
  try {
    walk(path);
  } catch {}
  return Math.round(total / 1e5) / 10;
}

export function listSessions(opts: { cwd?: string; includeArchived?: boolean; base?: string } = {}): SessionInfo[] {
  const root = opts.base ?? sessionsDir();
  const wanted = opts.cwd ? resolve(opts.cwd) : null;
  const groups: [string, boolean][] = [[root, false]];
  if (opts.includeArchived !== false) groups.push([join(root, "archived"), true]);
  const infos: SessionInfo[] = [];
  for (const [groupRoot, archived] of groups) {
    if (!existsSync(groupRoot)) continue;
    for (const name of readdirSync(groupRoot).sort()) {
      const path = join(groupRoot, name);
      try {
        if (!statSync(path).isDirectory() || !existsSync(join(path, "team.db"))) continue;
      } catch {
        continue;
      }
      const meta = readMeta(path);
      const info: SessionInfo = {
        sessionId: name,
        path,
        archived,
        cwd: String(meta.cwd ?? ""),
        status: String(meta.status ?? "?"),
        goalState: String(meta.goal_state ?? "?"),
        permissionsMode: String(meta.permissions_mode ?? "?"),
        updatedAt: Number(meta.updated_at ?? 0),
        events: Number(meta.events ?? 0),
        tasks: Number(meta.tasks ?? 0),
        sizeMb: dirSizeMb(path),
        locked: isSessionLocked(name, groupRoot),
        error: meta.error,
      };
      if (!archived && wanted && resolve(info.cwd || "/") !== wanted) continue;
      infos.push(info);
    }
  }
  infos.sort((a, b) => Number(a.archived) - Number(b.archived) || b.updatedAt - a.updatedAt);
  return infos;
}

export function newSessionId(cwd: string): string {
  const root = sessionsDir();
  const existing = new Set<string>();
  for (const group of [root, join(root, "archived")]) {
    if (existsSync(group)) for (const n of readdirSync(group)) existing.add(n);
  }
  const base = defaultSessionId(cwd);
  if (!existing.has(base)) return base;
  let index = 2;
  while (existing.has(`${base}_${index}`)) index++;
  return `${base}_${index}`;
}

export function archiveSession(sessionId: string, base?: string): string {
  const root = base ?? sessionsDir();
  if (isSessionLocked(sessionId, root)) throw new SessionInUse(`session ${sessionId} is running`);
  const targetDir = join(root, "archived");
  mkdirSync(targetDir, { recursive: true });
  const target = join(targetDir, sessionId);
  if (existsSync(target)) rmSync(target, { recursive: true });
  renameSync(join(root, sessionId), target);
  return target;
}

export function deleteSession(sessionId: string, base?: string): void {
  const root = base ?? sessionsDir();
  if (isSessionLocked(sessionId, root)) throw new SessionInUse(`session ${sessionId} is running`);
  rmSync(join(root, sessionId), { recursive: true, force: true });
}
