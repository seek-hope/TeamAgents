//! Render synthetic team scenes as JSON terminal cells for reproducible visual review. No daemon/model calls.
use ratatui::{backend::TestBackend, style::Color, Terminal};
use serde_json::{json, Value};
use teamagents_tui::{team::View, v2app::V2App, v2ui};

fn color(value: Color) -> Value {
    match value {
        Color::Rgb(r, g, b) => json!([r, g, b]),
        Color::Reset => Value::Null,
        other => json!(format!("{other:?}")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let width = args.get(1).and_then(|value| value.parse().ok()).unwrap_or(150);
    let height = args.get(2).and_then(|value| value.parse().ok()).unwrap_or(38);
    let scene = args.get(3).map(String::as_str).unwrap_or("team");
    let mut app = V2App::new("preview");
    app.session_name = "parser-repair".into();
    app.apply_checkpoint(json!({"environment":{"workspace":"~/projects/parser"},"instances":[
        {"id":"i-leader","model":"deepseek-flash","effort":"high","lifecycle":"ACTIVE","phase":"WAITING",
            "diagnosis":{"code":"waiting","summary":"Waiting for implementation and verification results.","waits":[{"id":"wait-team","mode":"all","timer_at":null,"conditions":[{"task":"verify-parser"}]}]}},
        {"id":"implementer","model":"deepseek-flash","effort":"high","lifecycle":"ACTIVE","phase":"TOOLS_PENDING",
            "diagnosis":{"code":"tools_pending","summary":"Running parser regression tests.","operations":[{"id":"test-parser","status":"RUNNING"}]}},
        {"id":"verifier","model":"deepseek-flash","effort":"max","lifecycle":"PARKED","phase":"READY",
            "diagnosis":{"code":"inactive","attention":true,"summary":"Verification needs attention.","reason":"Required fixture is missing: fixtures/malformed.json","next_steps":"Provide the fixture, then explicitly resume verifier.","work":{"state":"BLOCKED"},"operations":[]}}
    ],"tasks":[{"id":"repair-parser","assignee":"implementer","status":"RUNNING"},
        {"id":"verify-parser","assignee":"verifier","status":"BLOCKED","diagnosis":{"reason":"Missing test fixture"}}],
      "work":{"status":"ACTIVE","known_usage":{"total":18420},"unknown_usage":0,"limits":{"max_total_tokens":100000}}}),10);
    app.apply_history(json!({"entries":[
        {"kind":"user","message":{"content":"Fix malformed version handling and verify that valid versions still work."}},
        {"kind":"assistant","message":{"content":"## Parser repair\n\nI have delegated the implementation and an independent verification pass.\n\n- **implementer** is running the regression suite.\n- **verifier** found a missing fixture and needs attention.\n\nThe work remains open until the checks have a recorded result."}},
        {"kind":"assistant","message":{"tool_calls":[{"id":"delegation","function":{"name":"delegate_task","arguments":"{\"assignee\":\"verifier\",\"description\":\"Check malformed inputs\"}"}}]}},
        {"kind":"tool_result","message":{"tool_call_id":"delegation","content":"Task verify-parser assigned to verifier."}},
        {"kind":"assistant","message":{"content":"The right pane shows the saved blocker. Browsing a member's history keeps this conversation and your draft intact."}}
    ]}));
    app.team.select("verifier");
    app.composer.set_text("The fixture is in tests/data/; check there first.");
    if scene == "history" {
        app.team.history_open = true;
        app.team.member.as_mut().unwrap().apply(&json!({"instance_id":"verifier","entries":[
            {"kind":"assistant","message":{"content":"## Independent verification\n\nThe parser tests reference a fixture that is absent from this checkout.\n\n```text\nfixtures/malformed.json: not found\n```\n\nI have recorded the blocker. I have not marked the task successful."}}
        ]}));
    } else if scene == "tasks" {
        app.team.view = View::Tasks;
        app.team.apply_tasks(&json!({"tasks":[{"id":"inspect-parser","assignee":"i-leader","status":"SUCCEEDED"},
            {"id":"repair-parser","assignee":"implementer","status":"RUNNING"},
            {"id":"verify-parser","assignee":"verifier","status":"BLOCKED","diagnosis":{"reason":"Missing test fixture"}}]}));
    } else if width < 132 {
        app.team.view = View::Inspect;
    }
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| v2ui::render(frame, &mut app)).unwrap();
    let cells=(0..height).map(|y| (0..width).map(|x| {
        let cell=&terminal.backend().buffer()[(x,y)];
        json!({"text":cell.symbol(),"fg":color(cell.fg),"bg":color(cell.bg),"bold":cell.modifier.contains(ratatui::style::Modifier::BOLD),"dim":cell.modifier.contains(ratatui::style::Modifier::DIM),"reversed":cell.modifier.contains(ratatui::style::Modifier::REVERSED)})
    }).collect::<Vec<_>>()).collect::<Vec<_>>();
    println!("{}", json!({"fixture":"synthetic team preview","width":width,"height":height,"cells":cells}));
}
