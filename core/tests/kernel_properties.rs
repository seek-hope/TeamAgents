//! Bounded exhaustive checks for the pure functions (wire view, output capping,
//! paging and response classification).
//!
//! These functions never touch the database, so their properties can be enumerated
//! directly; they are the spec's "view / wire protocol" part (pairing in
//! `pair_tool_results` and the coordinates of `page_output`).
//!
//! ```text
//! cargo test --offline --manifest-path core/Cargo.toml --test kernel_properties
//! ```

use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use teamagents_core::kernel::{
    cap_tool_output, page_output, ContextEntry, EntryKind, KernelInstance, KernelOutput, KernelProfile, ModelResponse,
    FINISH_TOOL, TOOL_OUTPUT_CAP, WAIT_TOOL,
};

fn kernel() -> KernelInstance {
    KernelInstance::new(
        "inst-1",
        0,
        KernelProfile {
            images: false,
            model: "deepseek-v4.1-flash".into(),
            instructions: "You are a worker.".into(),
            tools: vec![json!({"type":"function","function":{"name":"shell","parameters":{"type":"object"}}})],
            options: json!({}),
            context_window: Some(1_000_000),
        },
    )
}

/// Multiset of messages (for exact comparison; the test inputs are tiny and never capped).
fn multiset(messages: &[Json]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for message in messages {
        *counts.entry(message.to_string()).or_insert(0) += 1;
    }
    counts
}

fn call(id: &str, name: &str) -> Json {
    json!({"id": id, "type": "function", "function": {"name": name, "arguments": "{}"}})
}

/// The view domain: the entry shapes a wire-protocol view can meet.
fn alphabet(index: usize) -> ContextEntry {
    let id = format!("e{index}");
    match index % 6 {
        0 => ContextEntry::new(id, EntryKind::User, json!({"role": "user", "content": "ask"})),
        1 => ContextEntry::new(id, EntryKind::Assistant, json!({"role": "assistant", "content": "reply"})),
        2 => ContextEntry::new(
            id,
            EntryKind::Assistant,
            json!({"role": "assistant", "content": "", "tool_calls": [call("c1", "shell")]}),
        ),
        3 => {
            ContextEntry::new(id, EntryKind::ToolResult, json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}))
        }
        4 => ContextEntry::new(id, EntryKind::Note, json!({"role": "user", "content": "[note]"})),
        _ => ContextEntry::new(
            id,
            EntryKind::Assistant,
            json!({"role": "assistant", "content": "", "tool_calls": [call("c1", "shell"), call("c2", "shell")]}),
        ),
    }
}

/// Wire invariants of `prepare_request` (D-62): the system prompt comes first, the input
/// survives in order, **every** tool call is immediately followed by an answer — a call the
/// runtime answered outside the tool channel gains one synthesized answer — and no answer is
/// an orphan. Without the synthesized answers a strict endpoint rejects the request, which is
/// what a real model did after an instance finished a turn and then received new work.
#[test]
fn wire_view_is_a_paired_permutation() {
    let kernel = kernel();
    // every combination of length <= 3 (6 entry shapes)
    let mut cases: Vec<Vec<ContextEntry>> = Vec::new();
    for first in 0..6 {
        cases.push(vec![alphabet(first)]);
        for second in 0..6 {
            cases.push(vec![alphabet(first), alphabet(second)]);
            for third in 0..6 {
                cases.push(vec![alphabet(first), alphabet(second), alphabet(third)]);
            }
        }
    }
    // plus a few longer cases that deliberately land an answer after other entries
    cases.push(vec![alphabet(2), alphabet(4), alphabet(3), alphabet(1), alphabet(0)]);
    cases.push(vec![alphabet(5), alphabet(4), alphabet(0), alphabet(1), alphabet(3), alphabet(2)]);

    // coverage assertion: at least one case must really move an answer behind its call, and at
    // least one must gain a synthesized answer (the D-62 fix), or the properties are vacuous
    let mut moved = 0usize;
    let mut synthesized = 0usize;
    for entries in cases {
        let request = kernel.prepare_request(&entries, "req");
        let messages = &request.messages;
        assert_eq!(messages[0]["role"], json!("system"), "the system prompt must come first");

        let expected: Vec<Json> = entries.iter().map(|entry| entry.message.clone()).collect();
        // the only messages the wire may add are the answers a call never got in the log
        // the ids the instances really called: a synthesized answer may only name one of
        // these (the pairing algorithm itself is what decides which calls need one, so this
        // test states the properties rather than predicting the counts)
        let input_call_ids: Vec<String> = expected
            .iter()
            .flat_map(|message| message["tool_calls"].as_array().into_iter().flatten())
            .filter_map(|call| call["id"].as_str().map(str::to_string))
            .collect();
        assert!(
            messages.len() > entries.len(),
            "the wire never drops an entry (the system prompt is added): {messages:?}"
        );
        let mut preserved = multiset(&expected);
        let mut added = 0usize;
        for (message, count) in multiset(&messages[1..]) {
            let stored = preserved.get(&message).copied().unwrap_or(0);
            let extra = count.saturating_sub(stored);
            if let Some(remaining) = preserved.get_mut(&message) {
                *remaining = remaining.saturating_sub(count);
            }
            for _ in 0..extra {
                // an addition must be a synthesized answer for a call the log left open
                let parsed: Json = serde_json::from_str(&message).expect("message JSON");
                assert_eq!(parsed["role"], json!("tool"), "only answers may be added: {parsed}");
                let id = parsed["tool_call_id"].as_str().unwrap_or("");
                assert!(
                    input_call_ids.iter().any(|called| called == id),
                    "an added answer must name a call the instance made: {parsed}"
                );
                assert!(
                    parsed["content"].as_str().unwrap_or("").contains("no tool result follows"),
                    "a synthesized answer must say what it is: {parsed}"
                );
                added += 1;
            }
        }
        assert!(
            preserved.values().all(|remaining| *remaining == 0),
            "every entry survives: input {expected:?} wire {messages:?}"
        );
        synthesized += added;
        // Every call in the wire — answered or synthesized — is immediately followed by an answer.
        // Skipped for an impossible history (a call id answered twice, or two responses that reuse
        // one call id): the runtime appends one answer per call, deduplicated by operation, so the
        // view domain is wider than the histories the code can produce.
        let mut seen_calls: Vec<&str> = Vec::new();
        let mut seen_answers: Vec<&str> = Vec::new();
        let mut ambiguous = false;
        for message in &expected {
            for call in message["tool_calls"].as_array().into_iter().flatten() {
                let id = call["id"].as_str().unwrap_or("");
                ambiguous |= seen_calls.contains(&id);
                seen_calls.push(id);
            }
        }
        for message in &expected {
            if message["role"] != json!("tool") {
                continue;
            }
            let id = message["tool_call_id"].as_str().unwrap_or("");
            ambiguous |= seen_answers.contains(&id);
            seen_answers.push(id);
        }
        if ambiguous {
            continue;
        }
        for (index, message) in messages[1..].iter().enumerate() {
            let Some(calls) = message["tool_calls"].as_array() else { continue };
            let ids: Vec<&str> = calls.iter().filter_map(|call| call["id"].as_str()).collect();
            let answers: Vec<(Json, Json)> = messages[index + 2..]
                .iter()
                .take(ids.len())
                .map(|answer| (answer["role"].clone(), answer["tool_call_id"].clone()))
                .collect();
            let expected: Vec<(Json, Json)> = ids.iter().map(|id| (json!("tool"), json!(id))).collect();
            assert_eq!(answers, expected, "every call of one response is answered, in order, right after it");
        }

        // pairing: a real log entry whose answer lands after its call must be moved right
        // behind the call; the reverse order cannot occur in the context (the runtime appends
        // the call first) and is covered by the permutation property alone
        let find_call = |items: &[Json], id: &str| -> Option<usize> {
            items.iter().position(|message| {
                message["tool_calls"].as_array().into_iter().flatten().any(|call| call["id"].as_str() == Some(id))
            })
        };
        let find_answers = |items: &[Json], id: &str| -> Vec<usize> {
            items
                .iter()
                .enumerate()
                .filter(|(_, message)| message["role"] == json!("tool") && message["tool_call_id"].as_str() == Some(id))
                .map(|(index, _)| index)
                .collect()
        };
        for id in ["c1", "c2"] {
            let answer_in_input = find_answers(&expected, id);
            let call_in_input = find_call(&expected, id);
            let (Some(&answer), Some(call)) = (answer_in_input.first(), call_in_input) else { continue };
            if answer <= call {
                continue; // impossible history (answer before its call)
            }
            let answers = find_answers(&messages[1..], id);
            let call_in_wire = find_call(&messages[1..], id).expect("the call survives pairing");
            let first = answers.first().copied().expect("the answer survives pairing");
            assert_eq!(
                call_in_wire + 1,
                first,
                "answer {id} must directly follow its call (strict endpoints reject otherwise)"
            );
            if answer != call + 1 {
                moved += 1;
            }
        }
        // assistants keep their relative order
        let assistants = |items: &[Json]| -> Vec<String> {
            items.iter().filter(|m| m["role"] == json!("assistant")).map(|m| m.to_string()).collect()
        };
        assert_eq!(assistants(&expected), assistants(&messages[1..]), "assistant order must be preserved");
    }
    assert!(moved >= 5, "the pairing property must really fire (moved {moved} times)");
    assert!(synthesized >= 10, "the synthesized-answer property must really fire ({synthesized} times)");
}

/// The wire must answer **every** call it carries (D-62): a strict endpoint rejects an
/// assistant message whose `tool_calls` are not followed by tool messages, and an accepted
/// `finish` is exactly such a call — the runtime answers it by settling the turn, not with a
/// tool result, so the *next* request of that instance was rejected with HTTP 400 by a real
/// model. The synthesized answer says what it is, and a call that does have an answer keeps it
/// (the receipt D-54's refusal path produces is not duplicated).
#[test]
fn the_wire_answers_every_call_it_carries() {
    let kernel = kernel();
    let assistant = |calls: Vec<Json>| {
        ContextEntry::new("a", EntryKind::Assistant, json!({"role": "assistant", "content": "…", "tool_calls": calls}))
    };
    let answer = |id: &str, content: &str| {
        ContextEntry::new("t", EntryKind::ToolResult, json!({"role": "tool", "tool_call_id": id, "content": content}))
    };
    let cases = vec![
        // a completion candidate: the call the settlement answers
        (vec![assistant(vec![call("finish-1", FINISH_TOOL)])], vec!["finish-1"], 1),
        // two calls, neither answered
        (vec![assistant(vec![call("c1", "shell"), call("c2", "shell")])], vec!["c1", "c2"], 2),
        // one answered, one not: the real one survives, the other gains an answer
        (vec![assistant(vec![call("c1", "shell"), call("c2", "shell")]), answer("c1", "ok")], vec!["c1", "c2"], 1),
        // fully answered: nothing is added
        (vec![assistant(vec![call("c1", "shell")]), answer("c1", "ok")], vec!["c1"], 0),
        // an answer that landed behind other entries is moved up, and nothing is added
        (vec![assistant(vec![call("c1", "shell")]), alphabet(0), answer("c1", "ok")], vec!["c1"], 0),
    ];
    for (entries, ids, synthesized) in cases {
        let messages = kernel.prepare_request(&entries, "req").messages;
        let answers: Vec<(Json, Json)> = messages
            .iter()
            .filter(|message| message["role"] == json!("tool"))
            .map(|message| (message["tool_call_id"].clone(), message["content"].clone()))
            .collect();
        assert_eq!(answers.len(), ids.len(), "{messages:?}");
        assert_eq!(
            answers.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            ids.iter().map(|id| json!(id)).collect::<Vec<_>>(),
            "{messages:?}"
        );
        let invented = answers
            .iter()
            .filter(|(_, content)| content.as_str().unwrap_or("").contains("no tool result follows"))
            .count();
        assert_eq!(invented, synthesized, "{messages:?}");
        // and the answers hug their calls
        for (index, message) in messages.iter().enumerate() {
            let Some(calls) = message["tool_calls"].as_array() else { continue };
            for (offset, call) in calls.iter().enumerate() {
                let next = &messages[index + 1 + offset];
                assert_eq!(next["role"], json!("tool"), "{messages:?}");
                assert_eq!(next["tool_call_id"], call["id"], "{messages:?}");
            }
        }
        // a stored answer is never an orphan
        for (index, message) in messages.iter().enumerate() {
            if message["role"] != json!("tool") {
                continue;
            }
            let id = message["tool_call_id"].as_str().unwrap_or("");
            assert!(
                messages[..index].iter().any(|earlier| earlier["tool_calls"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|call| call["id"].as_str() == Some(id))),
                "no orphan tool answer: {message}"
            );
        }
    }
}

/// Output capping: short content passes through, long content keeps head and tail within
/// a bound (the model never sees unbounded output).
#[test]
fn tool_output_cap_keeps_head_and_tail_within_bounds() {
    for length in [0usize, 1, TOOL_OUTPUT_CAP - 1, TOOL_OUTPUT_CAP, TOOL_OUTPUT_CAP + 1, TOOL_OUTPUT_CAP * 2] {
        // every position holds a distinct character, so head and tail are locatable
        let content: String =
            (0..length).map(|index| char::from_u32(0x4E00 + (index % 2000) as u32).unwrap()).collect();
        let capped = cap_tool_output(&content);
        if length <= TOOL_OUTPUT_CAP {
            assert_eq!(capped, content, "content within the cap must not be rewritten");
            continue;
        }
        let capped_len = capped.chars().count();
        assert!(capped_len <= TOOL_OUTPUT_CAP + 64, "the capped length must stay bounded: {capped_len}");
        if length > TOOL_OUTPUT_CAP + 64 {
            assert!(capped_len < length, "far beyond the cap the text must really shrink");
        }
        let half = TOOL_OUTPUT_CAP / 2;
        let head: String = content.chars().take(half).collect();
        let tail: String = content.chars().skip(length - half).collect();
        assert!(capped.starts_with(&head), "the head must be kept");
        assert!(capped.ends_with(&tail), "the tail must be kept");
        assert!(capped.contains("truncated"), "truncation must be marked");
    }
}

/// Paging (readback): page-by-page retrieval with a limit rebuilds the original text
/// seamlessly, the coordinates agree, and out-of-range requests fail loudly.
#[test]
fn paging_reconstructs_the_original_without_gaps() {
    for length in 0..=12usize {
        let content: String = (0..length).map(|index| char::from_digit((index % 10) as u32, 10).unwrap()).collect();
        for limit in 1..=5usize {
            let mut rebuilt = String::new();
            let mut offset = 0usize;
            let mut pages = 0;
            loop {
                let page = page_output(&content, &json!({"offset": offset, "limit": limit})).expect("page");
                assert_eq!(page["offset"], json!(offset));
                assert_eq!(page["total_chars"], json!(length));
                let text = page["output"].as_str().expect("output text");
                assert!(text.chars().count() <= limit, "a page must not exceed limit");
                assert_eq!(
                    text,
                    content.chars().skip(offset).take(limit).collect::<String>(),
                    "page content must match the coordinates"
                );
                rebuilt.push_str(text);
                pages += 1;
                assert!(pages <= length + 2, "the page count must be bounded");
                if page["eof"] == json!(true) {
                    assert!(page["next_offset"].is_null(), "eof must not advertise another page");
                    break;
                }
                let next = page["next_offset"].as_u64().expect("next_offset") as usize;
                assert_eq!(next, offset + text.chars().count(), "next_offset must equal the consumed length");
                offset = next;
            }
            assert_eq!(rebuilt, content, "page-by-page retrieval must rebuild the text seamlessly");
        }
    }
    // out-of-range and invalid arguments fail loudly instead of silently truncating
    assert!(page_output("abc", &json!({"limit": 0})).is_err());
    assert!(page_output("abc", &json!({"limit": 12_001})).is_err());
    assert!(page_output("abc", &json!({"offset": 4})).is_err());
    assert!(page_output("abc", &json!({"offset": 3})).is_ok(), "offset == length is a legal empty page");
}

/// Response classification: finish / wait must be the only call; mixing them only earns a
/// note; an empty response is an ordinary reply.
#[test]
fn response_classification_is_exhaustive() {
    let kernel = kernel();
    let finish = json!({"role": "assistant", "tool_calls": [{"id": "f1", "function": {"name": FINISH_TOOL,
              "arguments": "{\"status\":\"success\",\"summary\":\"s\",\"evidence\":[\"e\"]}"}}]});
    let wait = json!({"role": "assistant", "tool_calls": [{"id": "w1", "function": {"name": WAIT_TOOL,
                   "arguments": "{\"mode\":\"ANY\",\"conditions\":[{\"kind\":\"message\",\"from\":\"i2\"}]}"}}]});
    let tool = json!({"role": "assistant", "tool_calls": [call("c1", "shell")]});
    let blank = json!({"role": "assistant", "content": ""});
    let text = json!({"role": "assistant", "content": "hello"});

    let classify = |message: Json| -> (String, usize) {
        let response = ModelResponse { message, usage: None, native: json!({}) };
        let out = kernel.interpret_response(&response, "e1");
        let kind = match &out.output {
            KernelOutput::Completion(candidate) => format!("completion:{}", serde_json::to_string(candidate).unwrap()),
            KernelOutput::Wait(args) => format!("wait:{}", args["mode"].as_str().unwrap_or("")),
            KernelOutput::ToolIntents(intents) => format!("intents:{}", intents.len()),
            KernelOutput::Reply(content) => format!("reply:{content}"),
        };
        (kind, out.notes.len())
    };

    assert!(classify(finish.clone()).0.starts_with("completion:"));
    assert_eq!(classify(wait.clone()).0, "wait:ANY");
    assert_eq!(
        classify(json!({"role":"assistant","tool_calls":[call("c1","shell"), call("c2","shell")]})).0,
        "intents:2"
    );
    assert_eq!(classify(blank).0, "reply:");
    assert_eq!(classify(text).0, "reply:hello");
    // finish/wait mixed with other calls: drop them with a note, run the rest
    let mut mixed = tool.clone();
    mixed["tool_calls"].as_array_mut().unwrap().push(finish["tool_calls"][0].clone());
    let (kind, notes) = classify(mixed);
    assert_eq!(kind, "intents:1");
    assert_eq!(notes, 1);
    let mut mixed_wait = tool.clone();
    mixed_wait["tool_calls"].as_array_mut().unwrap().push(wait["tool_calls"][0].clone());
    let (kind, notes) = classify(mixed_wait);
    assert_eq!(kind, "intents:1");
    assert_eq!(notes, 1);
}

/// The argument hash is deterministic: equal arguments always yield the same args_hash
/// (receipts and replays rely on it).
#[test]
fn args_hash_is_deterministic() {
    let kernel = kernel();
    let response = |args: &str| ModelResponse {
        message: json!({"role": "assistant", "tool_calls": [
            {"id": "c1", "function": {"name": "shell", "arguments": args}}]}),
        usage: None,
        native: json!({}),
    };
    let first = kernel.interpret_response(&response("{\"command\":\"ls\"}"), "e1");
    let second = kernel.interpret_response(&response("{\"command\":\"ls\"}"), "e2");
    let third = kernel.interpret_response(&response("{\"command\":\"pwd\"}"), "e3");
    let hashes: Vec<String> = [&first, &second, &third]
        .iter()
        .map(|out| match &out.output {
            KernelOutput::ToolIntents(intents) => intents[0].args_hash.clone(),
            other => panic!("expected intents, got {other:?}"),
        })
        .collect();
    assert_eq!(hashes[0], hashes[1], "equal arguments must hash equally");
    assert_ne!(hashes[0], hashes[2], "different arguments must not collide (probed with concrete values)");
}
