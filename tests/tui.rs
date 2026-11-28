#![cfg(target_os = "linux")]
//! End-to-end PTY verification for the interactive TUI (resize hit-testing, Del×2, `--plain`).
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Command, Stdio},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use unicode_width::UnicodeWidthChar;

fn read_daemon_request(conn: &mut UnixStream) -> Value {
    let mut reader = BufReader::new(&*conn);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let hello: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(hello["protocol"], 3);
    assert_eq!(hello["body"], json!({"op":"hello"}));
    let mut welcome = json!({"protocol": 3});
    if hello["query_provider"] == true {
        welcome["provider"] = json!("api");
        welcome["model_settings"] = json!(true);
    }
    writeln!(reader.get_mut(), "{}", json!({"value": welcome})).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    let frame: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["protocol"], 3);
    assert_eq!(frame["workspace"], hello["workspace"]);
    frame["body"].clone()
}

struct Lab(PathBuf);

impl Lab {
    fn new() -> (Self, UnixListener) {
        let root = std::env::temp_dir().join(format!("ta-tui-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("state")).unwrap();
        fs::create_dir(root.join("work")).unwrap();
        let socket = UnixListener::bind(root.join("state/swarm.sock")).unwrap();
        (Self(root), socket)
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        command
            .env_clear()
            .env("TERM", "xterm-256color")
            .env("PATH", "/usr/bin:/bin")
            .args(args)
            .arg("--session")
            .arg(self.0.join("state"))
            .arg("--workspace")
            .arg(self.0.join("work"));
        command
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Screen {
    width: usize,
    height: usize,
    cells: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

impl Screen {
    fn new(width: usize, height: usize) -> Self {
        Self { width, height, cells: vec![vec![' '; width]; height], row: 0, col: 0 }
    }

    fn clear_all(&mut self) {
        for r in &mut self.cells {
            r.fill(' ');
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => match chars.peek().copied() {
                    Some('[') => {
                        chars.next();
                        let mut params = String::new();
                        let mut final_ch = None;
                        for next in chars.by_ref() {
                            if ('@'..='~').contains(&next) {
                                final_ch = Some(next);
                                break;
                            }
                            params.push(next);
                        }
                        if let Some(cmd) = final_ch {
                            self.csi(&params, cmd);
                        }
                    }
                    Some(']') => {
                        chars.next();
                        while let Some(next) = chars.next() {
                            if next == '\x07' || (next == '\x1b' && chars.peek() == Some(&'\\')) {
                                if next == '\x1b' {
                                    chars.next();
                                }
                                break;
                            }
                        }
                    }
                    Some(_) => {
                        chars.next();
                    }
                    None => {}
                },
                '\r' => self.col = 0,
                '\n' => self.row = (self.row + 1).min(self.height.saturating_sub(1)),
                c if c.is_control() => {}
                ch => {
                    let w = ch.width().unwrap_or(0);
                    if w == 0 {
                        continue;
                    }
                    if self.col < self.width && self.row < self.height {
                        self.cells[self.row][self.col] = ch;
                        if w == 2 && self.col + 1 < self.width {
                            self.cells[self.row][self.col + 1] = '\0';
                        }
                        self.col = (self.col + w).min(self.width);
                    }
                }
            }
        }
    }

    fn csi(&mut self, params: &str, cmd: char) {
        if let Some(private) = params.strip_prefix('?') {
            if private == "1049" && cmd == 'h' {
                self.clear_all();
                self.row = 0;
                self.col = 0;
            }
            return;
        }
        let nums: Vec<usize> = if params.is_empty() {
            Vec::new()
        } else {
            params.split(';').map(|p| p.parse::<usize>().unwrap_or(0)).collect()
        };
        let p = |idx: usize, default: usize| nums.get(idx).copied().filter(|&v| v > 0).unwrap_or(default);
        match cmd {
            'H' | 'f' => {
                self.row = p(0, 1).saturating_sub(1).min(self.height.saturating_sub(1));
                self.col = p(1, 1).saturating_sub(1).min(self.width.saturating_sub(1));
            }
            'A' => self.row = self.row.saturating_sub(p(0, 1)),
            'B' => self.row = (self.row + p(0, 1)).min(self.height.saturating_sub(1)),
            'C' => self.col = (self.col + p(0, 1)).min(self.width),
            'D' => self.col = self.col.saturating_sub(p(0, 1)),
            'G' => self.col = p(0, 1).saturating_sub(1).min(self.width.saturating_sub(1)),
            'J' => match nums.first().copied().unwrap_or(0) {
                2 => self.clear_all(),
                0 => {
                    for c in self.col..self.width {
                        self.cells[self.row][c] = ' ';
                    }
                    for r in (self.row + 1)..self.height {
                        self.cells[r].fill(' ');
                    }
                }
                _ => {}
            },
            'K' => match nums.first().copied().unwrap_or(0) {
                0 => {
                    for c in self.col..self.width {
                        self.cells[self.row][c] = ' ';
                    }
                }
                1 => {
                    for c in 0..=self.col.min(self.width.saturating_sub(1)) {
                        self.cells[self.row][c] = ' ';
                    }
                }
                2 => self.cells[self.row].fill(' '),
                _ => {}
            },
            _ => {}
        }
    }

    fn text(&self) -> String {
        let mut out = String::new();
        for r in &self.cells {
            for &ch in r {
                if ch != '\0' {
                    out.push(ch);
                }
            }
            out.push('\n');
        }
        out
    }

    /// Return 0-based `(col, row)` screen cell where ASCII substring `needle` appears.
    fn find_cell(&self, needle: &str) -> Option<(u16, u16)> {
        let chars: Vec<char> = needle.chars().collect();
        if chars.is_empty() {
            return None;
        }
        for (r, row_cells) in self.cells.iter().enumerate() {
            for c in 0..row_cells.len() {
                if row_cells[c] == '\0' {
                    continue;
                }
                let mut matched = true;
                let mut cur = c;
                for &nc in &chars {
                    while cur < row_cells.len() && row_cells[cur] == '\0' {
                        cur += 1;
                    }
                    if cur >= row_cells.len() || row_cells[cur] != nc {
                        matched = false;
                        break;
                    }
                    cur += 1;
                }
                if matched {
                    return Some((c as u16, r as u16));
                }
            }
        }
        None
    }
}

fn open_pty() -> (fs::File, OwnedFd) {
    let mut master = -1;
    let mut slave = -1;
    let ws = libc::winsize { ws_row: 32, ws_col: 112, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: `master`, `slave`, and `ws` are valid pointers for `openpty`.
    let rc = unsafe { libc::openpty(&mut master, &mut slave, ptr::null_mut(), ptr::null_mut(), &ws) };
    assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());
    // SAFETY: `openpty` returned newly opened valid descriptors.
    unsafe {
        libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC);
        libc::fcntl(slave, libc::F_SETFD, libc::FD_CLOEXEC);
        (fs::File::from_raw_fd(master), OwnedFd::from_raw_fd(slave))
    }
}

fn wait_for_text(
    buf: &(Mutex<Vec<u8>>, Condvar),
    offset: usize,
    w: usize,
    h: usize,
    needle: &str,
    timeout: Duration,
) -> Screen {
    let deadline = Instant::now() + timeout;
    let mut guard = buf.0.lock().unwrap();
    loop {
        let mut screen = Screen::new(w, h);
        screen.feed(&guard[offset.min(guard.len())..]);
        let text = screen.text();
        if text.contains(needle) {
            return screen;
        }
        let rem = deadline.saturating_duration_since(Instant::now());
        assert!(!rem.is_zero(), "timed out waiting for {needle:?} in {w}x{h} screen:\n{text}");
        let (next, _) = buf.1.wait_timeout(guard, rem).unwrap();
        guard = next;
    }
}

fn send_sgr_click(master: &mut fs::File, col: u16, row: u16) {
    // SGR mouse press + release use 1-based terminal cell coordinates.
    let seq = format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", col + 1, row + 1, col + 1, row + 1);
    master.write_all(seq.as_bytes()).unwrap();
    master.flush().unwrap();
}

struct StopGuard(Arc<AtomicBool>);
impl Drop for StopGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[test]
fn tui_submits_cjk_approves_resizes_clicks_deletes_and_exits_cleanly() {
    let (lab, socket) = Lab::new();
    socket.set_nonblocking(true).unwrap();
    let stop_peer = Arc::new(AtomicBool::new(false));
    let _stop_guard = StopGuard(stop_peer.clone());
    let peer_stop = stop_peer.clone();
    let peer = thread::spawn(move || {
        let old_root = json!({
            "id": "old-1", "root": "old-1", "agent": "default", "status": "done",
            "stage_status": "done", "kind": "root", "prompt": "Old History",
            "result": "Old output", "continuation_node": "old-1",
            "continuation_agent": "default", "continuation_kind": "leaf",
            "turns_used": 1, "turn_budget": 32
        });
        let mut tasks = json!([old_root.clone()]);
        let mut whiteboard: Vec<Value> = Vec::new();
        let mut wb_seq: u64 = 0;
        let mut approved = false;
        let mut continued = false;
        let mut seen_wb_write = false;
        let mut seen_status_done = false;
        let mut deleted = false;
        while !peer_stop.load(Ordering::Relaxed) {
            let mut conn = match socket.accept() {
                Ok((conn, _)) => conn,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => panic!("accept failed: {e}"),
            };
            conn.set_nonblocking(false).unwrap();
            let body = read_daemon_request(&mut conn);
            let op = body["op"].as_str().unwrap();
            let reply = match op {
                "tasks" => tasks.clone(),
                "members" => json!([{"name": "default"}]),
                "model" => json!({"provider": "api", "model": "fixture-model", "effort": "high"}),
                "submit" => {
                    assert_eq!(body["text"], "修复 parser");
                    assert_eq!(body["agent"], "default");
                    tasks = json!([
                        old_root.clone(),
                        {
                            "id": "r1", "root": "r1", "agent": "default", "status": "waiting",
                            "stage_status": "done", "kind": "root", "prompt": "修复 parser",
                            "turns_used": 1, "turn_budget": 32, "policy": "review"
                        },
                        {
                            "id": "c1", "root": "r1", "parent": "r1", "agent": "worker",
                            "status": "approval", "approval_id": "appr-7", "kind": "node",
                            "action": {"op": "edit_file", "path": "src/lib.rs", "old": "bug", "new": "fix"}
                        }
                    ]);
                    json!({"id": "r1"})
                }
                "continue" => {
                    assert_eq!(body["id"], "c1");
                    assert_eq!(body["text"], "追加测试");
                    continued = true;
                    tasks = json!([
                        old_root.clone(),
                        {
                            "id": "r1", "root": "r1", "agent": "default", "status": "done",
                            "stage_status": "done", "kind": "root", "prompt": "修复 parser",
                            "result": "Round 2 tests added.", "round": 2,
                            "continuation_node": "c1", "continuation_agent": "worker",
                            "continuation_kind": "leaf", "turns_used": 1, "turn_budget": 32, "sandbox": "direct"
                        }
                    ]);
                    json!({"id": "r1", "node": "c1"})
                }
                "wb_write" => {
                    assert_eq!(body["id"], "r1");
                    assert_eq!(body["key"], "ui:note");
                    assert_eq!(body["mode"], "set");
                    assert_eq!(body["content"], "hello_board");
                    wb_seq += 1;
                    seen_wb_write = true;
                    let entry = json!({
                        "key": "ui:note",
                        "kind": body.get("kind").and_then(Value::as_str).unwrap_or("note"),
                        "content": "hello_board",
                        "author_agent": "user",
                        "round": 2,
                        "version": 1,
                        "seq": wb_seq,
                        "status": "active"
                    });
                    whiteboard = vec![entry.clone()];
                    json!({"whiteboard": true, "op": "wb_write", "ok": true, "mode": "set", "key": "ui:note", "seq": wb_seq, "entry": entry})
                }
                "status" => {
                    let id = body["id"].as_str().unwrap();
                    if id == "r1" && continued && !deleted {
                        json!({
                            "id": "r1", "root": "r1", "agent": "default", "status": "done",
                            "stage_status": "done", "kind": "root", "prompt": "修复 parser",
                            "result": "Round 2 tests added.", "round": 2, "turns_used": 1, "turn_budget": 32,
                            "continuation_node": "c1", "continuation_agent": "worker",
                            "continuation_kind": "leaf",
                            "sandbox": "direct",
                            "whiteboard_seq": wb_seq,
                            "whiteboard": whiteboard.clone(),
                            "rounds": [
                                {"round": 1, "subtree_root": "r1", "prompt": "修复 parser", "outcome": "done", "result": "All 12 tests passed."},
                                {"round": 2, "subtree_root": "c1", "prompt": "追加测试", "outcome": "done", "result": "Round 2 tests added."}
                            ],
                            "evidence": [], "leaf_results": [], "subtree_receipts": []
                        })
                    } else if id == "r1" && approved && !deleted {
                        seen_status_done = true;
                        json!({
                            "id": "r1", "root": "r1", "agent": "default", "status": "done",
                            "stage_status": "done", "kind": "root", "prompt": "修复 parser",
                            "result": "All 12 tests passed.", "turns_used": 3, "turn_budget": 32,
                            "policy": "review", "verification": "not_requested",
                            "continuation_node": "c1", "continuation_agent": "worker",
                            "continuation_kind": "leaf",
                            "evidence": [], "leaf_results": [], "subtree_receipts": []
                        })
                    } else if id == "c1" {
                        json!({
                            "id": "c1", "root": "r1", "parent": "r1", "agent": "worker",
                            "status": if approved { "done" } else { "approval" },
                            "approval_id": if approved { Value::Null } else { json!("appr-7") },
                            "action": if approved { Value::Null } else { json!({"op": "edit_file", "path": "src/lib.rs", "old": "bug", "new": "fix"}) }
                        })
                    } else {
                        tasks
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|t| t["id"] == id)
                            .cloned()
                            .unwrap_or(json!({"id": id, "status": "waiting"}))
                    }
                }
                "approve" => {
                    assert_eq!(body["id"], "appr-7");
                    approved = true;
                    tasks = json!([
                        old_root.clone(),
                        {
                            "id": "r1", "root": "r1", "agent": "default", "status": "done",
                            "stage_status": "done", "kind": "root", "prompt": "修复 parser",
                            "continuation_node": "c1", "continuation_agent": "worker",
                            "continuation_kind": "leaf",
                            "turns_used": 3, "turn_budget": 32
                        }
                    ]);
                    json!({"ok": true})
                }
                "delete" => {
                    assert_eq!(body["id"], "r1");
                    deleted = true;
                    tasks = json!([old_root.clone()]);
                    json!({"ok": true, "root": "r1"})
                }
                other => panic!("unexpected op: {other}"),
            };
            writeln!(conn, "{}", json!({"value": reply})).unwrap();
        }
        assert!(
            approved && continued && seen_wb_write && seen_status_done && deleted,
            "expected approval, continue, wb_write, status done, and delete"
        );
    });

    let (mut master, slave) = open_pty();
    let slave_in = slave.try_clone().unwrap();
    let slave_out = slave;
    let mut cmd = lab.cmd(&["tui"]);
    cmd.stdin(Stdio::from(slave_in)).stdout(Stdio::from(slave_out)).stderr(Stdio::piped());
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY, 0);
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    drop(cmd);

    let output = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let out_reader = output.clone();
    let mut master_reader = master.try_clone().unwrap();
    let stop_reader = stop_peer.clone();
    let reader_handle = thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        let fd = master_reader.as_raw_fd();
        while !stop_reader.load(Ordering::Relaxed) {
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let rc = unsafe { libc::poll(&mut pfd, 1, 50) };
            if rc < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if rc == 0 {
                continue;
            }
            match master_reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut guard = out_reader.0.lock().unwrap();
                    guard.extend_from_slice(&chunk[..n]);
                    out_reader.1.notify_all();
                }
            }
        }
    });

    let timeout = Duration::from_secs(8);
    // Even after the initial snapshot loads a settled session ("Old History"), TUI stays in New Session.
    let initial = wait_for_text(&output, 0, 112, 32, "Old History", timeout);
    assert!(initial.text().contains("New session"));
    assert!(initial.text().contains("fixture-model"));

    master.write_all("修复 parser\r".as_bytes()).unwrap();
    master.flush().unwrap();
    wait_for_text(&output, 0, 112, 32, "approval(s) waiting", timeout);

    // Open approval modal (Ctrl+P) and approve with 'a'.
    master.write_all(&[0x10]).unwrap();
    master.flush().unwrap();
    wait_for_text(&output, 0, 112, 32, "Pending Approvals", timeout);
    wait_for_text(&output, 0, 112, 32, "+ fix", timeout);
    master.write_all(b"a").unwrap();
    master.flush().unwrap();
    wait_for_text(&output, 0, 112, 32, "Continue @worker", timeout);

    // Sending follow-up input in settled session routes directly to the sole leaf (`c1`).
    master.write_all("追加测试\r".as_bytes()).unwrap();
    master.flush().unwrap();
    wait_for_text(&output, 0, 112, 32, "Round 2 tests added.", timeout);

    // Toggle /staged on and edit/inspect collaborative whiteboard via /wb.
    let after_cont_screen = output.0.lock().unwrap().len();
    master.write_all("/staged on\r".as_bytes()).unwrap();
    master.flush().unwrap();
    wait_for_text(&output, after_cont_screen, 112, 32, "· staged", timeout);
    master.write_all("/wb set ui:note hello_board\r".as_bytes()).unwrap();
    master.flush().unwrap();
    wait_for_text(&output, after_cont_screen, 112, 32, "hello_board", timeout);
    // Switch back to Summary tab (Alt+1) before returning to New session via Ctrl+N.
    master.write_all(b"\x1b1").unwrap();
    master.flush().unwrap();
    wait_for_text(&output, after_cont_screen, 112, 32, "Round 2 tests added.", timeout);

    // Ctrl+N switches back to a blank New Session, then clicking "修复 parser" re-selects r1.
    let after_cont = output.0.lock().unwrap().len();
    master.write_all(&[0x0e]).unwrap();
    master.flush().unwrap();
    let blank_again = wait_for_text(&output, after_cont, 112, 32, "New session", timeout);
    let (r1_col, r1_row) = blank_again.find_cell("修复").expect("r1 visible in sidebar");
    send_sgr_click(&mut master, r1_col + 1, r1_row);
    wait_for_text(&output, after_cont, 112, 32, "Round 2 tests added.", timeout);
    // Focus Inspector (Tab from Sidebar) so narrow viewport renders Inspector + "☰ Sessions" pill.
    master.write_all(b"\t").unwrap();
    master.flush().unwrap();

    // Resize PTY across the 96-col responsive breakpoint and verify SGR click on exact rendered tab coordinates.
    let offset = output.0.lock().unwrap().len();
    let narrow_ws = libc::winsize { ws_row: 26, ws_col: 90, ws_xpixel: 0, ws_ypixel: 0 };
    assert_eq!(unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &narrow_ws) }, 0);
    wait_for_text(&output, offset, 90, 26, "☰ Sessions", timeout);
    let resized_screen = wait_for_text(&output, offset, 90, 26, "5 Board", timeout);
    let (ev_col, ev_row) = resized_screen.find_cell("Evidence").expect("Evidence tab visible after resize");
    send_sgr_click(&mut master, ev_col + 1, ev_row);
    wait_for_text(&output, offset, 90, 26, "Runtime-observed execution receipts", timeout);

    // Click "Sessions" toggle to focus sidebar, then Del×2 to arm and delete r1 (returns to New session).
    let (sess_col, sess_row) = resized_screen.find_cell("Sessions").expect("Sessions toggle visible");
    send_sgr_click(&mut master, sess_col + 1, sess_row);
    wait_for_text(&output, offset, 90, 26, "Agents (1)", timeout);
    master.write_all(b"\x1b[3~").unwrap();
    master.flush().unwrap();
    wait_for_text(&output, offset, 90, 26, "Press Del again", timeout);
    master.write_all(b"\x1b[3~").unwrap();
    master.flush().unwrap();
    wait_for_text(&output, offset, 90, 26, "Deleted session r1", timeout);

    // Exit cleanly with Ctrl+Q.
    master.write_all(&[0x11]).unwrap();
    master.flush().unwrap();

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "TUI did not exit on Ctrl+Q");
        thread::sleep(Duration::from_millis(15));
    };
    stop_peer.store(true, Ordering::Relaxed);
    drop(master);
    reader_handle.join().unwrap();
    peer.join().unwrap();

    assert!(status.success(), "TUI exited with {status}");
    let raw = output.0.lock().unwrap();
    let raw_str = String::from_utf8_lossy(&raw);
    assert!(raw_str.contains("\x1b[?1049h"), "expected alternate screen entry");
    assert!(raw_str.contains("\x1b[?1049l"), "expected clean alternate screen exit");
}

#[test]
fn bare_resume_honors_explicit_wait_options_on_a_tty() {
    for flags in [&["--jsonl"][..], &["--timeout", "0"], &["--detach"]] {
        let (lab, socket) = Lab::new();
        socket.set_nonblocking(true).unwrap();
        let (mut master, slave) = open_pty();
        let mut child = lab
            .cmd(&["resume"])
            .args(flags)
            .stdin(slave.try_clone().unwrap())
            .stdout(slave)
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let root = json!({"id":"r1", "root":"r1", "agent":"default", "status":"done", "result":"finished"});
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut requests = ["tasks", "status"].into_iter().take(if flags == ["--jsonl"] { 2 } else { 1 });
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("resume {flags:?} did not exit on a TTY");
            }
            if requests.len() == 0 {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            match socket.accept() {
                Ok((mut conn, _)) => {
                    let body = read_daemon_request(&mut conn);
                    assert_eq!(body["op"], requests.next().unwrap());
                    let value = if body["op"] == "tasks" {
                        json!([root])
                    } else {
                        assert_eq!(body["id"], "r1");
                        root.clone()
                    };
                    writeln!(conn, "{}", json!({"value":value})).unwrap();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(10)),
                Err(e) => panic!("accept failed: {e}"),
            }
        }
        let out = child.wait_with_output().unwrap();
        let mut bytes = Vec::new();
        // Linux PTY masters return EIO when the last slave descriptor closes.
        if let Err(e) = master.read_to_end(&mut bytes) {
            assert_eq!(e.raw_os_error(), Some(libc::EIO));
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains('\x1b'), "unexpected terminal control sequence: {text:?}");
        if flags == ["--timeout", "0"] {
            assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("timed out waiting for r1"));
        } else {
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            let rows: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
            if flags == ["--jsonl"] {
                assert_eq!(rows.first().unwrap()["type"], "submitted");
                assert_eq!(rows.last().unwrap()["type"], "completed");
            } else {
                assert_eq!(rows, [json!({"id":"r1"})]);
            }
        }
    }
}

#[test]
fn tui_requires_tty_and_chat_plain_works_without_tty() {
    let (lab, socket) = Lab::new();
    let no_tty = lab.cmd(&["tui"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().unwrap();
    assert!(!no_tty.status.success());
    assert!(String::from_utf8_lossy(&no_tty.stderr).contains("interactive terminal"));

    socket.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let _stop_guard = StopGuard(stop.clone());
    let peer_stop = stop.clone();
    let peer = thread::spawn(move || {
        let mut seen = Vec::new();
        while seen.len() < 2 && !peer_stop.load(Ordering::Relaxed) {
            let mut conn = match socket.accept() {
                Ok((conn, _)) => conn,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => panic!("accept failed: {e}"),
            };
            conn.set_nonblocking(false).unwrap();
            let body = read_daemon_request(&mut conn);
            let op = body["op"].as_str().unwrap().to_owned();
            let reply = if op == "tasks" { json!([]) } else { json!([{"name": "default"}]) };
            writeln!(conn, "{}", json!({"value": reply})).unwrap();
            seen.push(op);
        }
        seen
    });
    let mut chat = lab
        .cmd(&["chat", "--plain"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(chat.stdin.take().unwrap(), "/members\n/quit").unwrap();
    let out = chat.wait_with_output().unwrap();
    stop.store(true, Ordering::Relaxed);
    let ops = peer.join().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(ops, vec!["tasks", "members"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "default");
}
