//! R2 kernel: the no-I/O request/response/observation core (rebuild plan §3).
//!
//! The kernel never touches the database, network or global registries. It
//! converts a context view into a fixed model request, a complete response
//! into executable intents, and observations into context entries. The
//! persistent runtime fills in every identity, operation id, permission
//! revision and delivery sequence number.

pub mod instance;
pub mod types;

pub use instance::{Interpretation, KernelInstance, KernelProfile, COMPACT_AT};
pub use types::*;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn kernel() -> KernelInstance {
        KernelInstance::new(
            "inst-1",
            0,
            KernelProfile {
                model: "deepseek-v4.1-flash".into(),
                instructions: "You are a worker.".into(),
                tools: vec![
                    json!({"type":"function","function":{"name":"shell","parameters":{"type":"object","properties":{"command":{"type":"string"}}}}}),
                ],
                options: json!({"temperature": 0.2}),
                context_window: Some(1_000_000),
            },
        )
    }

    #[test]
    fn prepare_request_builds_system_plus_entries_and_builtins() {
        let kernel = kernel();
        let entries = vec![
            kernel.user_entry("list files", "e1"),
            kernel.apply_observation(
                &Observation::ToolResult {
                    call_id: "c1".into(),
                    name: "shell".into(),
                    content: "a.rs".into(),
                    receipt_ref: "rcpt-1".into(),
                },
                "e2",
            ),
        ];
        let request = kernel.prepare_request(&entries, "req-1");
        assert_eq!(request.request_id, "req-1");
        assert_eq!(request.model, "deepseek-v4.1-flash");
        assert_eq!(request.messages[0], json!({"role":"system","content":"You are a worker."}));
        assert_eq!(request.messages[1]["content"], "list files");
        assert_eq!(request.messages[2]["tool_call_id"], "c1");
        let names: Vec<&str> = request.tools.iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["shell", FINISH_TOOL, READBACK_TOOL]);
        assert_eq!(request.options["temperature"], json!(0.2));
        assert!(request.est_prompt_tokens > 0);
    }

    #[test]
    fn interpret_tool_calls_assigns_index_and_hash() {
        let kernel = kernel();
        let response = ModelResponse {
            message: json!({"role":"assistant","tool_calls":[
                {"id":"c1","function":{"name":"shell","arguments":"{\"command\":\"ls\"}"}},
                {"id":"c2","function":{"name":"shell","arguments":"{\"command\":\"pwd\"}"}}
            ]}),
            usage: Some(Usage { prompt: 10, completion: 5, total: 15 }),
            native: json!({}),
        };
        let out = kernel.interpret_response(&response, "e9");
        match out.output {
            KernelOutput::ToolIntents(intents) => {
                assert_eq!(intents.len(), 2);
                assert_eq!(intents[0].index, 0);
                assert_eq!(intents[1].call_id, "c2");
                assert_eq!(intents[0].args, json!({"command":"ls"}));
                assert_ne!(intents[0].args_hash, intents[1].args_hash);
            }
            other => panic!("expected tool intents, got {other:?}"),
        }
        assert_eq!(out.entry.kind, EntryKind::Assistant);
    }

    #[test]
    fn interpret_plain_reply_does_not_complete() {
        let kernel = kernel();
        let response = ModelResponse {
            message: json!({"role":"assistant","content":"done already"}),
            usage: None,
            native: json!({}),
        };
        let out = kernel.interpret_response(&response, "e9");
        assert_eq!(out.output, KernelOutput::Reply("done already".into()));
    }

    #[test]
    fn sole_finish_is_completion_candidate() {
        let kernel = kernel();
        let response = ModelResponse {
            message: json!({"role":"assistant","tool_calls":[
                {"id":"c1","function":{"name":"finish","arguments":"{\"status\":\"blocked\",\"summary\":\"no key\",\"evidence\":[\"x\"],\"unverified\":[\"y\"]}"}}
            ]}),
            usage: None,
            native: json!({}),
        };
        let out = kernel.interpret_response(&response, "e9");
        match out.output {
            KernelOutput::Completion(candidate) => {
                assert_eq!(candidate.outcome, Outcome::Blocked);
                assert_eq!(candidate.summary, "no key");
                assert_eq!(candidate.evidence, vec!["x"]);
                assert_eq!(candidate.unverified, vec!["y"]);
            }
            other => panic!("expected completion, got {other:?}"),
        }
        assert!(out.notes.is_empty());
    }

    #[test]
    fn finish_with_other_calls_is_ignored_with_note() {
        let kernel = kernel();
        let response = ModelResponse {
            message: json!({"role":"assistant","tool_calls":[
                {"id":"c1","function":{"name":"shell","arguments":"{\"command\":\"ls\"}"}},
                {"id":"c2","function":{"name":"finish","arguments":"{\"status\":\"success\",\"summary\":\"x\"}"}}
            ]}),
            usage: None,
            native: json!({}),
        };
        let out = kernel.interpret_response(&response, "e9");
        match out.output {
            KernelOutput::ToolIntents(intents) => {
                assert_eq!(intents.len(), 1);
                assert_eq!(intents[0].name, "shell");
            }
            other => panic!("expected tool intents, got {other:?}"),
        }
        assert_eq!(out.notes.len(), 1);
    }

    #[test]
    fn invalid_arguments_are_preserved_not_dropped() {
        let kernel = kernel();
        let response = ModelResponse {
            message: json!({"role":"assistant","tool_calls":[
                {"id":"c1","function":{"name":"shell","arguments":"{broken"}}
            ]}),
            usage: None,
            native: json!({}),
        };
        let out = kernel.interpret_response(&response, "e9");
        match out.output {
            KernelOutput::ToolIntents(intents) => {
                assert_eq!(intents[0].args["_invalid_arguments"], "{broken");
            }
            other => panic!("expected tool intents, got {other:?}"),
        }
    }

    #[test]
    fn masking_hides_old_outputs_and_keeps_readback_recipe() {
        let kernel = KernelInstance::new(
            "inst-1",
            0,
            KernelProfile {
                model: "m".into(),
                instructions: "i".into(),
                tools: vec![],
                options: json!({}),
                // tiny window forces masking of everything older
                context_window: Some(4),
            },
        );
        let big = "x".repeat(40_000);
        let entries = vec![
            kernel.apply_observation(
                &Observation::ToolResult {
                    call_id: "c1".into(),
                    name: "shell".into(),
                    content: big.clone(),
                    receipt_ref: "r1".into(),
                },
                "e1",
            ),
            ContextEntry::new("e2", EntryKind::Assistant, json!({"role":"assistant","content":"next"})),
        ];
        let request = kernel.prepare_request(&entries, "req");
        let tool_msg = &request.messages[1];
        let content = tool_msg["content"].as_str().unwrap();
        assert!(content.contains("tool output hidden"), "content was: {content}");
        assert!(content.contains("read_history"));
        assert!(content.contains("c1"));
        // originals stay in the store; only the wire copy is masked
        assert_eq!(entries[0].message["content"].as_str().unwrap().len(), 40_000);
    }

    #[test]
    fn a_sole_wait_call_is_the_wait_output() {
        let kernel = kernel();
        let response = ModelResponse {
            message: json!({"role":"assistant","tool_calls":[
                {"id":"w1","function":{"name": WAIT_TOOL, "arguments":
                    "{\"mode\":\"ANY\",\"conditions\":[{\"kind\":\"message\",\"from\":\"i2\"}],\"timer_seconds\":30}"}}
            ]}),
            usage: None,
            native: json!({}),
        };
        let out = kernel.interpret_response(&response, "e9");
        match out.output {
            KernelOutput::Wait(wait) => {
                assert_eq!(wait["mode"], json!("ANY"));
                assert_eq!(wait["timer_seconds"], json!(30));
            }
            other => panic!("expected wait, got {other:?}"),
        }
    }

    #[test]
    fn a_combined_wait_is_ignored_with_a_note() {
        let kernel = kernel();
        let response = ModelResponse {
            message: json!({"role":"assistant","tool_calls":[
                {"id":"w1","function":{"name": WAIT_TOOL, "arguments": "{\"mode\":\"ALL\",\"conditions\":[{\"kind\":\"task\",\"task_id\":\"t1\"}]}"}},
                {"id":"c1","function":{"name":"shell","arguments":"{\"command\":\"ls\"}"}}
            ]}),
            usage: None,
            native: json!({}),
        };
        let out = kernel.interpret_response(&response, "e9");
        assert!(out.notes.iter().any(|note| note.contains("ignored wait")), "{:?}", out.notes);
        match out.output {
            KernelOutput::ToolIntents(intents) => {
                assert_eq!(intents.len(), 1);
                assert_eq!(intents[0].name, "shell");
            }
            other => panic!("expected tool intents, got {other:?}"),
        }
    }

    #[test]
    fn collaboration_schemas_follow_the_requested_actions() {
        let schemas = collaboration_tool_schemas(&[WAIT_TOOL, SEND_TOOL]);
        let names: Vec<&str> = schemas.iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec![WAIT_TOOL, SEND_TOOL]);
        assert!(collaboration_tool_schemas(&[]).is_empty());
    }

    #[test]
    fn page_output_contract() {
        let out = page_output("hello world", &json!({"offset": 6, "limit": 5})).unwrap();
        assert_eq!(out["output"], "world");
        assert_eq!(out["eof"], true);
        assert!(page_output("abc", &json!({"limit": 0})).is_err());
        assert!(page_output("abc", &json!({"offset": 4})).is_err());
    }

    #[test]
    fn usage_parses_openai_and_anthropic_shapes() {
        assert_eq!(
            Usage::from_json(&json!({"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}})),
            Some(Usage { prompt: 10, completion: 4, total: 14 })
        );
        assert_eq!(
            Usage::from_json(&json!({"usage":{"input_tokens":7,"output_tokens":3}})),
            Some(Usage { prompt: 7, completion: 3, total: 10 })
        );
        assert_eq!(Usage::from_json(&json!({})), None);
    }
}
