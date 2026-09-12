/** Composer history persistence (500 cap), shared by TUI variants. */
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { sessionsDir } from "../config.ts";

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
  writeFileSync(historyFile(), JSON.stringify({ history: history.slice(-500) }));
}
