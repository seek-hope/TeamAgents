//! D-73: the front-end refuses the flags it cannot honour. The session's
//! workspace and mode belong to the daemon the engine boots, so a flag here used
//! to be accepted and quietly ignored; the entry point now says what to use
//! instead. (The flags it *does* support still parse: with no terminal the TUI
//! says so and stops, which is how these runs also prove the parse succeeded.)

use std::process::Command;

#[test]
fn the_tui_refuses_the_flags_the_daemon_owns() {
    let bin = env!("CARGO_BIN_EXE_teamagents-tui");
    for (flag, needle) in [
        ("--cwd", "--cwd is not a TUI flag"),
        ("--full-auto", "--full-auto is not a TUI flag"),
        ("--resume", "is no longer supported"),
        ("--team", "is no longer supported"),
    ] {
        let out = Command::new(bin).arg(flag).arg("x").output().expect("run teamagents-tui");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{flag}: usage is exit 2, got {out:?}");
        assert!(stderr.contains(needle), "{flag}: {stderr}");
        assert!(stderr.contains("usage: teamagents-tui --daemon"), "{flag}: {stderr}");
    }
    // a supported invocation parses: with no terminal it stops with the pointer
    // to `exec` instead of a usage error
    let out = Command::new(bin).args(["--daemon", "/nonexistent/daemon.sock"]).output().expect("run teamagents-tui");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stderr.contains("needs a real terminal"), "{stderr}");
}
