//! Minimal real-terminal client for the isolated protocol probe.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::{
    backend::CrosstermBackend,
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use serde_json::{json, Value};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn request(socket: &Path, method: &str) -> Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_millis(200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(200)))?;
    let id = format!("tui-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos());
    serde_json::to_writer(&mut stream, &json!({"version":1,"command_id":id,"method":method}))?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream.take(65_537)).read_line(&mut line)?;
    if line.len() > 65_536 {
        return Err("daemon reply too large".into());
    }
    let value: Value = serde_json::from_str(&line)?;
    if value["ok"] != true {
        return Err(format!("daemon refused: {value}").into());
    }
    Ok(value)
}

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let socket = Path::new(args.get(1).ok_or("usage: probe DAEMON_SOCKET")?);
    let mut state = request(socket, "status")?;
    enable_raw_mode()?;
    let _restore = Restore;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let mut next_poll = Instant::now();
    let mut notice = "attached to the standalone daemon; work continues after you quit.".to_string();
    loop {
        if Instant::now() >= next_poll {
            match request(socket, "status") {
                Ok(value) => state = value,
                Err(error) => notice = format!("connection lost, reconnect available: {error}"),
            }
            next_poll = Instant::now() + Duration::from_millis(150);
        }
        terminal.draw(|frame|{
            let text=format!("daemon work: {}\nstatus: {}\nticks: {}\nevent watermark: {}\n\n{s}\n\nkeys: s start  p pause  r resume  c cancel  q detach\n\n{notice}",
                state["task_id"].as_str().unwrap_or(""),state["status"].as_str().unwrap_or(""),
                state["ticks"],state["sequence"],s="this entry point only verifies the protocol and terminal behaviour; it never calls a model.");
            frame.render_widget(Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("TeamAgents probe")),frame.area());
        })?;
        if event::poll(Duration::from_millis(20))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let method = match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('s') => Some("start"),
                    KeyCode::Char('p') => Some("pause"),
                    KeyCode::Char('r') => Some("resume"),
                    KeyCode::Char('c') => Some("cancel"),
                    _ => None,
                };
                if let Some(method) = method {
                    match request(socket, method) {
                        Ok(value) => {
                            state = value;
                            notice = "the daemon stored the control command.".into();
                        }
                        Err(error) => notice = format!("control failed: {error}"),
                    }
                }
            }
        }
    }
    Ok(())
}
