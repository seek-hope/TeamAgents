//! Codemode: one tool that runs a model-written JavaScript program whose only capabilities are the bound MCP
//! tools (D-374, learned from pi's `codemode`).
//!
//! The point is the information-flow shape, not the scripting: a script may call many MCP tools, loop, filter
//! and chain them, and **only the script's own `text()` output and return value enter the model's context**.
//! Nested tool results stay in the VM. That is why MCP tools stop being advertised as individual tools once
//! this is on: the model composes them in code instead of paying for every intermediate payload.
//!
//! The engine is QuickJS (through `rquickjs`), the same one pi's codemode uses. Its only imports are the host
//! functions this module registers, so a script cannot fetch, read files, spawn processes or time out on its
//! own. A nested MCP call is synchronous on the host side, so `await tools.x(args)` resolves without an async
//! runtime; `tools.x` returns the tool's JSON value directly and rejects with an `Error` carrying the tool's
//! error text.
//!
//! `store`/`load` keep JSON values across calls **within one toolkit** (an instance's boot); they are not
//! persisted, which is the v1 ceiling.

use crate::bound::BoundTools;
use rquickjs::{Context, Function, Promise, Runtime, Value};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The name the model sees. One tool for every bound MCP tool.
pub const TOOL_NAME: &str = "codemode";

/// A runaway script must not grow the wasm32 heap without bound; QuickJS reports `out of memory` inside the
/// script when this is crossed (pi's codemode uses the same 256 MiB).
const MEMORY_LIMIT_BYTES: usize = 256 * 1024 * 1024;
/// The script's own output is what reaches the context, so it is capped even when the script prints more.
const MAX_OUTPUT_CHARS: usize = 200_000;
/// Default hard deadline for one script; a `// @options:` line may override it per call.
pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// A `// @options: {"timeout_ms": …}` first line, as pi's codemode source grammar allows.
#[derive(Debug, Default, PartialEq)]
struct Options {
    timeout_ms: Option<u64>,
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
            format!("{}[]", schema.get("items").map(|i| ts_type(i, depth + 1)).unwrap_or("unknown".into()))
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

/// The model-facing tool schema: the intro text plus a declaration of every bound MCP tool. Only called when at
/// least one tool is bound (the toolkit gates it), so the description never advertises an empty `tools`.
pub fn schema(tools: &BoundTools) -> Json {
    let mut declarations = String::new();
    for tool in &tools.tools {
        let ident = identifier(&tool.name);
        let alias = if ident == tool.name { String::new() } else { format!(" (bound as `{}`)", tool.name) };
        declarations.push_str(&format!(
            "\n  /** {}{} */\n  {}(args: {}): Promise<unknown>;",
            tool.description.replace("*/", "* /").replace('\n', " "),
            alias,
            ident,
            ts_type(&tool.parameters, 0)
        ));
    }
    let description = format!(
        "Run JavaScript that calls MCP tools as `await tools.<name>(args)`.\n\
         - The code is evaluated in a fresh QuickJS sandbox as the body of an async function: top-level `await` and `return` work.\n\
         - Tool names are normalized to JavaScript identifiers (`my-tool` is `tools.my_tool`); `ALL_TOOLS` lists `{{name, description}}`.\n\
         - A nested call that fails rejects with an `Error` carrying the tool's error text. Calls are real and have side effects; earlier calls are not undone when a later one fails.\n\
         - Only what the script emits with `text(...)`/`console.log(...)` and what it returns reaches you — nested tool output does NOT. Filter and aggregate in the script instead of returning raw payloads.\n\
         - No Node, no file system, no network, no timers; a 256 MB heap and a hard deadline apply.\n\
         - `store(key, value)`/`load(key)` persist JSON values across codemode calls in this session. `exit()` ends the script successfully.\n\
         - The first line may be `// @options: {{\"timeout_ms\": 60000}}`.\n\n\
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
        return Ok((tail.to_string(), options));
    }
    Ok((source.to_string(), options))
}

/// The JS prelude. It is the only thing a script can reach besides its own code: everything here is either a
/// value, a wrapper over the host functions below, or a pure JavaScript helper.
fn prelude(names: &BTreeMap<String, String>, all_tools: &Json) -> String {
    format!(
        r#"
globalThis.__names = {names};
globalThis.ALL_TOOLS = {all};
globalThis.tools = new Proxy({{}}, {{
  get: (_target, property) => {{
    const key = String(property);
    const real = Object.prototype.hasOwnProperty.call(__names, key) ? __names[key] : key;
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
globalThis.store = (key, value) => __store(String(key), value === undefined ? null : JSON.stringify(value));
globalThis.load = (key) => {{ const raw = __load(String(key)); return raw === null || raw === undefined ? undefined : JSON.parse(raw); }};
globalThis.exit = () => {{ throw {{ __codemode_exit: true }}; }};
"#,
        names = serde_json::to_string(names).unwrap_or_else(|_| "{}".into()),
        all = serde_json::to_string(all_tools).unwrap_or_else(|_| "[]".into()),
    )
}

/// Run one script. `tools` supplies the only capabilities; `store` is the cross-call JSON store.
///
/// Returns the text the script produced (its `text()`/`console` output and its top-level return value), or the
/// script's error. A nested call's payload never appears in the returned text unless the script chose to emit it.
pub fn run(
    source: &str,
    tools: Arc<BoundTools>,
    store: Arc<Mutex<BTreeMap<String, Json>>>,
    timeout_ms: u64,
) -> Result<String, String> {
    if tools.tools.is_empty() {
        return Err("codemode is not available: no MCP tool is bound to this member".into());
    }
    let (body, options) = parse_source(source)?;
    if body.trim().is_empty() {
        return Err("codemode: the script is empty".into());
    }
    let deadline = Instant::now() + Duration::from_millis(options.timeout_ms.unwrap_or(timeout_ms).max(1));
    let runtime = Runtime::new().map_err(|error| format!("codemode: {error}"))?;
    runtime.set_memory_limit(MEMORY_LIMIT_BYTES);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = Context::full(&runtime).map_err(|error| format!("codemode: {error}"))?;

    let mut names = BTreeMap::new();
    let mut all_tools = Vec::new();
    for tool in &tools.tools {
        names.insert(identifier(&tool.name), tool.name.clone());
        all_tools.push(json!({"name": identifier(&tool.name), "description": tool.description}));
    }
    let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let outcome: Result<(), String> = context.with(|ctx| {
        let prepared: Result<(), String> = (|| {
            let host = tools.clone();
            let host_fn = Function::new(ctx.clone(), move |name: String, args: String| -> String {
                let parsed: Json = serde_json::from_str(&args).unwrap_or(Json::Null);
                match host.call(&name, &parsed) {
                    Some(Ok(value)) => json!({"ok": true, "value": value}).to_string(),
                    Some(Err(error)) => json!({"ok": false, "error": error}).to_string(),
                    None => json!({"ok": false, "error": format!("unknown tool {name:?}")}).to_string(),
                }
            })
            .map_err(|error| error.to_string())?;
            let setter = {
                let store = store.clone();
                Function::new(ctx.clone(), move |key: String, value: Option<String>| {
                    let mut store = store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    match value {
                        None => {
                            store.remove(&key);
                        }
                        Some(raw) => {
                            if let Ok(parsed) = serde_json::from_str::<Json>(&raw) {
                                store.insert(key, parsed);
                            }
                        }
                    }
                })
                .map_err(|error| error.to_string())?
            };
            let getter = {
                let store = store.clone();
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
            ctx.globals().set("__call", host_fn).map_err(|error| error.to_string())?;
            ctx.globals().set("__store", setter).map_err(|error| error.to_string())?;
            ctx.globals().set("__load", getter).map_err(|error| error.to_string())?;
            ctx.globals().set("__text", emitter).map_err(|error| error.to_string())?;
            ctx.eval::<(), _>(prelude(&names, &Json::Array(all_tools))).map_err(|_| exception(&ctx))?;
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
                    // `ctx.catch()` consumes the pending exception, so it is read once and then classified
                    let value: Value = ctx.catch();
                    if is_exit(&value) {
                        Ok(())
                    } else {
                        Err(exception_of(value))
                    }
                }
            },
            Err(_) => Err(exception(&ctx)),
        }
    });

    match outcome {
        Ok(()) => {
            let text = output.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).join("\n");
            if text.chars().count() > MAX_OUTPUT_CHARS {
                let head: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
                Ok(format!("{head}\n… (codemode output truncated)"))
            } else if text.is_empty() {
                Ok("(the script produced no output)".into())
            } else {
                Ok(text)
            }
        }
        Err(error) => Err(error),
    }
}

/// The pending exception as `Name: message` plus the first stack line, which is what the model needs to fix it.
fn exception(ctx: &rquickjs::Ctx<'_>) -> String {
    exception_of(ctx.catch())
}

/// Format an already-caught exception. Split out because `Ctx::catch` consumes it: the exit marker and the
/// error text must be read from the same value.
fn exception_of(value: Value<'_>) -> String {
    if let Some(exception) = value.as_exception() {
        let message = exception.message().unwrap_or_default();
        return match exception.stack() {
            Some(stack) => format!("codemode: {message}\n{}", stack.lines().take(2).collect::<Vec<_>>().join("\n")),
            None => format!("codemode: {message}"),
        };
    }
    format!("codemode: script failed: {value:?}")
}

/// `exit()` throws a marker object, which is a successful end, not an error.
fn is_exit(value: &Value<'_>) -> bool {
    value
        .as_object()
        .and_then(|object| object.get::<_, Option<bool>>("__codemode_exit").ok().flatten())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bound::BoundTool;

    fn fake_tools() -> (Arc<BoundTools>, Arc<Mutex<BTreeMap<String, Json>>>) {
        // Two tools stand in for an MCP service: one echoes, one returns a larger payload the script must filter.
        let tools = Arc::new(BoundTools {
            tools: vec![
                BoundTool {
                    name: "mcp_echo".into(),
                    description: "Echo the argument back".into(),
                    parameters: json!({"type": "object", "properties": {"value": {"type": "string"}}, "required": ["value"]}),
                    remote: None,
                },
                BoundTool {
                    name: "big-list".into(),
                    description: "Return a list of records".into(),
                    parameters: json!({"type": "object", "properties": {}}),
                    remote: None,
                },
            ],
        });
        (tools, Arc::new(Mutex::new(BTreeMap::new())))
    }

    fn run_with(body: &str) -> Result<String, String> {
        let (tools, store) = fake_tools();
        run(body, tools, store, DEFAULT_TIMEOUT_MS)
    }

    #[test]
    fn a_script_emits_its_text_and_its_return_value() {
        let output = run_with(r#"text("hello " + (1 + 1)); return {ok: true};"#).expect("script runs");
        assert!(output.contains("hello 2"), "{output}");
        assert!(output.contains("\"ok\":true"), "the returned value is emitted like text(): {output}");
    }

    #[test]
    fn a_script_that_calls_an_unbound_tool_rejects_with_the_tools_error() {
        let error = run_with("return await tools.mcp_echo({ value: 'x' });").unwrap_err();
        assert!(
            error.contains("unknown tool"),
            "a host call to a tool the BoundTools do not carry is refused: {error}"
        );
    }

    #[test]
    fn a_script_cannot_reach_the_host_beyond_the_registered_functions() {
        for body in ["return typeof fetch;", "return typeof process;", "return typeof require;"] {
            let output = run_with(body).expect("the script still runs");
            assert!(output.contains("undefined"), "no host capability is exposed ({body}): {output}");
        }
    }

    #[test]
    fn store_and_load_survive_inside_one_store_and_exit_ends_successfully() {
        let (tools, store) = fake_tools();
        run("store('n', 41); text('stored');", tools.clone(), store.clone(), DEFAULT_TIMEOUT_MS).expect("first run");
        let second =
            run("text(String(load('n') + 1));", tools.clone(), store.clone(), DEFAULT_TIMEOUT_MS).expect("second run");
        assert!(second.contains("42"), "{second}");
        let exited =
            run("text('before'); exit(); text('after');", tools, store, DEFAULT_TIMEOUT_MS).expect("exit is success");
        assert!(exited.contains("before") && !exited.contains("after"), "{exited}");
    }

    #[test]
    fn the_options_line_sets_the_deadline_and_unknown_fields_are_refused() {
        assert_eq!(parse_source("// @options: {\"timeout_ms\": 5000}\nreturn 1;").unwrap().1.timeout_ms, Some(5000));
        assert!(parse_source("// @options: {\"nope\": 1}\nreturn 1;").unwrap_err().contains("unknown @options"));
        // an infinite loop is stopped by the interrupt handler, not left to hang the daemon
        let error = run_with("// @options: {\"timeout_ms\": 200}\nwhile (true) {}").unwrap_err();
        assert!(error.to_lowercase().contains("interrupt") || error.contains("codemode"), "{error}");
    }

    #[test]
    fn the_schema_declares_each_tool_with_its_identifier() {
        let (tools, _) = fake_tools();
        let schema = schema(&tools);
        assert_eq!(schema["name"], json!(TOOL_NAME));
        let description = schema["description"].as_str().unwrap();
        assert!(description.contains("big_list"), "{description}");
        assert!(description.contains("bound as `big-list`"), "{description}");
        assert_eq!(schema["parameters"]["required"], json!(["code"]));
    }

    #[test]
    fn the_identifier_rule_matches_pis_normalization() {
        assert_eq!(identifier("my-tool"), "my_tool");
        assert_eq!(identifier("mcp__ologs__get-profile"), "mcp__ologs__get_profile");
        assert_eq!(identifier("2fast"), "_2fast");
    }
}
