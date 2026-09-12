/**
 * Sandboxed tool executors, ported from execution.py/tools.py:
 * file tools confined to the member workdir; shell via bwrap when available;
 * web_search/web_fetch with SSRF guard (guard_url).
 */
import { execFile, spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import { isAbsolute, join, normalize, resolve } from "node:path";
import { lookup } from "node:dns/promises";
import { isIP } from "node:net";

const MAX_FILE_BYTES = 10 * 1024 * 1024;

/** Resolve `key` inside root; reject traversal and symlinks escaping root. */
export async function resolveInRoot(root: string, key: string): Promise<string> {
  const p = isAbsolute(key) ? normalize(key) : join(root, key);
  const resolvedRoot = await fs.realpath(root).catch(() => resolve(root));
  // lexical containment first (works for non-existent files)
  if (!p.startsWith(resolvedRoot + "/") && p !== resolvedRoot) throw new Error(`path escapes workspace: ${key}`);
  // symlink check on existing ancestors
  const real = await fs.realpath(p).catch(() => null);
  if (real && !real.startsWith(resolvedRoot + "/") && real !== resolvedRoot) throw new Error(`path escapes workspace: ${key}`);
  return p;
}

async function capRead(p: string): Promise<string> {
  const st = await fs.stat(p);
  if (st.size > MAX_FILE_BYTES) throw new Error(`file too large (${st.size} bytes)`);
  return fs.readFile(p, "utf8");
}

/** File/tool executor for one member's workspace. */
export function workspaceExecutor(root: string) {
  return async (tool: string, args: Record<string, any>): Promise<unknown> => {
    switch (tool) {
      case "ls": {
        const p = await resolveInRoot(root, String(args.path ?? "."));
        const entries = await fs.readdir(p, { withFileTypes: true });
        return entries.map((e) => (e.isDirectory() ? `${e.name}/` : e.name)).join("\n");
      }
      case "read_file": {
        const p = await resolveInRoot(root, String(args.path));
        return capRead(p);
      }
      case "write_file": {
        const p = await resolveInRoot(root, String(args.path));
        const content = String(args.content ?? "");
        if (Buffer.byteLength(content) > MAX_FILE_BYTES) throw new Error("content too large");
        await fs.mkdir(join(p, ".."), { recursive: true });
        await fs.writeFile(p, content);
        return `wrote ${p}`;
      }
      case "edit_file": {
        const p = await resolveInRoot(root, String(args.path));
        const text = await capRead(p);
        const [oldS, newS] = [String(args.old_string ?? ""), String(args.new_string ?? "")];
        if (!text.includes(oldS)) throw new Error("old_string not found");
        await fs.writeFile(p, text.replace(oldS, newS));
        return `edited ${p}`;
      }
      case "delete": {
        const p = await resolveInRoot(root, String(args.path));
        await fs.rm(p, { recursive: Boolean(args.recursive) });
        return `deleted ${p}`;
      }
      case "glob": {
        const { glob } = await import("node:fs/promises");
        const out: string[] = [];
        for await (const entry of glob(String(args.pattern ?? "*"), { cwd: root })) out.push(entry);
        return out.slice(0, 500).join("\n");
      }
      case "grep": {
        return shellRun(`grep -rn -- ${shellQuote(String(args.pattern))} ${shellQuote(String(args.path ?? "."))} | head -100`, root, 30);
      }
      case "shell": {
        return shellRun(String(args.command), root, Number(args.timeout ?? 120), Boolean(args.network));
      }
      default:
        throw new Error(`unknown tool ${tool}`);
    }
  };
}

function shellQuote(s: string): string {
  return `'${s.replaceAll("'", `'\\''`)}'`;
}

export async function bwrapAvailable(): Promise<boolean> {
  return new Promise((r) => execFile("which", ["bwrap"], (e) => r(!e)));
}

/** execution.py::run_isolated — bwrap when present, else bare with warning. */
export async function shellRun(command: string, workdir: string, timeoutS = 120, network = false): Promise<string> {
  const useBwrap = await bwrapAvailable();
  const argv = useBwrap
    ? [
        "bwrap",
        "--dev-bind", "/dev", "/dev",
        "--bind", workdir, workdir,
        "--ro-bind", "/usr", "/usr",
        "--ro-bind", "/lib", "/lib",
        "--ro-bind", "/bin", "/bin",
        "--proc", "/proc",
        "--chdir", workdir,
        ...(network ? [] : ["--unshare-net"]),
        "bash", "-lc", command,
      ]
    : ["bash", "-lc", command];
  return new Promise((resolveP, reject) => {
    const proc = spawn(argv[0], argv.slice(1), { cwd: workdir });
    let out = "";
    proc.stdout.on("data", (d) => (out += d));
    proc.stderr.on("data", (d) => (out += d));
    const timer = setTimeout(() => {
      proc.kill("SIGKILL");
      reject(new Error(`command timed out after ${timeoutS}s`));
    }, timeoutS * 1000);
    timer.unref();
    proc.on("close", (code) => {
      clearTimeout(timer);
      const trimmed = out.slice(0, 200_000);
      resolveP(code === 0 ? trimmed : `${trimmed}\n(exit ${code})`);
    });
    proc.on("error", reject);
  });
}

/** tools.py::guard_url — block private/loopback targets unless allowed. */
export async function guardUrl(url: string, allowPrivate = false): Promise<URL> {
  const u = new URL(url);
  if (!["http:", "https:"].includes(u.protocol)) throw new Error(`unsupported scheme ${u.protocol}`);
  if (allowPrivate) return u;
  const host = u.hostname;
  const addrs = isIP(host) ? [host] : (await lookup(host, { all: true })).map((a) => a.address);
  for (const addr of addrs) {
    if (isPrivateAddr(addr)) throw new Error(`refusing private address for ${host}`);
  }
  return u;
}

function isPrivateAddr(addr: string): boolean {
  if (isIP(addr) === 6) return addr === "::1" || addr.toLowerCase().startsWith("fc") || addr.toLowerCase().startsWith("fd");
  const parts = addr.split(".").map(Number);
  return (
    parts[0] === 10 ||
    parts[0] === 127 ||
    (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31) ||
    (parts[0] === 192 && parts[1] === 168) ||
    (parts[0] === 169 && parts[1] === 254) ||
    parts[0] === 0
  );
}

/** web_fetch tool: guarded GET, HTML→text. */
export async function webFetch(url: string, maxBytes = 2_000_000): Promise<string> {
  const u = await guardUrl(url);
  const res = await fetch(u, { signal: AbortSignal.timeout(30_000) });
  const buf = Buffer.from(await res.arrayBuffer()).subarray(0, maxBytes).toString("utf8");
  return buf.replace(/<script[\s\S]*?<\/script>/gi, "").replace(/<style[\s\S]*?<\/style>/gi, "").replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
}
