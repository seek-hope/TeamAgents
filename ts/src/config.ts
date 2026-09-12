/**
 * User config loading, ported from config.py. TOML subset parser covering
 * sections, dotted keys, strings, ints/floats/bools, arrays, inline tables
 * (enough for ~/.config/teamagents/config.toml).
 */
import { readFileSync, existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const APP = "teamagents";

export function xdgConfigHome(): string {
  return process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config");
}
export function xdgStateHome(): string {
  return process.env.XDG_STATE_HOME ?? join(homedir(), ".local", "state");
}
export function userConfigPath(): string {
  return join(xdgConfigHome(), APP, "config.toml");
}
export function sessionsDir(): string {
  return join(xdgStateHome(), APP, "sessions");
}

/** Minimal TOML reader: [table]/[a.b], key = "s"|123|true|[..]|{k=v,..}. */
export function parseToml(text: string): Record<string, any> {
  const root: Record<string, any> = {};
  let table: string[] = [];
  const setAt = (path: string[], key: string, value: any) => {
    let node = root;
    for (const part of [...table, ...path]) node = node[part] ??= {};
    node[key] = value;
  };
  const parseValue = (raw: string): any => {
    raw = raw.trim();
    if (raw.startsWith('"') && raw.endsWith('"')) return raw.slice(1, -1).replace(/\\"/g, '"').replace(/\\\\/g, "\\");
    if (raw.startsWith("'") && raw.endsWith("'")) return raw.slice(1, -1);
    if (raw === "true" || raw === "false") return raw === "true";
    if (raw.startsWith("[") && raw.endsWith("]")) return splitTop(raw.slice(1, -1)).filter(Boolean).map(parseValue);
    if (raw.startsWith("{") && raw.endsWith("}")) {
      const obj: Record<string, any> = {};
      for (const part of splitTop(raw.slice(1, -1))) {
        if (!part) continue;
        const eq = part.indexOf("=");
        obj[part.slice(0, eq).trim()] = parseValue(part.slice(eq + 1));
      }
      return obj;
    }
    const n = Number(raw);
    if (!Number.isNaN(n)) return n;
    throw new Error(`unsupported TOML value: ${raw}`);
  };
  const splitTop = (s: string): string[] => {
    const out: string[] = [];
    let depth = 0, cur = "", inStr: string | null = null;
    for (const ch of s) {
      if (inStr) {
        cur += ch;
        if (ch === inStr) inStr = null;
        continue;
      }
      if (ch === '"' || ch === "'") inStr = ch;
      else if (ch === "[" || ch === "{") depth++;
      else if (ch === "]" || ch === "}") depth--;
      if (ch === "," && depth === 0) {
        out.push(cur.trim());
        cur = "";
      } else cur += ch;
    }
    if (cur.trim()) out.push(cur.trim());
    return out;
  };
  for (const rawLine of text.split("\n")) {
    let line = rawLine;
    // strip comments outside strings
    let inStr: string | null = null, cut = line.length;
    for (let i = 0; i < line.length; i++) {
      const ch = line[i];
      if (inStr) {
        if (ch === inStr && line[i - 1] !== "\\") inStr = null;
      } else if (ch === '"' || ch === "'") inStr = ch;
      else if (ch === "#") {
        cut = i;
        break;
      }
    }
    line = line.slice(0, cut).trim();
    if (!line) continue;
    if (line.startsWith("[") && line.endsWith("]")) {
      table = line.slice(1, -1).trim().split(".");
      continue;
    }
    const eq = line.indexOf("=");
    if (eq < 0) throw new Error(`bad TOML line: ${line}`);
    const key = line.slice(0, eq).trim();
    setAt(key.split(".").slice(0, -1), key.split(".").pop()!, parseValue(line.slice(eq + 1)));
  }
  return root;
}

export interface ModelProfileT {
  provider: string;
  protocol: "openai" | "anthropic" | "deepseek";
  model: string;
  base_url?: string;
  api_key_env?: string;
  timeout: number;
  max_retries: number;
  generation_options: Record<string, any>;
}

export interface UserConfigT {
  models: Record<string, ModelProfileT>;
  tools: Record<string, Record<string, any>>;
  skills_paths: string[];
  instruction_files: string[];
}

export function loadUserConfig(path = userConfigPath()): UserConfigT {
  if (!existsSync(path)) return { models: {}, tools: {}, skills_paths: [], instruction_files: [] };
  const data = parseToml(readFileSync(path, "utf8"));
  const models: Record<string, ModelProfileT> = {};
  for (const [name, m] of Object.entries(data.models ?? {}) as any) {
    models[name] = {
      provider: m.provider ?? "openai",
      protocol: m.protocol ?? "openai",
      model: m.model,
      base_url: m.base_url,
      api_key_env: m.api_key_env,
      timeout: m.timeout ?? 120,
      max_retries: m.max_retries ?? 5,
      generation_options: m.generation_options ?? {},
    };
  }
  return {
    models,
    tools: data.tools ?? {},
    skills_paths: data.skills_paths ?? [],
    instruction_files: data.instruction_files ?? [],
  };
}
