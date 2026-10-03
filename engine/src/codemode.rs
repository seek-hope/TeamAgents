//! Codemode: one tool that runs a model-written JavaScript program whose only capabilities are the bound MCP
//! tools (D-374, learned from pi's `codemode`; completed by D-376).
//!
//! The point is the information-flow shape, not the scripting: a script may call many MCP tools, loop, filter
//! and chain them, and **only the script's own `text()` output and return value enter the model's context**.
//! Nested tool results stay in the VM. That is why MCP tools stop being advertised as individual tools once
//! this is on: the model composes them in code instead of paying for every intermediate payload.
//!
//! The engine is QuickJS (through `rquickjs`), the same one pi's codemode uses. Its only imports are the host
//! functions this module registers, so a script cannot fetch, read files, spawn processes or time out on its
//! own. A nested MCP call is synchronous on the host side, so `await tools.x(args)` resolves without an async
//! runtime; `tools.x` returns the tool's `structuredContent` when it declares an output schema and its text
//! otherwise, and rejects with an `Error` carrying the tool's error text (D-376).
//!
//! Two rules go beyond the sandbox and are the reason this file is not just a parser:
//!
//! * **A nested call obeys the user's `pre_tool` veto** (D-376). The direct path asks the hook before a tool
//!   runs; a script must not be a way around it, so the host bridge asks the same hook for every nested call.
//! * **The payload still does not enter the context.** `V2Codemode.tla` states that rule and its negative
//!   control refutes a build that pushed nested payloads.
//!
//! Ceiling (D-376): `image()` validates and accounts for an image item, but this build has no image flow into
//! the model context (`engine/src/providers/*` says so where the placeholder lives), so an image is reported
//! to the model as a short marker rather than a multimodal block. Remote image URLs are refused, as in pi.

use crate::bound::BoundTools;
use rquickjs::{Context, Function, Promise, Runtime, Value};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The name the model sees. One tool for every bound MCP tool.
pub const TOOL_NAME: &str = "codemode";

/// A runaway script must not grow the wasm32 heap without bound; QuickJS reports `out of memory` inside the
/// script when this is crossed (pi's codemode uses the same 256 MiB).
const MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;
/// The script's own output is what reaches the context, so it is capped even when the script prints more.
/// pi's default is 10,000 tokens; a token is estimated at four characters here.
const DEFAULT_MAX_OUTPUT_TOKENS: usize = 10_000;
const CHARS_PER_TOKEN: usize = 4;
/// pi's store limits: one value may be 256 Ki characters of JSON and all values together 1 Mi.
const MAX_STORE_VALUE_CHARS: usize = 256 * 1024;
const MAX_STORE_TOTAL_CHARS: usize = 1024 * 1024;
/// Default hard deadline for one script; a `// @options:` line may override it per call.
pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// The user's `pre_tool` veto, as the driver supplies it: `Some(reason)` denies the call.
pub type Veto = Arc<dyn Fn(&str, &Json) -> Option<String> + Send + Sync>;

/// A `// @options: {"timeout_ms": …}` first line, as pi's codemode source grammar allows.
#[derive(Debug, Default, PartialEq)]
struct Options {
    timeout_ms: Option<u64>,
    max_output_tokens: Option<usize>,
}

/// One nested call, as `result.calls` reports it (D-376).
#[derive(Debug, Clone, PartialEq)]
pub struct CallRecord {
    pub name: String,
    pub status: &'static str,
    pub error: Option<String>,
}

/// Why a script ended. `Completed` is success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Completed,
    Script,
    Timeout,
    Aborted,
    Sandbox,
}

/// Everything one `run` produced.
#[derive(Debug)]
pub struct Outcome {
    /// What the model sees: the script's `text()`/`console` output, its return value and any image markers.
    pub output: String,
    pub calls: Vec<CallRecord>,
    /// The model-visible error when the script failed.
    pub error: Option<String>,
    pub kind: Kind,
}

/// What the toolkit hands to `run`.
pub struct Codemode {
    pub tools: Arc<BoundTools>,
    pub store: Arc<Mutex<BTreeMap<String, Json>>>,
    /// The user's `pre_tool` veto, asked once per nested call (D-376).
    pub veto: Option<Veto>,
    pub timeout_ms: u64,
    /// True when the turn was already interrupted before the script started.
    pub aborted: bool,
}

/// `my-tool` -> `my_tool`, the identifier the model calls it by (pi's `toCodemodeIdentifier` rule).
fn identifier(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
            if index == 0 && ch.is_ascii_digit() {
                out.push('_');
            }
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// A compact TypeScript rendering of one MCP tool's argument schema. It is a *declaration* for the model, not a
/// validator: nested objects render as `object`, and unknown shapes as `unknown`. Depth is capped so a
/// recursive schema cannot produce an unbounded description.
fn ts_type(schema: &Json, depth: usize) -> String {
    if depth > 3 {
        return "unknown".into();
    }
    match schema.get("type").and_then(|t| t.as_str()) {
        Some("string") => "string".into(),
        Some("number") | Some("integer") => "number".into(),
        Some("boolean") => "boolean".into(),
        Some("array") => {
            format!("{}[]", schema.get("items").map(|i| ts_type(i, depth + 1)).unwrap_or_else(|| "unknown".into()))
        }
        Some("object") | None => {
            let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
                return "object".into();
            };
            let required: Vec<&str> = schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            let fields: Vec<String> = properties
                .iter()
                .map(|(name, sub)| {
                    let optional = if required.contains(&name.as_str()) { "" } else { "?" };
                    format!("{name}{optional}: {}", ts_type(sub, depth + 1))
                })
                .collect();
            format!("{{ {} }}", fields.join("; "))
        }
        Some(other) => other.into(),
    }
}

/// One tool as the script sees it: the normalized identifier, the real name, its description, its declaration
/// and the namespace (MCP service) it came from.
struct Meta {
    ident: String,
    real: String,
    description: String,
    declaration: String,
    namespace: String,
}

fn metas(tools: &BoundTools) -> Vec<Meta> {
    tools
        .tools
        .iter()
        .map(|tool| Meta {
            ident: identifier(&tool.name),
            real: tool.name.clone(),
            description: tool.description.clone(),
            declaration: format!(
                "{}(args: {}): Promise<unknown>",
                identifier(&tool.name),
                ts_type(&tool.parameters, 0)
            ),
            namespace: tool.namespace.clone(),
        })
        .collect()
}

/// The model-facing tool schema: the intro text plus a declaration of every bound MCP tool. Only called when at
/// least one tool is bound (the toolkit gates it), so the description never advertises an empty `tools`.
pub fn schema(tools: &BoundTools) -> Json {
    let meta = metas(tools);
    let mut declarations = String::new();
    for tool in &meta {
        let alias = if tool.ident == tool.real { String::new() } else { format!(" (bound as `{}`)", tool.real) };
        declarations.push_str(&format!(
            "\n  /** {}{} */\n  {};",
            tool.description.replace("*/", "* /").replace('\n', " "),
            alias,
            tool.declaration
        ));
    }
    let description = format!(
        "Run JavaScript that calls MCP tools as `await tools.<name>(args)`.\n\
         - The code is evaluated in a fresh QuickJS sandbox as the body of an async function: top-level `await` and `return` work.\n\
         - Tool names are normalized to JavaScript identifiers (`my-tool` is `tools.my_tool`); `ALL_TOOLS` lists every tool, `searchTools(query)` finds one by name or description, `describeTool(name)` returns its declaration and `describeNamespace(service)` lists one service's tools.\n\
         - A nested call that fails, is vetoed by the user's `pre_tool` hook, or gets invalid arguments rejects with an `Error` carrying the reason. Calls are real and have side effects; earlier calls are not undone when a later one fails.\n\
         - Only what the script emits with `text(...)`/`console.log(...)` and what it returns reaches you — nested tool output does NOT. Filter and aggregate in the script instead of returning raw payloads.\n\
         - No Node, no file system, no network, no timers; a 256 MB heap and a hard deadline apply.\n\
         - `store(key, value)`/`load(key)` persist JSON values across codemode calls in this session. `exit()` ends the script successfully. `image(...)` accepts a base64 `data:` URL.\n\
         - The first line may be `// @options: {{\"timeout_ms\": 60000, \"max_output_tokens\": 2000}}`.\n\n\
         Available tools:\ndeclare const tools: {{{declarations}\n}};"
    );
    json!({
        "name": TOOL_NAME,
        "description": description,
        "parameters": {
            "type": "object",
            "properties": {
                "code": {"type": "string", "description": "JavaScript source (the body of an async function), not JSON or a fenced block."}
            },
            "required": ["code"]
        }
    })
}

/// Split a `// @options:` first line from the script. The line is replaced by a blank line so stack-trace line
/// numbers still match the input.
fn parse_source(source: &str) -> Result<(String, Options), String> {
    let mut options = Options::default();
    let trimmed = source.trim_start();
    if let Some(rest) = trimmed.strip_prefix("// @options:") {
        let (line, tail) = match rest.find('\n') {
            Some(index) => (&rest[..index], &rest[index + 1..]),
            None => (rest, ""),
        };
        let parsed: Json = serde_json::from_str(line.trim())
            .map_err(|error| format!("codemode: the @options line is not JSON: {error}"))?;
        let object =
            parsed.as_object().ok_or_else(|| "codemode: the @options line must be a JSON object".to_string())?;
        for key in object.keys() {
            if key != "timeout_ms" && key != "max_output_tokens" {
                return Err(format!("codemode: unknown @options field {key:?}"));
            }
        }
        options.timeout_ms = object.get("timeout_ms").and_then(|v| v.as_u64());
        options.max_output_tokens = object.get("max_output_tokens").and_then(|v| v.as_u64()).map(|v| v as usize);
        return Ok((tail.to_string(), options));
    }
    Ok((source.to_string(), options))
}

/// The JS prelude. Everything a script can reach besides its own code is either a value here, a wrapper over
/// the host functions below, or a pure JavaScript helper.
fn prelude(meta: &[Meta]) -> String {
    let names: BTreeMap<&str, &str> = meta.iter().map(|tool| (tool.ident.as_str(), tool.real.as_str())).collect();
    let catalogue: Json = Json::Array(
        meta.iter()
            .map(|tool| {
                json!({"name": tool.ident, "description": tool.description,
                       "declaration": tool.declaration, "namespace": tool.namespace})
            })
            .collect(),
    );
    format!(
        r#"
globalThis.__names = {names};
globalThis.__catalogue = {catalogue};
globalThis.ALL_TOOLS = __catalogue.map((entry) => ({{ name: entry.name, description: entry.description }}));
{unknown_help}
globalThis.tools = new Proxy({{}}, {{
  get: (_target, property) => {{
    const key = String(property);
    const real = Object.prototype.hasOwnProperty.call(__names, key) ? __names[key] : key;
    // D-404: refuse an unknown name here, with the catalogue in hand, instead of letting the host answer
    if (!__catalogue.some((entry) => entry.name === real)) {{
      throw new Error(__unknownTool(key));
    }}
    return (args) => {{
      const reply = JSON.parse(__call(real, JSON.stringify(args === undefined ? {{}} : args)));
      if (!reply.ok) {{ throw new Error(reply.error); }}
      return reply.value;
    }};
  }},
}});
globalThis.__stringify = (value) => (typeof value === "string" ? value : JSON.stringify(value));
globalThis.text = (value) => __text(globalThis.__stringify(value));
globalThis.console = {{}};
for (const method of ["log", "info", "warn", "error", "debug"]) {{
  globalThis.console[method] = (...args) => __text(args.map(globalThis.__stringify).join(" "));
}}
globalThis.store = (key, value) => {{
  const problem = __store(String(key), value === undefined ? null : JSON.stringify(value));
  if (problem) {{ throw new Error(problem); }}
}};
globalThis.load = (key) => {{ const raw = __load(String(key)); return raw === null || raw === undefined ? undefined : JSON.parse(raw); }};
globalThis.exit = () => {{ throw {{ __codemode_exit: true }}; }};
globalThis.image = (item) => {{
  const problem = __image(JSON.stringify(item));
  if (problem) {{ throw new Error(problem); }}
}};
globalThis.__score = (query, entry) => {{
  const words = String(query).toLowerCase().split(/\s+/).filter((word) => word.length > 0);
  if (words.length === 0) {{ return 0; }}
  const name = entry.name.toLowerCase();
  const description = String(entry.description || "").toLowerCase();
  let score = 0;
  for (const word of words) {{
    if (name.includes(word)) {{ score += 3; }}
    if (description.includes(word)) {{ score += 1; }}
  }}
  return score;
}};
globalThis.searchTools = (query, options) => {{
  const limit = options && Number.isFinite(options.limit) ? options.limit : 8;
  const namespace = options && options.namespace;
  return __catalogue
    .filter((entry) => !namespace || entry.namespace === namespace)
    .map((entry) => ({{ entry, score: __score(query, entry) }}))
    .filter((candidate) => candidate.score > 0)
    .sort((a, b) => b.score - a.score || a.entry.name.localeCompare(b.entry.name))
    .slice(0, limit)
    .map((candidate) => ({{ name: candidate.entry.name, description: candidate.entry.description }}));
}};
globalThis.describeTool = (name) => {{
  const entry = __catalogue.find((candidate) => candidate.name === name);
  return entry ? {{ name: entry.name, description: entry.description, declaration: entry.declaration }} : undefined;
}};
globalThis.describeNamespace = (name) => {{
  const members = __catalogue.filter((entry) => entry.namespace === name);
  return members.length === 0 ? undefined : {{ name, tools: members.map((entry) => ({{ name: entry.name, description: entry.description }})) }};
}};
"#,
        names = serde_json::to_string(&names).unwrap_or_else(|_| "{}".into()),
        catalogue = serde_json::to_string(&catalogue).unwrap_or_else(|_| "[]".into()),
        unknown_help = UNKNOWN_TOOL_HELP,
    )
}

/// Format an already-caught exception. `Ctx::catch` consumes the pending exception, so the exit marker and the
/// error text must be read from the same value.
/// D-404: what a script gets when it names a tool that does not exist. `tools` is a Proxy, so the miss is caught
/// before the host call, where the catalogue is — and the answer names the close tools (case- and
/// separator-insensitive, so `tools.Bash` finds `bash`) or, failing that, points at `ALL_TOOLS` and
/// `searchTools`. pi's codemode does the same (`tools.Bash` suggests `tools.bash`), and the same principle just
/// removed a fifth of one measured turn's operations on the tool surface (D-402).
const UNKNOWN_TOOL_HELP: &str = r#"
globalThis.__unknownTool = (key) => {
  const flatten = (name) => String(name).toLowerCase().replace(/[^a-z0-9]/g, "");
  const wanted = flatten(key);
  const close = __catalogue
    .filter((entry) => entry.name !== key)
    .filter((entry) => {
      const flat = flatten(entry.name);
      return flat === wanted || flat.startsWith(wanted) || wanted.startsWith(flat);
    })
    .map((entry) => "tools." + entry.name);
  if (close.length > 0) {
    return "unknown tool " + key + " — did you mean " + close.slice(0, 3).join(" or ")
      + "? (`ALL_TOOLS` lists every tool, `searchTools(query)` finds one by name or description)";
  }
  const all = __catalogue.map((entry) => entry.name);
  const shown = all.slice(0, 20).join(", ") + (all.length > 20 ? ", …" : "");
  return "unknown tool " + key + " — this session offers: " + shown
    + " (`ALL_TOOLS` is the full list, `searchTools(query)` finds one by name or description)";
};
"#;

fn exception_of(value: Value<'_>) -> String {
    if let Some(exception) = value.as_exception() {
        let message = exception.message().unwrap_or_default();
        return match exception.stack() {
            Some(stack) => format!("{message}\n{}", stack.lines().take(2).collect::<Vec<_>>().join("\n")),
            None => message,
        };
    }
    format!("script failed: {value:?}")
}

/// `exit()` throws a marker object, which is a successful end, not an error.
fn is_exit(value: &Value<'_>) -> bool {
    value
        .as_object()
        .and_then(|object| object.get::<_, Option<bool>>("__codemode_exit").ok().flatten())
        .unwrap_or(false)
}

/// Validate an `image()` argument and return `(mime, approximate bytes)`. Remote URLs are refused (pi's rule):
/// a script must not make the host fetch a URL.
fn image_shape(item: &Json) -> Result<(String, usize), String> {
    let data = match item {
        Json::String(url) => url.as_str(),
        Json::Object(object) => {
            if let Some(url) = object.get("image_url").and_then(|v| v.as_str()) {
                url
            } else if object.get("type").and_then(|v| v.as_str()) == Some("image") {
                let mime = object.get("mimeType").and_then(|v| v.as_str()).unwrap_or("image/png").to_string();
                let data = object.get("data").and_then(|v| v.as_str()).unwrap_or("");
                return Ok((mime, data.len() * 3 / 4));
            } else {
                return Err("codemode: image() wants a data: URL or an MCP image block".into());
            }
        }
        _ => return Err("codemode: image() wants a data: URL or an MCP image block".into()),
    };
    let Some(rest) = data.strip_prefix("data:") else {
        return Err("codemode: image() refuses a remote URL; pass a base64 data: URL".into());
    };
    let mime = rest.split(';').next().unwrap_or("image/png").to_string();
    let bytes = rest.split(',').nth(1).map(|payload| payload.len() * 3 / 4).unwrap_or(0);
    Ok((mime, bytes))
}

/// Run one script. `tools` supplies the only capabilities; `store` is the cross-call JSON store; `veto` is the
/// user's `pre_tool` hook, asked once per nested call.
pub fn run(source: &str, context: Codemode) -> Outcome {
    let kind =
        |kind: Kind, error: String| Outcome { output: String::new(), calls: Vec::new(), error: Some(error), kind };
    if context.aborted {
        return kind(Kind::Aborted, "codemode: the turn was interrupted before the script ran".into());
    }
    if context.tools.tools.is_empty() {
        return kind(Kind::Script, "codemode is not available: no MCP tool is bound to this member".into());
    }
    let (body, options) = match parse_source(source) {
        Ok(parsed) => parsed,
        Err(error) => return kind(Kind::Script, error),
    };
    if body.trim().is_empty() {
        return kind(Kind::Script, "codemode: the script is empty".into());
    }
    let deadline = Instant::now() + Duration::from_millis(options.timeout_ms.unwrap_or(context.timeout_ms).max(1));
    let max_output_tokens = options.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
    let timed_out = Arc::new(AtomicBool::new(false));
    let runtime = match Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => return kind(Kind::Sandbox, format!("codemode: {error}")),
    };
    runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
    {
        let timed_out = timed_out.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            if Instant::now() >= deadline {
                timed_out.store(true, Ordering::SeqCst);
                return true;
            }
            false
        })));
    }
    let context_js = match Context::full(&runtime) {
        Ok(context) => context,
        Err(error) => return kind(Kind::Sandbox, format!("codemode: {error}")),
    };

    let meta = metas(&context.tools);
    let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let calls: Arc<Mutex<Vec<CallRecord>>> = Arc::new(Mutex::new(Vec::new()));

    let outcome: Result<(), String> = context_js.with(|ctx| {
        let prepared: Result<(), String> = (|| {
            let host = context.tools.clone();
            let veto = context.veto.clone();
            let call_log = calls.clone();
            let host_fn = Function::new(ctx.clone(), move |name: String, args: String| -> String {
                let parsed: Json = serde_json::from_str(&args).unwrap_or(Json::Null);
                if let Some(reason) = veto.as_ref().and_then(|veto| veto(&name, &parsed)) {
                    call_log
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(CallRecord { name, status: "denied", error: Some(reason.clone()) });
                    return json!({"ok": false, "error": format!("denied by the user's pre_tool hook: {reason}")})
                        .to_string();
                }
                let rendered = match host.call(&name, &parsed) {
                    Some(Ok(value)) => Ok(value),
                    Some(Err(error)) => Err(error),
                    None => Err(format!("unknown tool {name:?}")),
                };
                let mut log = call_log.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                match &rendered {
                    Ok(_) => log.push(CallRecord { name, status: "ok", error: None }),
                    Err(error) => log.push(CallRecord { name, status: "error", error: Some(error.clone()) }),
                }
                drop(log);
                match rendered {
                    Ok(value) => json!({"ok": true, "value": value}).to_string(),
                    Err(error) => json!({"ok": false, "error": error}).to_string(),
                }
            })
            .map_err(|error| error.to_string())?;
            let setter = {
                let store = context.store.clone();
                Function::new(ctx.clone(), move |key: String, value: Option<String>| -> Option<String> {
                    let mut store = store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    let parsed = match value {
                        None => {
                            store.remove(&key);
                            return None;
                        }
                        Some(raw) => match serde_json::from_str::<Json>(&raw) {
                            Ok(parsed) => parsed,
                            Err(_) => Json::Null,
                        },
                    };
                    let size = parsed.to_string().chars().count();
                    if size > MAX_STORE_VALUE_CHARS {
                        return Some(format!("codemode: store value for {key:?} is {size} characters; the limit is {MAX_STORE_VALUE_CHARS}"));
                    }
                    let existing = store.get(&key).map(|value| value.to_string().chars().count()).unwrap_or(0);
                    let total: usize =
                        store.values().map(|value| value.to_string().chars().count()).sum::<usize>() - existing + size;
                    if total > MAX_STORE_TOTAL_CHARS {
                        return Some(format!("codemode: the store would hold {total} characters; the limit is {MAX_STORE_TOTAL_CHARS}"));
                    }
                    store.insert(key, parsed);
                    None
                })
                .map_err(|error| error.to_string())?
            };
            let getter = {
                let store = context.store.clone();
                Function::new(ctx.clone(), move |key: String| -> Option<String> {
                    let store = store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    store.get(&key).map(|value| value.to_string())
                })
                .map_err(|error| error.to_string())?
            };
            let emitter = {
                let output = output.clone();
                Function::new(ctx.clone(), move |text: String| {
                    output.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(text);
                })
                .map_err(|error| error.to_string())?
            };
            let imagist = {
                let output = output.clone();
                Function::new(ctx.clone(), move |item: String| -> Option<String> {
                    let parsed: Json = serde_json::from_str(&item).unwrap_or(Json::Null);
                    match image_shape(&parsed) {
                        Ok((mime, bytes)) => {
                            output
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push(format!("[image: {mime}, ~{bytes} bytes — this build cannot put an image into the model's context]"));
                            None
                        }
                        Err(error) => Some(error),
                    }
                })
                .map_err(|error| error.to_string())?
            };
            ctx.globals().set("__call", host_fn).map_err(|error| error.to_string())?;
            ctx.globals().set("__store", setter).map_err(|error| error.to_string())?;
            ctx.globals().set("__load", getter).map_err(|error| error.to_string())?;
            ctx.globals().set("__text", emitter).map_err(|error| error.to_string())?;
            ctx.globals().set("__image", imagist).map_err(|error| error.to_string())?;
            ctx.eval::<(), _>(prelude(&meta)).map_err(|_| {
                let value: Value = ctx.catch();
                exception_of(value)
            })?;
            Ok(())
        })();
        prepared?;
        let wrapped = format!(
            "(async () => {{ const __value = await (async () => {{\n{body}\n}})(); if (__value !== undefined) {{ text(__value); }} return __value; }})()"
        );
        match ctx.eval::<Promise, _>(wrapped) {
            Ok(promise) => match promise.finish::<Value>() {
                Ok(_) => Ok(()),
                Err(_) => {
                    let value: Value = ctx.catch();
                    if is_exit(&value) {
                        Ok(())
                    } else {
                        Err(exception_of(value))
                    }
                }
            },
            Err(_) => {
                let value: Value = ctx.catch();
                Err(exception_of(value))
            }
        }
    });

    let calls = calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    match outcome {
        Ok(()) => {
            let raw = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).join("\n");
            let limit = max_output_tokens.saturating_mul(CHARS_PER_TOKEN);
            let (text, truncated) = if raw.chars().count() > limit {
                (raw.chars().take(limit).collect::<String>(), true)
            } else {
                (raw, false)
            };
            let mut text = if text.is_empty() { "(the script produced no output)".to_string() } else { text };
            if truncated {
                text.push_str("\n… (codemode output truncated at max_output_tokens)");
            }
            Outcome { output: text, calls, error: None, kind: Kind::Completed }
        }
        Err(error) => {
            let kind = if timed_out.load(Ordering::SeqCst) { Kind::Timeout } else { Kind::Script };
            let prefix = if kind == Kind::Timeout { "codemode: the script exceeded its deadline\n" } else { "" };
            Outcome { output: String::new(), calls, error: Some(format!("{prefix}{error}")), kind }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound::BoundTool;

    fn tool(name: &str, description: &str, parameters: Json) -> BoundTool {
        BoundTool {
            name: name.into(),
            description: description.into(),
            parameters,
            namespace: "svc".into(),
            output_schema: None,
            remote: None,
        }
    }

    fn fake_tools() -> Arc<BoundTools> {
        Arc::new(BoundTools {
            tools: vec![
                tool(
                    "echo",
                    "Echo the argument back",
                    json!({"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"]}),
                ),
                tool("big-list", "Return a list of records", json!({"type": "object", "properties": {}})),
            ],
        })
    }

    fn store() -> Arc<Mutex<BTreeMap<String, Json>>> {
        Arc::new(Mutex::new(BTreeMap::new()))
    }

    fn run_source(body: &str) -> Outcome {
        run(
            body,
            Codemode {
                tools: fake_tools(),
                store: store(),
                veto: None,
                timeout_ms: DEFAULT_TIMEOUT_MS,
                aborted: false,
            },
        )
    }

    #[test]
    fn a_script_emits_its_text_and_its_return_value() {
        let outcome = run_source(r#"text("hello " + (1 + 1)); return {ok: true};"#);
        assert_eq!(outcome.kind, Kind::Completed);
        assert!(outcome.output.contains("hello 2"), "{}", outcome.output);
        assert!(
            outcome.output.contains("\"ok\":true"),
            "the returned value is emitted like text(): {}",
            outcome.output
        );
    }

    /// D-404: a name that is not a tool is refused with the catalogue in hand — the close tools when there are
    /// any (`tools.Bash` → `tools.bash`, the separator-insensitive match pi does) and the offered list otherwise.
    /// The call never reaches the host, so it is not logged as a tool call: nothing was called.
    #[test]
    fn a_script_that_names_a_tool_that_does_not_exist_is_told_what_does() {
        let close = run_source(r#"return await tools.BigList({});"#);
        let error = close.error.expect("a script error");
        assert!(
            error.contains("did you mean tools.big_list") || error.contains("did you mean tools.big-list"),
            "{error}"
        );
        assert!(error.contains("searchTools"), "and how to look one up: {error}");
        assert!(close.calls.is_empty(), "nothing was called: {:?}", close.calls);

        let nowhere = run_source(r#"return await tools.no_such_tool_anywhere({});"#);
        let error = nowhere.error.expect("a script error");
        assert!(error.contains("unknown tool no_such_tool_anywhere"), "{error}");
        assert!(error.contains("echo"), "the offered tools are named: {error}");
        assert!(error.contains("ALL_TOOLS"), "{error}");
        assert!(nowhere.calls.is_empty(), "nothing was called: {:?}", nowhere.calls);
    }

    #[test]
    fn a_script_that_calls_an_unbound_tool_rejects_and_logs_the_call() {
        let outcome = run_source("return await tools.echo({ value: 'x' });");
        let error = outcome.error.expect("a script error");
        assert!(error.contains("unknown tool"), "a host call to a tool with no remote is refused: {error}");
        assert_eq!(outcome.calls.len(), 1);
        assert_eq!(outcome.calls[0].status, "error");
        assert!(outcome.calls[0].error.as_deref().unwrap_or_default().contains("unknown tool"), "{:?}", outcome.calls);
    }

    #[test]
    fn a_nested_call_obeys_the_users_pre_tool_veto() {
        let veto: Veto =
            Arc::new(|name: &str, _args: &Json| if name == "echo" { Some("policy says no".into()) } else { None });
        let outcome = run(
            "return await tools.echo({ value: 'x' });",
            Codemode {
                tools: fake_tools(),
                store: store(),
                veto: Some(veto),
                timeout_ms: DEFAULT_TIMEOUT_MS,
                aborted: false,
            },
        );
        let error = outcome.error.expect("the veto rejects the script");
        assert!(error.contains("pre_tool hook") && error.contains("policy says no"), "{error}");
        assert_eq!(outcome.calls[0].status, "denied");
    }

    #[test]
    fn a_script_cannot_reach_the_host_beyond_the_registered_functions() {
        for body in ["return typeof fetch;", "return typeof process;", "return typeof require;"] {
            let outcome = run_source(body);
            assert!(outcome.output.contains("undefined"), "no host capability is exposed ({body}): {}", outcome.output);
        }
    }

    #[test]
    fn store_and_load_survive_inside_one_store_exit_ends_cleanly_and_limits_are_enforced() {
        let tools = fake_tools();
        let store = store();
        let run_one = |body: &str| {
            run(
                body,
                Codemode {
                    tools: tools.clone(),
                    store: store.clone(),
                    veto: None,
                    timeout_ms: DEFAULT_TIMEOUT_MS,
                    aborted: false,
                },
            )
        };
        run_one("store('n', 41); text('stored');");
        assert!(run_one("text(String(load('n') + 1));").output.contains("42"));
        let exited = run_one("text('before'); exit(); text('after');");
        assert!(exited.output.contains("before") && !exited.output.contains("after"), "{}", exited.output);
        let too_large = run_one("store('big', 'x'.repeat(300000));");
        assert!(too_large.error.expect("over the limit").contains("limit"), "a value over the store limit is refused");
    }

    #[test]
    fn search_describe_and_namespace_expose_the_catalogue_to_scripts() {
        let found = run_source("const hits = await searchTools('records'); text(hits[0].name + ':' + hits.length);");
        assert!(found.output.contains("big_list:1"), "{}", found.output);
        let described =
            run_source("const tool = await describeTool('big_list'); text(String(tool.declaration.length > 0));");
        assert!(described.output.contains("true"), "{}", described.output);
        let namespace = run_source("const svc = await describeNamespace('svc'); text(svc.tools.length + ':' + String(await describeNamespace('nope')));");
        assert!(namespace.output.contains("2:undefined"), "{}", namespace.output);
        // the helpers are plain functions, so a script that forgets `await` still gets the value (a real-model
        // run wasted a turn on `[object Promise]` before this; `await` keeps working on a non-promise)
        let sync = run_source("const hits = searchTools('records'); const tool = describeTool('big_list'); text(hits[0].name + ':' + String(tool.declaration.length > 0));");
        assert!(sync.output.contains("big_list:true"), "{}", sync.output);
    }

    #[test]
    fn the_options_line_sets_the_deadline_and_the_output_budget_and_refuses_unknown_fields() {
        assert_eq!(
            parse_source("// @options: {\"timeout_ms\": 5000,\"max_output_tokens\": 10}\nreturn 1;").unwrap().1,
            Options { timeout_ms: Some(5000), max_output_tokens: Some(10) }
        );
        assert!(parse_source("// @options: {\"nope\": 1}\nreturn 1;").unwrap_err().contains("unknown @options"));
        let long = run_source("// @options: {\"max_output_tokens\": 5}\ntext('x'.repeat(1000));");
        assert!(long.output.contains("truncated at max_output_tokens"), "{}", long.output);
        let timed = run_source("// @options: {\"timeout_ms\": 200}\nwhile (true) {}");
        assert_eq!(timed.kind, Kind::Timeout, "{:?}", timed);
    }

    #[test]
    fn image_accepts_a_data_url_and_refuses_a_remote_url() {
        let good = run_source("image('data:image/png;base64,AAAA'); text('done');");
        assert!(good.output.contains("[image: image/png"), "{}", good.output);
        let remote = run_source("image('https://example.com/a.png');");
        assert!(remote.error.expect("remote refused").contains("remote URL"), "a remote image URL is refused");
    }

    #[test]
    fn the_schema_declares_each_tool_with_its_identifier() {
        let schema = schema(&fake_tools());
        assert_eq!(schema["name"], json!(TOOL_NAME));
        let description = schema["description"].as_str().unwrap();
        assert!(description.contains("big_list"), "{description}");
        assert!(description.contains("bound as `big-list`"), "{description}");
        assert!(description.contains("searchTools"), "the deferred-tool helpers are documented: {description}");
        assert_eq!(schema["parameters"]["required"], json!(["code"]));
    }

    #[test]
    fn the_identifier_rule_matches_pis_normalization() {
        assert_eq!(identifier("my-tool"), "my_tool");
        assert_eq!(identifier("mcp__ologs__get-profile"), "mcp__ologs__get_profile");
        assert_eq!(identifier("2fast"), "_2fast");
    }
}
