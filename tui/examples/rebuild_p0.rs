//! Minimal real-terminal client for the R2-P0 independent daemon.
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
        return Err("后台回执过大".into());
    }
    let value: Value = serde_json::from_str(&line)?;
    if value["ok"] != true {
        return Err(format!("后台拒绝：{value}").into());
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
    let socket = Path::new(args.get(1).ok_or("用法：rebuild_p0 DAEMON_SOCKET")?);
    let mut state = request(socket, "status")?;
    enable_raw_mode()?;
    let _restore = Restore;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let mut next_poll = Instant::now();
    let mut notice = "已连接独立后台；退出界面后任务继续。".to_string();
    loop {
        if Instant::now() >= next_poll {
            match request(socket, "status") {
                Ok(value) => state = value,
                Err(error) => notice = format!("连接中断，可重连：{error}"),
            }
            next_poll = Instant::now() + Duration::from_millis(150);
        }
        terminal.draw(|frame|{
            let text=format!("后台任务：{}\n状态：{}\n进度计数：{}\n事件水位：{}\n\n{s}\n\n操作：s 开始  p 暂停  r 恢复  c 取消  q 断开\n\n{notice}",
                state["task_id"].as_str().unwrap_or(""),state["status"].as_str().unwrap_or(""),
                state["ticks"],state["sequence"],s="此入口仅验证 R2-P0，不调用模型。");
            frame.render_widget(Paragraph::new(text).block(Block::default().borders(Borders::ALL).title("TeamAgents R2-P0")),frame.area());
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
                            notice = "控制命令已由后台保存。".into();
                        }
                        Err(error) => notice = format!("控制失败：{error}"),
                    }
                }
            }
        }
    }
    Ok(())
}
