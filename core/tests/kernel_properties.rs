//! 纯函数层的有界穷举（R2 内核：线协议视图、输出裁剪、分页、响应分类）。
//!
//! 这些函数不碰数据库，性质可以直接穷举/枚举；它们对应规格里"视图/线协议"那部分
//! （`pair_tool_results` 的配对 = R22，`page_output` 的坐标 = A20 的原文可追溯）。
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
            model: "deepseek-v4.1-flash".into(),
            instructions: "You are a worker.".into(),
            tools: vec![json!({"type":"function","function":{"name":"shell","parameters":{"type":"object"}}})],
            options: json!({}),
            context_window: Some(1_000_000),
        },
    )
}

/// 消息多重集（全等比较用；测试输入都很小，不会被裁剪改写）
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

/// 视图取值域：线协议视角会遇到的几种条目形态
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

/// `prepare_request` 的线协议不变量：系统提示在最前、其余是输入的置换、
/// 每个有回答的 tool 调用后面紧跟它的回答（R22 配对）、assistant 之间保持原序。
#[test]
fn wire_view_is_a_paired_permutation() {
    let kernel = kernel();
    // 长度 ≤ 3 的全部组合（6 种条目形态）
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
    // 以及几个更长的、专门制造"回答落地在其它条目之后"的用例
    cases.push(vec![alphabet(2), alphabet(4), alphabet(3), alphabet(1), alphabet(0)]);
    cases.push(vec![alphabet(5), alphabet(4), alphabet(0), alphabet(1), alphabet(3), alphabet(2)]);

    // 覆盖断言：必须真的有"回答被搬到调用后面"的用例，否则这条性质只是空转
    let mut moved = 0usize;
    for entries in cases {
        let request = kernel.prepare_request(&entries, "req");
        let messages = &request.messages;
        assert_eq!(messages[0]["role"], json!("system"), "系统提示必须在最前");
        assert_eq!(messages.len(), entries.len() + 1, "除了系统提示不许多出或少掉消息");

        let expected: Vec<Json> = entries.iter().map(|entry| entry.message.clone()).collect();
        assert_eq!(multiset(&expected), multiset(&messages[1..]), "线协议视角必须是输入的置换");

        // 配对：真实日志里"回答落在调用之后"的条目，必须被搬到调用后面紧跟的位置；
        // 回答在调用之前的组合在上下文里不可能出现（运行时先追加调用），只用置换性质覆盖
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
                continue; // 不可实现的历史顺序（回答先于调用）
            }
            let answers = find_answers(&messages[1..], id);
            let call_in_wire = find_call(&messages[1..], id).expect("配对后调用仍在");
            let first = answers.first().copied().expect("配对后回答仍在");
            assert_eq!(call_in_wire + 1, first, "回答 {id} 必须紧跟它的调用（否则严格端点会拒绝）");
            if answer != call + 1 {
                moved += 1;
            }
        }
        // assistant 之间的相对顺序不变
        let assistants = |items: &[Json]| -> Vec<String> {
            items.iter().filter(|m| m["role"] == json!("assistant")).map(|m| m.to_string()).collect()
        };
        assert_eq!(assistants(&expected), assistants(&messages[1..]), "assistant 顺序必须保持");
    }
    assert!(moved >= 10, "配对性质必须真的被触发过（实际搬动了 {moved} 次）");
}

/// 输出裁剪：短内容原样、长内容保留首尾且有界（模型看不到无上限的输出）。
#[test]
fn tool_output_cap_keeps_head_and_tail_within_bounds() {
    for length in [0usize, 1, TOOL_OUTPUT_CAP - 1, TOOL_OUTPUT_CAP, TOOL_OUTPUT_CAP + 1, TOOL_OUTPUT_CAP * 2] {
        // 每个位置都取不同字符，便于定位首尾
        let content: String =
            (0..length).map(|index| char::from_u32(0x4E00 + (index % 2000) as u32).unwrap()).collect();
        let capped = cap_tool_output(&content);
        if length <= TOOL_OUTPUT_CAP {
            assert_eq!(capped, content, "不超上限时不得改写");
            continue;
        }
        let capped_len = capped.chars().count();
        assert!(capped_len <= TOOL_OUTPUT_CAP + 64, "裁剪后的长度必须有界：{capped_len}");
        if length > TOOL_OUTPUT_CAP + 64 {
            assert!(capped_len < length, "远超上限时必须真的变短");
        }
        let half = TOOL_OUTPUT_CAP / 2;
        let head: String = content.chars().take(half).collect();
        let tail: String = content.chars().skip(length - half).collect();
        assert!(capped.starts_with(&head), "必须保留开头");
        assert!(capped.ends_with(&tail), "必须保留结尾");
        assert!(capped.contains("truncated"), "必须标记被截断");
    }
}

/// 分页（readback）：按 limit 逐页取回能无缝重建原文，坐标自洽、越界明确报错。
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
                assert!(text.chars().count() <= limit, "单页不得超过 limit");
                assert_eq!(text, content.chars().skip(offset).take(limit).collect::<String>(), "页内容必须与坐标一致");
                rebuilt.push_str(text);
                pages += 1;
                assert!(pages <= length + 2, "页数必须有限");
                if page["eof"] == json!(true) {
                    assert!(page["next_offset"].is_null(), "eof 时不得有下一页坐标");
                    break;
                }
                let next = page["next_offset"].as_u64().expect("next_offset") as usize;
                assert_eq!(next, offset + text.chars().count(), "next_offset 必须等于已消费长度");
                offset = next;
            }
            assert_eq!(rebuilt, content, "逐页取回必须无缝重建原文");
        }
    }
    // 越界与非法参数明确报错，不静默截断
    assert!(page_output("abc", &json!({"limit": 0})).is_err());
    assert!(page_output("abc", &json!({"limit": 12_001})).is_err());
    assert!(page_output("abc", &json!({"offset": 4})).is_err());
    assert!(page_output("abc", &json!({"offset": 3})).is_ok(), "offset == 长度是合法的空页");
}

/// 响应分类：finish / wait 必须是唯一调用；混用只按注释忽略；空响应是普通回复。
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
    // finish/wait 与别的调用混用：按注释忽略它们，其余照常执行
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

/// 参数散列是确定的：同样的参数永远得到同样的 args_hash（收据与重放都依赖它）。
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
    assert_eq!(hashes[0], hashes[1], "同样的参数必须得到同样的散列");
    assert_ne!(hashes[0], hashes[2], "不同参数不应碰撞（这里用具体值探测）");
}
