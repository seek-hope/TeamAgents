//! CLI smoke: init / doctor / version and the v2 state root.

use std::process::Command;

/// A scratch tree one test owns, removed on **every** exit path including a panic — the `Daemon` guard's
/// sibling (D-160).
///
/// A test that panics skips its final `remove_dir_all`, so a failing run in a CI condition (where some tests
/// panic by design: `make check-nobwrap`) left state roots behind for `make test`'s leak guard to report,
/// turning one failure into two (measured twice, D-171 and D-174). Tests still remove their tree explicitly at
/// the end; this is the path they cannot reach.
struct Scratch(std::path::PathBuf);

impl Scratch {
    /// A fresh, empty directory named after `label` and the test process.
    fn new(label: &str) -> Scratch {
        let path = std::env::temp_dir().join(format!("ta-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        Scratch(path)
    }
}

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::path::Path> for Scratch {
    fn as_ref(&self) -> &std::path::Path {
        &self.0
    }
}

impl AsRef<std::ffi::OsStr> for Scratch {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.0.as_os_str()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A daemon the test started, stopped on every exit path including a panic:
/// a leaked daemon would keep running (and hold a coordinator lock) for the
/// rest of the suite.
struct Daemon(std::process::Child);

impl Daemon {
    /// Stop the daemon and wait for it, now rather than at the end of the scope.
    ///
    /// A test that removes its state root has to let the daemon go *first*: a live daemon recreates the
    /// directories it uses, so a removal that raced it left the root behind — `make test`'s leak guard caught
    /// exactly that (`/tmp/ta-checks-<pid>/root/instances/i-leader`, D-160). Idempotent: calling it again from
    /// `Drop` ignores the already-reaped child.
    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Stop the daemon serving `state`, by pid, and report whether one was found.
///
/// A test that only ran a *client* has no `Child` handle: `exec` detaches the daemon on purpose. Finding it
/// means reading `/proc` for the process whose argument list says `daemon … --state-root <state>` and that is
/// not a corpse — never `pkill -f`, whose pattern also matches any other command line that merely mentions the
/// string (D-144/D-148 measured that killing two shells of a session). SIGTERM is the graceful stop the daemon
/// handles (D-150), so the socket goes away with it.
///
/// Returning means *stopped*, not *signalled*: D-160 learned that for `Daemon` (`.kill()` **and** `.wait()`),
/// and the same race came back through this path — a detached daemon still shutting down recreated
/// `/tmp/ta-tui-knob-<pid>/root/instances/i-leader` after the test had removed its tree, and the leak guard
/// reported the reappeared directory (D-186). The wait is bounded and ends when the process is gone or becomes
/// a zombie, which this container's pid 1 never reaps.
fn stop_detached_daemon(state: &std::path::Path) -> bool {
    let root = state.to_string_lossy().into_owned();
    let mut found = false;
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let argv: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect();
        if argv.get(1).map(String::as_str) != Some("daemon") || !argv.contains(&root) {
            continue;
        }
        if !daemon_running(pid) {
            continue; // a zombie keeps the binary's name in this container (pid 1 does not reap)
        }
        found = true;
        let _ = Command::new("kill").arg(pid.to_string()).status();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while daemon_running(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    found
}

/// Whether `pid` still runs: `/proc/<pid>/stat`'s state field, with `Z` (a zombie) counting as gone and a
/// missing entry meaning the process has exited.
fn daemon_running(pid: u32) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else { return false };
    // the state field follows the parenthesised command name, which may itself contain spaces
    stat.rsplit_once(')')
        .map(|(_, rest)| rest.trim_start().chars().next().is_some_and(|field| field != 'Z'))
        .unwrap_or(false)
}

fn teamagents(args: &[&str], state_home: &std::path::Path, config_home: &std::path::Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(args)
        .env("XDG_STATE_HOME", state_home)
        .env("XDG_CONFIG_HOME", config_home)
        .output()
        .expect("run cli");
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

/// D-228: the two inversions of D-166's mistake — a *directory* where the session database or the socket
/// belongs. Measured 2026-09-27: `init` answered SQLite's `unable to open database file` (the path three times,
/// no fix), `exec` answered `Connection refused … start teamagents daemon first` (a diagnosis pointing at the
/// daemon) and `doctor` called that root `[ok]`. All three now name the shape and the fix.
#[test]
fn a_directory_where_the_database_or_socket_belongs_is_refused_with_the_shape_named() {
    let home = Scratch::new("pathkind");
    let config_home = home.join("config");
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_PATHKIND_KEY\"\n\
         base_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let run = |state_home: &std::path::Path, args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .env("XDG_STATE_HOME", state_home)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("TA_PATHKIND_KEY", "test-value")
            .output()
            .expect("run cli");
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.code(), text)
    };

    // the session database as a directory
    let db_root = home.join("db");
    std::fs::create_dir_all(db_root.join("teamagents/v2/session.sqlite")).unwrap();
    let (code, text) = run(&db_root, &["init"]);
    assert_ne!(code, Some(0), "{text}");
    assert!(text.contains("is a directory, but that path is the session database file"), "{text}");
    let (code, text) = run(&db_root, &["doctor"]);
    assert_ne!(code, Some(0), "doctor must fail a root nothing can use: {text}");
    assert!(text.contains("[FAIL] state root paths"), "{text}");
    assert!(!text.contains("unable to open database file"), "SQLite's raw message is replaced: {text}");

    // the socket as a directory
    let sock_root = home.join("sock");
    std::fs::create_dir_all(sock_root.join("teamagents/v2/daemon.sock")).unwrap();
    let (code, text) = run(&sock_root, &["exec", "--timeout", "5", "hi"]);
    assert_eq!(code, Some(2), "{text}");
    assert!(text.contains("is a directory, but that path is the daemon's socket"), "{text}");
    assert!(!text.contains("start teamagents daemon first"), "the old advice pointed at the daemon: {text}");
}

/// D-231: `TEAMAGENTS_TUI` naming a file that is not there is refused with the variable and the path in the
/// message. Measured 2026-09-27: the value went straight to `Command::new` and the answer was the OS's
/// `cannot start the TUI: No such file or directory (os error 2)` — worse than the help a user gets when the
/// variable is unset, because it named neither the variable nor the path it tried.
#[test]
fn a_teamagents_tui_that_names_nothing_is_refused_with_the_path() {
    let home = Scratch::new("tuipath");
    let missing = home.join("no-such-tui");
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .env("TEAMAGENTS_TUI", &missing)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run cli");
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("TEAMAGENTS_TUI names"), "{stderr}");
    assert!(stderr.contains(&missing.display().to_string()), "the message names the path: {stderr}");
    // and nothing was started: no daemon, no state root written
    assert!(!home.join("state/teamagents").exists(), "the refusal happens before anything starts");
}

/// D-230: every rejected argument says *why*. Measured 2026-09-27: `teamagents --nonsense` and
/// `teamagents --timeout abc hi` printed the whole help and never the offending word, so the user had to diff
/// their command against the usage; and `--state-root ""` (the unset-variable trap) was accepted and silently
/// used the current directory — it created `session.sqlite` in it.
#[test]
fn a_rejected_argument_says_which_one_and_why() {
    let home = Scratch::new("argreason");
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("TA_ARG_KEY", "test-value")
            .output()
            .expect("run cli");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    for (args, expected) in [
        (vec!["--nonsense"], "--nonsense: no entry point this build serves accepts that argument"),
        (vec!["--state-root", ""], "--state-root needs a path"),
        (vec!["--state-root", "/tmp/a", "--state-root", "/tmp/b", "init"], "--state-root was given twice"),
        (vec!["--model"], "--model needs a value"),
        (vec!["exec", "--timeout", "abc", "hi"], "--timeout needs a whole number of seconds"),
        (vec!["exec", "--timeout", "0", "hi"], "--timeout needs a whole number of seconds"),
        (vec!["exec", "hi", "there"], "this entry point takes one positional argument"),
        (vec!["init", "--cwd", "."], "init takes no other arguments"),
    ] {
        let (code, stderr) = run(&args);
        assert_eq!(code, Some(2), "{args:?} must be a usage error: {stderr}");
        assert!(stderr.contains(expected), "{args:?} must say {expected:?}, got: {stderr}");
    }
    // nothing ran: an empty --state-root no longer leaves a database in the caller's directory
    let (_, _) = run(&["--state-root", "", "init"]);
    assert!(!home.join("session.sqlite").exists(), "an empty --state-root must not initialize the cwd");
}

/// D-227: a state root deep enough that `daemon.sock` crosses Linux's `sun_path` limit cannot hold a session.
/// Measured 2026-09-27: `init` printed the socket path as if it were usable and `doctor` reported the state root
/// `[ok]`, so the first run was where the user met it — as the daemon's raw `bind …: path must be shorter than
/// SUN_LEN`. Both entry points now refuse it in the shell's own words, before anything starts.
#[test]
fn a_socket_path_past_the_kernel_limit_is_refused_before_anything_starts() {
    let home = Scratch::new("socklen");
    // `sun_path` is 108 bytes including the NUL, so 107 bind and 108 do not; this root is well past it
    let deep = home.join("a".repeat(60)).join("b".repeat(60));
    std::fs::create_dir_all(&deep).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .env("XDG_STATE_HOME", &deep)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .output()
            .expect("run cli")
    };
    let init = run(&["init"]);
    let text = String::from_utf8_lossy(&init.stderr).into_owned();
    assert_ne!(init.status.code(), Some(0), "init must not claim a root it cannot use: {text}");
    assert!(text.contains("`sun_path` limit"), "{text}");
    assert!(text.contains("point XDG_STATE_HOME"), "the refusal names the fix: {text}");
    let doctor = run(&["doctor"]);
    let text = String::from_utf8_lossy(&doctor.stdout).into_owned();
    assert!(text.contains("[FAIL] daemon socket"), "doctor must name the socket, not only the root: {text}");
}

/// D-245: the retention row says which of the two keys is applied — `history_days` is (a session boot sweeps
/// ordinary history under the guards `verification/tla/V2Retention.tla` pins) and `archived_days` is not (this
/// build keeps one session per state root, A33, so there is no archived-session set to walk) — and it names the
/// state root's `EVIDENCE` marker when the user has asked for nothing to be pruned there.
#[test]
fn doctor_reports_which_retention_keys_apply() {
    let home = Scratch::new("retention");
    let (config_home, root) = (home.join("config"), home.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let model = "[models.m]\nprovider = 'openai'\nmodel = 'x'\n";
    let run = |text: &str| {
        std::fs::write(config_home.join("teamagents/config.toml"), text).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(["doctor", "--state-root"])
            .arg(&root)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", home.join("state"))
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let text = run(&format!("[retention]\nhistory_days = 7\narchived_days = 30\n{model}"));
    assert!(text.contains("[WARN] retention"), "{text}");
    assert!(text.contains("drops events and applied deliveries"), "the applied key says what it does: {text}");
    assert!(text.contains("archived_days=30 is not applied"), "the unapplied key says so: {text}");
    std::fs::write(root.join("EVIDENCE"), "").unwrap();
    let text = run(&format!("[retention]\nhistory_days = 7\n{model}"));
    assert!(text.contains("[ok  ] retention"), "a marked root is the user's wish, honoured: {text}");
    assert!(text.contains("EVIDENCE"), "the marker is named: {text}");
    std::fs::remove_file(root.join("EVIDENCE")).unwrap();
    let text = run(&format!("[retention]\nhistory_days = 0\narchived_days = 30\n{model}"));
    assert!(text.contains("keeps the full history"), "{text}");
}

/// D-241: a state root that cannot be *created* names the flag and the fix, from every entry point that creates
/// one. They used to answer three different ways, none of them naming `--state-root`: measured 2026-09-27 with
/// the root under a symlink loop, `init` said `could not prepare the state root: create … (os error 40)`,
/// `daemon` said `state root: … (os error 40)` and `exec` said `cannot create … (os error 40)` — while
/// `doctor`'s WARN sent the user back to `init`, the command that had just failed. The same run found the
/// sibling shape D-228 had not covered: a *directory* where the daemon's log goes, which `exec` answered with a
/// bare `cannot open …/daemon.log: Is a directory (os error 21)`.
///
/// D-273: the scratch config below is this test's own and names a variable the test sets
/// (`TA_UNCREATABLE_KEY`), because the child's outcome must not depend on what the operator's shell exports
/// (AGENTS.md's rule for this crate's integration tests). Without it the `daemon` case read the *starter*
/// config `init` had just written — which names `DEEPSEEK_API_KEY` — and `daemon` builds its provider (the
/// credential included) before it reports the state root, so on a shell with no credential it answered
/// `missing API key env DEEPSEEK_API_KEY` instead of the state-root sentence this test is about, which is the
/// shape `.github/workflows/ci.yml` runs in. The assertions are unchanged: every entry point still refuses the
/// root, names the reason and names the flag.
#[test]
fn an_uncreatable_state_root_names_the_flag_from_every_entry_point() {
    let home = Scratch::new("uncreatable");
    std::fs::create_dir_all(&*home).unwrap();
    // a self-referential symlink: every process, root included, gets ELOOP from creating anything under it
    std::os::unix::fs::symlink("loop", home.join("loop")).unwrap();
    let root = home.join("loop/root");
    let config_home = home.join("config");
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_UNCREATABLE_KEY\"\n\
         base_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .arg("--state-root")
            .arg(&root)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CONFIG_HOME", &config_home)
            .env("TA_UNCREATABLE_KEY", "test-value")
            .output()
            .expect("run cli");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    for verb in [&["init"][..], &["daemon"][..], &["exec", "hi"][..]] {
        let (code, stderr) = run(verb);
        assert_ne!(code, Some(0), "{verb:?} must not claim a root it cannot use: {stderr}");
        assert!(stderr.contains("cannot be created"), "{verb:?}: the reason is stated: {stderr}");
        assert!(stderr.contains("point --state-root (or XDG_STATE_HOME)"), "{verb:?}: the fix is named: {stderr}");
    }
    // `doctor` reports the root as not initialized and sends the user to `init`; that advice has to lead to a
    // message that names the flag (the loop D-166 closed for the file case)
    let doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(["doctor", "--state-root"])
        .arg(&root)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", &config_home)
        .env("TA_UNCREATABLE_KEY", "test-value")
        .output()
        .expect("run doctor");
    let text = String::from_utf8_lossy(&doctor.stdout).into_owned();
    assert!(text.contains("[WARN] v2 state root"), "{text}");
    assert!(text.contains("teamagents init"), "the warning names the lever: {text}");

    // D-241's sibling: a directory where the daemon's log goes is refused by the kind check (D-228's rule), the
    // same way a directory named `session.sqlite` or `daemon.sock` is
    let log_root = home.join("logdir");
    std::fs::create_dir_all(log_root.join("daemon.log")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(["exec", "--state-root"])
        .arg(&log_root)
        .arg("hi")
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", &config_home)
        .env("TA_UNCREATABLE_KEY", "test-value")
        .output()
        .expect("run cli");
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_ne!(output.status.code(), Some(0), "{text}");
    assert!(text.contains("daemon.log is a directory"), "the shape is named: {text}");
    assert!(text.contains("point --state-root/XDG_STATE_HOME"), "the fix is named: {text}");
}

/// D-243: the client creates `<state root>/daemon.log` so a detached daemon has somewhere to complain, and a
/// `daemon.log` that exists but cannot be appended to answered the OS: measured 2026-09-27 with a mode-`000`
/// file, `exec` printed `cannot open …/daemon.log: Permission denied (os error 13)` — naming neither the
/// file's role nor a lever, while a run that once went through `sudo` leaves exactly that file behind.
#[test]
fn an_unopenable_daemon_log_names_the_file_and_the_lever() {
    let home = Scratch::new("logfile");
    let root = home.join("root");
    std::fs::create_dir_all(&root).unwrap();
    let log = root.join("daemon.log");
    std::fs::write(&log, "").unwrap();
    let mut perms = std::fs::metadata(&log).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o000);
    std::fs::set_permissions(&log, perms).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(["exec", "--state-root"])
        .arg(&root)
        .arg("hi")
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .output()
        .expect("run cli");
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_ne!(output.status.code(), Some(0), "the client must not start a daemon it cannot log: {text}");
    assert!(text.contains(&log.display().to_string()), "the log file is named: {text}");
    assert!(text.contains("point --state-root") || text.contains("make it writable"), "the fix is named: {text}");
}

/// D-242: the *config* directory that cannot be created names the variable to fix, the way the state root does
/// (D-241). `init` is its only creator and it answered the OS: measured 2026-09-27 with `XDG_CONFIG_HOME` under
/// a symlink loop, `init failed: cannot create …/teamagents: Too many levels of symbolic links (os error 40)` —
/// neither the variable nor a next step, while every other config-path failure in this family names its lever.
#[test]
fn an_uncreatable_config_directory_names_the_variable() {
    let home = Scratch::new("cfgdir");
    std::fs::create_dir_all(&*home).unwrap();
    std::os::unix::fs::symlink("loop", home.join("loop")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .arg("init")
        .arg("--state-root")
        .arg(home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("loop/config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .output()
        .expect("run cli");
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_ne!(output.status.code(), Some(0), "init must not claim a config it could not write: {text}");
    assert!(text.contains("cannot be created"), "the reason is stated: {text}");
    assert!(text.contains("point XDG_CONFIG_HOME (or HOME)"), "the lever is named: {text}");
}

/// D-73: the entry point refuses what it does not honour, and refuses it
/// *before* starting anything. A bare word used to fall through to the TUI — a
/// typo'd verb or a pasted prompt silently booted a session and was dropped —
/// and `-v` was accepted with nothing behind it.
#[test]
fn a_bare_word_and_verbose_are_refused_without_starting_a_session() {
    let home = Scratch::new("refuse");
    let (config_home, state_home, state_root) = (home.join("config"), home.join("state"), home.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_REFUSE_KEY\"\n\
         base_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .arg("--state-root")
            .arg(&state_root)
            .env("XDG_STATE_HOME", &state_home)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("TA_REFUSE_KEY", "test-value")
            .output()
            .expect("run cli");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).into_owned())
    };
    for word in ["hello", "frobnicate"] {
        let (code, stderr) = run(&[word]);
        assert_eq!(code, Some(2), "{word}: {stderr}");
        assert!(stderr.contains(word) && stderr.contains("is not an entry point"), "{word}: {stderr}");
        assert!(stderr.contains("teamagents exec"), "the message points at the headless entry: {stderr}");
    }
    let (code, stderr) = run(&["-v"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains("-v/--verbose is not supported") && stderr.contains("--version"), "{stderr}");
    // An entry point no release serves is refused *by name*, even when a flag of the removed subcommand
    // follows it: the word is the problem, and the flag must not be parsed into a field nothing reads
    // (D-136).
    for word in ["sessions", "validate", "serve"] {
        for args in [vec![word], vec![word, "--dry-run"], vec![word, "--history-days", "30"]] {
            let (code, stderr) = run(&args);
            assert_eq!(code, Some(2), "{args:?}: {stderr}");
            assert!(
                stderr.contains(&format!("teamagents {word}: this entry point is no longer supported")),
                "{args:?}: the message must name the entry point, not the flag: {stderr}"
            );
            assert!(stderr.contains("teamagents exec"), "{args:?}: {stderr}");
        }
    }
    // refusing happens before the session: no socket, no daemon, nothing written
    assert!(!state_root.join("daemon.sock").exists(), "a refused argument must not start a session");
    let _ = std::fs::remove_dir_all(&home);
}

/// finding 10/11: doctor runs real probes (bwrap isolation, hook programs)
/// and reports a malformed config instead of silently defaulting it.
#[test]
fn doctor_probes_isolation_and_config_errors() {
    let home = Scratch::new("doctor");
    let config = home.join("config/teamagents");
    std::fs::create_dir_all(&config).unwrap();
    let run_status = |state: &std::path::Path| -> (i32, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("doctor")
            .env("XDG_STATE_HOME", state)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .output()
            .expect("run doctor");
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.code().unwrap_or(-1), text)
    };
    let run = |state: &std::path::Path| -> String { run_status(state).1 };
    let clean = run(&home.join("state"));
    assert!(clean.contains("user config"), "{clean}");
    assert!(clean.contains("bubblewrap isolation"), "{clean}");
    if teamagents_engine::tools::sandbox_usable() {
        assert!(clean.contains("[ok  ] bubblewrap isolation"), "the isolation probe really runs: {clean}");
    }

    // hooks fail silently at event time, so doctor checks the programs
    std::fs::write(
        config.join("config.toml"),
        "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n\n[hooks]\nnotify = [\"/nonexistent/notify.sh\"]\npre_tool = [\"/bin/sh\", \"-c\", \"exit 0\"]\n\n[retention]\narchived_days = 30\n",
    )
    .unwrap();
    let with_hooks = run(&home.join("state"));
    assert!(with_hooks.contains("[FAIL] hooks.notify"), "{with_hooks}");
    assert!(with_hooks.contains("[ok  ] hooks.pre_tool"), "{with_hooks}");
    // D-75/D-245: retention stays user-config-only, `history_days` is applied and `archived_days` is not, so
    // the row reports each key for what it is instead of printing the numbers as if both were in effect
    assert!(with_hooks.contains("[WARN] retention"), "the policy is reported honestly: {with_hooks}");
    assert!(with_hooks.contains("archived_days=30 is not applied"), "{with_hooks}");

    // D-74: a declared MCP service is bound at session start, so doctor names each
    // one and says whether it can run (a mistyped command would stop the boot)
    std::fs::write(
        config.join("config.toml"),
        "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n\n\
         [tools.good]\nkind = \"mcp\"\ncommand = \"/bin/sh\"\n\n\
         [tools.typo]\nkind = \"mcp\"\ncommand = \"/nonexistent/mcp\"\n\n\
         [tools.remote]\nkind = \"mcp\"\nmcp_transport = \"http\"\nurl = \"https://example.invalid/mcp\"\n",
    )
    .unwrap();
    let with_tools = run(&home.join("state"));
    assert!(with_tools.contains("[ok  ] tools.good"), "{with_tools}");
    assert!(with_tools.contains("[WARN] tools.typo"), "a command that cannot run is reported: {with_tools}");
    assert!(with_tools.contains("not runnable"), "{with_tools}");
    assert!(with_tools.contains("[ok  ] tools.remote"), "{with_tools}");

    // D-78: the web half of the same section is reported too — each declared binding
    // and whether its credential is there (the executor resolves these lazily, so a
    // missing key otherwise shows up only in a tool receipt)
    std::fs::write(
        config.join("config.toml"),
        "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n\n\
         [tools.search]\nkind = \"web_search\"\nprovider = \"anysearch\"\n\
         url = \"https://api.anysearch.com/v1/search\"\napi_key_env = \"TA_DOCTOR_WEB_KEY\"\n",
    )
    .unwrap();
    let without_key = run(&home.join("state"));
    assert!(without_key.contains("[WARN] tools.search"), "{without_key}");
    assert!(without_key.contains("TA_DOCTOR_WEB_KEY is unset"), "{without_key}");
    assert!(without_key.contains("capability state"), "the row says what a call will answer: {without_key}");
    let with_key = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .arg("doctor")
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("TA_DOCTOR_WEB_KEY", "test-value")
        .output()
        .expect("run doctor");
    let with_key = String::from_utf8_lossy(&with_key.stdout).into_owned();
    assert!(with_key.contains("[ok  ] tools.search"), "{with_key}");
    assert!(with_key.contains("credential TA_DOCTOR_WEB_KEY is set"), "{with_key}");

    // D-167: DESIGN §7 calls a missing web-search credential a capability state — and the tool now answers
    // exactly that instead of sending an unauthenticated request. `web_fetch` is the half that needs no
    // credential, and a *required* binding that cannot work refuses the member's start, so its row is a FAIL
    // (D-164's rule, on the web half of the section).
    std::fs::write(
        config.join("config.toml"),
        "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n\n\
         [tools.fetch]\nkind = \"web_fetch\"\n\n\
         [tools.nokey]\nkind = \"web_search\"\nprovider = \"anysearch\"\n\n\
         [tools.needkey]\nkind = \"web_search\"\nprovider = \"anysearch\"\n\
         api_key_env = \"TA_DOCTOR_REQUIRED_KEY\"\nrequired = true\n",
    )
    .unwrap();
    let (code, web) = run_status(&home.join("state"));
    assert_eq!(code, 1, "a required binding that cannot work fails doctor: {web}");
    assert!(web.contains("[FAIL] tools.needkey"), "{web}");
    assert!(web.contains("TA_DOCTOR_REQUIRED_KEY is unset"), "{web}");
    assert!(web.contains("cannot start a member"), "the row says what the boot does: {web}");
    assert!(web.contains("[WARN] tools.nokey"), "no credential at all is a capability state too: {web}");
    assert!(web.contains("no `api_key_env` is configured"), "{web}");
    assert!(web.contains("[ok  ] tools.fetch") && web.contains("needs no credential"), "{web}");

    // D-79: a config that declares no web binding at all says so too — the model is then
    // offered neither web_search nor web_fetch, which the README's feature list would
    // otherwise imply is there by default
    std::fs::write(config.join("config.toml"), "[models.m]\nprovider=\"openai\"\nmodel=\"x\"\n").unwrap();
    let without_web = run(&home.join("state"));
    assert!(without_web.contains("[WARN] web tools"), "{without_web}");
    assert!(without_web.contains("offered neither web_search nor web_fetch"), "{without_web}");

    // a wrong type in [permissions] is an error, not a silent default
    std::fs::write(config.join("config.toml"), "[permissions]\ntrust_project = \"yes\"\n").unwrap();
    let broken = run(&home.join("state"));
    assert!(broken.contains("[FAIL] user config"), "{broken}");
    assert!(broken.contains("must be true/false"), "{broken}");
    let _ = std::fs::remove_dir_all(&home);
}

/// Plant a skills directory the doctor can count, and check both the resolving
/// and the missing case: a configured root that is not there is otherwise silent
/// (`skill` answers "no skills configured" only when the model asks).
#[test]
fn doctor_reports_the_skills_registry_and_missing_configured_paths() {
    let root = Scratch::new("skills");
    let (config, state, skills) = (root.join("config"), root.join("state"), root.join("skills"));
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    // one skill: a directory whose SKILL.md names it
    std::fs::create_dir_all(skills.join("reviewer")).unwrap();
    std::fs::write(skills.join("reviewer/SKILL.md"), "---\nname: reviewer\ndescription: reviews\n---\nbody\n").unwrap();
    std::fs::write(root.join("house-rules.md"), "be careful\n").unwrap();
    let run = |skills_path: &str, instructions: &str| -> String {
        std::fs::write(
            config.join("teamagents/config.toml"),
            format!(
                "skills_paths = [{skills_path}]\ninstruction_files = [{instructions}]\n\n\
                 [models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_SKILLS_KEY\"\n"
            ),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("doctor")
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", &state)
            .env("TA_SKILLS_KEY", "test-value")
            .output()
            .expect("run doctor");
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    };
    let good = run(&format!("\"{}\"", skills.display()), &format!("\"{}\"", root.join("house-rules.md").display()));
    assert!(good.contains("[ok  ] skills"), "a resolving root is reported ok: {good}");
    assert!(good.contains("1 skill(s) under 1 configured root(s)"), "{good}");
    // D-102/D-246: the promise is delivered now, so a resolving file is reported *ok* with what a session will
    // actually hand a prompt — the flip this row was built to make visible
    assert!(good.contains("[ok  ] instruction files"), "{good}");
    assert!(good.contains("reach every member's prompt") && good.contains("byte(s)"), "{good}");
    // a root that does not exist is a warning that names it, instead of a skill
    // list that silently stays empty
    let missing = run("\"/nonexistent/skills\"", "\"/nonexistent/rules.md\"");
    assert!(missing.contains("[WARN] skills"), "{missing}");
    assert!(missing.contains("/nonexistent/skills") && missing.contains("never load"), "{missing}");
    // D-168: and what the model gets from those roots — the `skill` tool stays offered and can only answer the
    // capability state, so the row names it instead of leaving the user to guess
    assert!(missing.contains("`skill` tool stays offered") && missing.contains("no skills configured"), "{missing}");
    assert!(
        missing.contains("[WARN] instruction files")
            && missing.contains("/nonexistent/rules.md")
            && missing.contains("cannot read:"),
        "{missing}"
    );
    // and no configured root at all says where to put one
    let none = run("", "");
    assert!(none.contains("[WARN] skills") && none.contains("skills_paths"), "{none}");
    assert!(none.contains("`skill` tool is still offered"), "{none}");
    assert!(!none.contains("instruction files"), "no row without configured files: {none}");
    let _ = std::fs::remove_dir_all(&root);
}

/// D-164: `doctor`'s MCP rows have to predict the boot (`engine/src/bound.rs`). A `required = true` service
/// that cannot load refuses the *driver* boot — the instance parks and no member ever runs — while an optional
/// one only drops that capability. Both were WARN rows under doctor's "WARN marks optional capabilities"
/// footer with exit 0, so a user was told the session was fine while nothing drove it. And a `command`
/// carrying `${VAR}` was reported `ok` with a promise no code keeps ("resolves an environment reference at
/// start"): nothing expands a command, so the string reaches `exec(2)` literally and the service never starts.
#[test]
fn doctor_predicts_whether_an_mcp_service_can_start() {
    let root = Scratch::new("mcp-doctor");
    let (config, state) = (root.join("config"), root.join("state"));
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    let model = "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_MCP_KEY\"\n";
    let run = |tools: &str| -> (i32, String) {
        std::fs::write(config.join("teamagents/config.toml"), format!("{model}{tools}")).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("doctor")
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", &state)
            .env("TA_MCP_KEY", "test-value")
            .output()
            .expect("run doctor");
        (
            output.status.code().unwrap_or(-1),
            format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)),
        )
    };
    let binding = |name: &str, body: &str| format!("\n[tools.{name}]\nkind = \"mcp\"\n{body}\n");

    // a required service whose command does not exist: the boot refuses, so the row fails and so does doctor
    let (code, out) = run(&binding("broken", "command = \"/nonexistent/mcp-server\"\nrequired = true\n"));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("[FAIL] tools.broken") && out.contains("not runnable"), "{out}");
    assert!(out.contains("the instance parks"), "the row says what the boot does: {out}");

    // the same service optional: the session boots without that capability — a WARN row, and doctor's verdict is
    // decided by the other rows (its exit code is 1 wherever the *isolation* probe fails, which is the documented
    // A14 behaviour without bubblewrap — `make check-nobwrap` runs this suite in exactly that condition)
    let (code, out) = run(&binding("broken", "command = \"/nonexistent/mcp-server\"\n"));
    assert!(out.contains("[WARN] tools.broken"), "{out}");
    assert!(out.contains("capability is dropped") && out.contains("still boots"), "{out}");
    assert!(code == 0 || out.contains("[FAIL] bubblewrap isolation"), "{code} without an isolation failure: {out}");

    // `${VAR}` in a command: expanded by nothing, so it can never start (it used to be reported `ok`)
    let (code, out) = run(&binding("ref", "command = \"${HOME}/bin/mcp-server\"\nrequired = true\n"));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("nothing expands") && out.contains("in a command"), "{out}");

    // an env value naming an unset variable is a hard error in `bound.rs`, so the row must not be green
    let (code, out) = run(&binding(
        "env",
        "command = \"/bin/sh\"\nargs = [\"-c\", \"true\"]\nenv = { TOKEN = \"${TA_MCP_STDIO_DEFINITELY_UNSET}\" }\n\
         required = true\n",
    ));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("TA_MCP_STDIO_DEFINITELY_UNSET") && out.contains("unset"), "{out}");

    // control: a service that is really there stays ok, required or not
    let (code, out) = run(&binding("sh", "command = \"/bin/sh\"\nrequired = true\n"));
    assert!(out.contains("[ok  ] tools.sh") && out.contains("is runnable"), "{out}");
    // doctor's exit code belongs to all its rows: without bubblewrap the isolation probe fails by design (A14)
    assert!(code == 0 || out.contains("[FAIL] bubblewrap isolation"), "{code} without an isolation failure: {out}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Keep diagnostics deterministic on machines without user namespaces.
#[test]
fn doctor_fresh_install_reports_the_missing_requirements() {
    let root = Scratch::new("doctor-fresh");
    let bin = root.join("bin");
    let config = root.join("config/teamagents/config.toml");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    // An empty PATH makes bwrap consistently absent.
    let run = |key: &str| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .arg("doctor")
            .env("PATH", &bin)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_DOCTOR_TEST_KEY", key)
            .output()
            .unwrap();
        (output.status.success(), String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let (ok, text) = run("test-value");
    assert!(!ok && text.contains("[FAIL] user config"), "{text}");
    assert!(text.contains(&config.display().to_string()), "{text}");
    std::fs::write(
        &config,
        "[models.leader_main]\nprovider='openai'\nmodel='test'\napi_key_env='TA_DOCTOR_TEST_KEY'\n",
    )
    .unwrap();
    let (ok, text) = run("test-value");
    assert!(!ok, "missing required bubblewrap must fail doctor: {text}");
    let failures: Vec<_> = text.lines().filter(|line| line.contains("[FAIL]")).collect();
    assert_eq!(failures.len(), 1, "only missing bubblewrap must fail doctor: {text}");
    assert!(failures[0].contains("bubblewrap isolation"), "{text}");
    for key in ["", "   "] {
        let (ok, text) = run(key);
        assert!(!ok && text.contains("[FAIL] model profile leader_main"), "{text}");
        assert!(text.contains("TA_DOCTOR_TEST_KEY"), "{text}");
    }
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    let (ok, text) = run("test-value");
    assert!(!ok && text.contains("cannot read config"), "{text}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn init_creates_private_config_and_never_overwrites_existing_paths() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = Scratch::new("init");
    let config = root.join("config with spaces/teamagents/config.toml");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .env("XDG_CONFIG_HOME", root.join("config with spaces"))
            .env("XDG_STATE_HOME", root.join("state"))
            .output()
            .unwrap()
    };
    assert!(!run(&["init", "--cwd", "/tmp"]).status.success());
    assert!(!config.exists());
    let output = run(&["init"]);
    assert!(output.status.success(), "{output:?}");
    // init prepares only the *v2* state root (R27/A36); it never opens a v1
    // session, so no legacy state directory appears
    assert!(!root.join("state/teamagents/sessions").exists(), "init must not open a legacy session");
    assert!(root.join("state/teamagents/v2/session.sqlite").is_file(), "init must prepare the v2 root");
    let text = std::fs::read_to_string(&config).unwrap();
    let catalog = teamagents_engine::config::parse_user_config(&text).unwrap();
    assert_eq!(catalog.models["leader_main"].context_window, Some(1_000_000));
    assert_eq!(catalog.models["leader_main"].api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
    assert_eq!(std::fs::metadata(&config).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::write(&config, "# existing user content\n").unwrap();
    let output = run(&["init"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("not overwritten"));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), "# existing user content\n");
    std::fs::remove_file(&config).unwrap();
    let target = root.join("missing-target");
    symlink(&target, &config).unwrap();
    assert!(run(&["init"]).status.success());
    assert!(!target.exists(), "a dangling symlink must not be followed");
    assert!(std::fs::symlink_metadata(&config).unwrap().is_symlink());
    std::fs::remove_file(&config).unwrap();
    std::fs::create_dir(&config).unwrap();
    assert!(!run(&["init"]).status.success());
    std::fs::remove_dir_all(root).unwrap();
}

/// D-174: `doctor` reports the artifact footprint of a state root, because DESIGN §4.4 requires artifact
/// collection and the user cannot act on growth they cannot see; D-191 implemented the collection half (a
/// driver's boot claims unreferenced artifacts, deletes their bytes and collects their rows), so the row
/// now states what that does and that a schedule beyond boot does not exist yet.
#[test]
fn doctor_reports_the_artifact_footprint() {
    let root = Scratch::new("artifacts-doctor");
    let (config, state, tree) = (root.join("config"), root.join("state"), root.join("tree"));
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    std::fs::write(
        config.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_ART_KEY\"\n",
    )
    .unwrap();
    let run = |state_root: &std::path::Path| -> String {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(["doctor", "--state-root"])
            .arg(state_root)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", &state)
            .env("TA_ART_KEY", "test-value")
            .output()
            .expect("run doctor");
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    };

    // nothing on disk yet: no row (a fresh root has nothing to report)
    let empty = run(&tree.join("empty"));
    assert!(!empty.contains("[WARN] artifacts"), "{empty}");

    // one member's artifacts, the layout the driver creates (D-174): the row names the count and the size
    let artifacts = tree.join("used/instances/i-leader/artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    std::fs::write(artifacts.join("resp-a1.json"), "x".repeat(1500)).unwrap();
    std::fs::write(artifacts.join("exec-1.log"), "y".repeat(500)).unwrap();
    let used = run(&tree.join("used"));
    assert!(used.contains("[WARN] artifacts") && used.contains("2 file(s)"), "{used}");
    assert!(used.contains("collected when a driver boots"), "the row says what collection does: {used}");
    let _ = std::fs::remove_dir_all(&root);
}

/// D-181: `TEAMAGENTS_TUI` is the documented way to point the engine at the front-end (`docs/DEVELOPMENT.md`'s
/// knob table: "where `teamagents` finds `teamagents-tui`; its own error message names this variable"), and
/// nothing exercised its *effect* — only the message that names it. The test gives the engine a recorder and
/// asserts what it was launched with; the control leaves the knob unset, where the discovery falls back to the
/// repository's own TUI, which refuses a non-terminal run (that refusal is how the fallback shows up in a test).
#[test]
fn the_tui_knob_decides_which_front_end_the_engine_launches() {
    use std::os::unix::fs::PermissionsExt;
    let root = Scratch::new("tui-knob");
    let (config, state, out) = (root.join("config"), root.join("root"), root.join("argv"));
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    std::fs::write(
        config.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_TUI_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let recorder = root.join("fake-tui.sh");
    std::fs::write(&recorder, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", out.display())).unwrap();
    std::fs::set_permissions(&recorder, std::fs::Permissions::from_mode(0o755)).unwrap();
    let run = |tui: Option<&std::path::Path>| -> std::process::Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        command
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_TUI_KEY", "test-value")
            .env("TEAMAGENTS_TUI", tui.unwrap_or(std::path::Path::new("")))
            .args(["--state-root"])
            .arg(&state)
            .output()
            .expect("run teamagents")
    };

    // the knob decides: the recorder sees the daemon socket the engine started for it
    let output = run(Some(&recorder));
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let argv = std::fs::read_to_string(&out).unwrap_or_default();
    assert!(argv.contains("--daemon"), "the front-end is launched with the socket: {argv:?}");
    assert!(
        argv.contains(&state.join("daemon.sock").to_string_lossy().to_string()),
        "and the socket is this session's: {argv:?}"
    );
    assert!(state.join("daemon.sock").exists(), "the engine booted the session first");
    assert!(stop_detached_daemon(&state), "the daemon this test started is stopped by pid, not by pattern");

    // control: with the knob unset the discovery falls back to the repository's own TUI, which refuses to run
    // without a terminal — the observable difference between "the knob was used" and "the default was"
    let output = run(None);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("needs a real terminal") || stderr.contains("teamagents-tui not found"),
        "the fallback discovery ran and reached a real TUI (or found none): {stderr}"
    );
    // the control run boots a session when the repository's TUI was found; when discovery failed before
    // booting there is nothing to stop, so the answer is deliberately not asserted here (the leak guard in
    // `make test` is the backstop, D-147)
    stop_detached_daemon(&state);
}

/// D-169: `init` creates a fresh root at whatever path it is given (D-149), so a path *named like the session
/// database* used to produce a directory called `session.sqlite` with a database inside it — a layout whose own
/// `doctor`/`daemon` then fail on. The note names the shape; it never refuses, because a fresh root at any name
/// is what the user asked for. (D-166 is the neighbouring case: a `--state-root` that already *is* a file.)
#[test]
fn init_notes_a_root_named_like_the_session_database() {
    let root = Scratch::new("init-shape");
    std::fs::create_dir_all(root.join("config/teamagents")).unwrap();
    let run = |args: &[&str]| -> (bool, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .output()
            .unwrap();
        (
            output.status.success(),
            format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)),
        )
    };
    let (ok, out) = run(&["init", "--state-root", root.join("data/session.sqlite").to_str().unwrap()]);
    assert!(ok, "{out}");
    assert!(out.contains("looks like a session database"), "the note names the shape: {out}");
    assert!(out.contains("pass its parent directory"), "and the way out: {out}");
    assert!(
        root.join("data/session.sqlite/session.sqlite").is_file(),
        "the fresh root is still created, exactly as asked (D-149)"
    );
    // control: an ordinary fresh directory gets no note
    let (ok, out) = run(&["init", "--state-root", root.join("plain").to_str().unwrap()]);
    assert!(ok, "{out}");
    assert!(!out.contains("looks like a session database"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A36/R27: `init` prepares an identifiable v2 state root and `doctor` verifies
/// it; a foreign database in the same path is refused rather than reinterpreted
/// (A34), and the legacy layout is only reported.
#[test]
fn init_prepares_the_v2_root_and_doctor_verifies_it() {
    let home = Scratch::new("cli-v2");
    let config = home.join("config");
    std::fs::create_dir_all(config.join("teamagents")).unwrap();
    std::fs::write(
        config.join("teamagents/config.toml"),
        "[models.leader_main]
provider = \"deepseek\"\nprotocol = \"deepseek\"\nmodel = \"deepseek-flash\"\napi_key_env = \"DEEPSEEK_API_KEY\"\ncontext_window = 1000000\n",
    )
    .unwrap();

    let init = teamagents(&["init"], &home, &config);
    assert!(init.contains("state root ready"), "{init}");
    let db = home.join("teamagents/v2/session.sqlite");
    assert!(db.is_file(), "session database missing: {init}");
    let doctor = teamagents(&["doctor"], &home, &config);
    assert!(doctor.contains("v2 state root"), "{doctor}");
    assert!(doctor.contains("journal_mode=wal"), "{doctor}");
    assert!(doctor.contains("synchronous=FULL"), "{doctor}");
    // D-183: the same row states the SQLite this build links, because DESIGN §4.4 makes its version (the
    // WAL-reset fix) part of the durability guarantee; the predicate itself is asserted in core's own test.
    assert!(doctor.contains(&format!("sqlite={}", rusqlite::version())), "{doctor}");
    // D-149: the check that names the state directory must not create the *legacy* one. `doctor`'s writability
    // probe used to run on `teamagents/sessions/`, so running `doctor` before `init` (which the guide invites:
    // "config, credentials, state root, …") made the next `init` tell the user to remove "an older release's
    // sessions directory" that this build had just made.
    assert!(!home.join("teamagents/sessions").exists(), "doctor must not create the legacy sessions layout: {doctor}");
    let rerun = teamagents(&["init"], &home, &config);
    assert!(!rerun.contains("older release"), "init must not report a legacy layout this build made: {rerun}");

    // an explicit --state-root is honoured and reported
    let other = home.join("other-root");
    let custom = teamagents(&["init", "--state-root", other.to_str().unwrap()], &home, &config);
    assert!(custom.contains(other.to_str().unwrap()), "{custom}");

    // a foreign database under the v2 path is refused (A34)
    let foreign = home.join("foreign");
    std::fs::create_dir_all(&foreign).unwrap();
    rusqlite::Connection::open(foreign.join("session.sqlite"))
        .unwrap()
        .execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); INSERT INTO meta VALUES ('format_id','other-store'),('schema_version','1');")
        .unwrap();
    let refused = teamagents(&["init", "--state-root", foreign.to_str().unwrap()], &home, &config);
    assert!(refused.contains("refusing to reinterpret") || refused.contains("init failed"), "{refused}");

    // the legacy layout is reported (never touched) when it exists
    std::fs::create_dir_all(home.join("teamagents/sessions/old")).unwrap();
    let doctor = teamagents(&["doctor"], &home, &config);
    assert!(doctor.contains("an older release's sessions directory"), "{doctor}");
    // …and an *empty* one is not evidence of an older release (D-149): it holds nothing to migrate, and an
    // empty directory there is what this build's own probe used to leave behind.
    std::fs::remove_dir_all(home.join("teamagents/sessions")).unwrap();
    std::fs::create_dir_all(home.join("teamagents/sessions")).unwrap();
    let doctor = teamagents(&["doctor"], &home, &config);
    assert!(!doctor.contains("older release"), "an empty sessions directory is not a legacy layout: {doctor}");
    std::fs::remove_dir_all(&home).unwrap();
}

/// The documented headless surface end to end through the real binary: plain
/// mode is not a usage error, `-` takes the prompt from stdin, and `--check`
/// really runs the user's acceptance command (it used to be parsed and then
/// dropped) and leaves the verification ledger behind.
#[test]
fn exec_takes_the_prompt_from_stdin_and_runs_the_acceptance_check() {
    use std::io::Write;
    let root = Scratch::new("exec");
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    // a profile that resolves without a network: the turn fails fast against a
    // closed port, which is what makes this test cheap and offline
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_EXEC_TEST_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\ntimeout = 5\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_EXEC_TEST_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(socket.exists(), "the daemon must listen before exec runs");

    let mut exec = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut exec);
    let output = exec
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--timeout", "60", "--check", "echo accepted", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(b"instruction from stdin\n")?;
            child.wait_with_output()
        })
        .expect("run exec");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!stdout.contains("usage:") && !stderr.contains("usage:"), "plain mode is legal: {stdout}{stderr}");
    // the endpoint is closed, so the turn fails and the run is an honest 1
    assert_eq!(output.status.code(), Some(1), "{stdout}{stderr}");
    assert!(stdout.contains("verification:"), "{stdout}");
    let ledger: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("verification.json")).unwrap()).unwrap();
    assert_eq!(ledger["verification"][0]["command"], "echo accepted");
    if teamagents_engine::tools::sandbox_usable() {
        assert!(stdout.contains("check 1: ok (exit 0)  echo accepted"), "{stdout}");
        assert_eq!(ledger["verification"][0]["ok"], true);
    } else {
        // No sandbox on this machine (the GitHub runner, D-113): the same run must report the check as failed
        // *closed* — never as a pass, and never by running the command on the host.
        assert!(stdout.contains("check 1: FAILED (exit -1)  echo accepted"), "{stdout}");
        assert_eq!(ledger["verification"][0]["ok"], false);
        assert_eq!(ledger["verification"][0]["exit_code"], -1);
        let error = ledger["verification"][0]["error"].as_str().unwrap_or("");
        assert!(error.contains("IsolationUnavailable"), "the refusal names the isolation: {ledger}");
    }
    // the piped prompt really reached the leader's context (§4.2 accept boundary)
    let mut client = teamagents_engine::v2::exec::Client::connect(&socket).expect("client");
    let history = client.history("i-leader").expect("history");
    assert!(
        history.iter().any(|entry| entry["message"]["content"] == "instruction from stdin"),
        "the stdin prompt is what was submitted: {history:?}"
    );
    drop(client);
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// The acceptance checks a user writes in `[[checks]]` really reach the goal the
/// runtime boots: the daemon stores them in the goal's `limits_json`, which is
/// exactly what the completion boundary reads (the gate itself is covered by
/// `v2_driver::configured_checks_gate_the_goal_through_the_config_edge`).
#[test]
fn the_daemon_carries_configured_checks_into_the_goal() {
    let root = Scratch::new("checks");
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_CHECKS_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\n\
         [[checks]]\nid = \"tests\"\ncommand = \"cargo test --offline\"\ntimeout = 600\ninputs = [\"src\"]\n\n\
         [[checks]]\nid = \"docs\"\ncommand = \"test -s README.md\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_CHECKS_KEY", "test-value");
    };

    // doctor reports what will gate every goal in this session
    let mut doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut doctor);
    let doctor = doctor.arg("doctor").output().expect("run doctor");
    let text = format!("{}{}", String::from_utf8_lossy(&doctor.stdout), String::from_utf8_lossy(&doctor.stderr));
    assert!(text.contains("2 configured and run at the completion boundary: tests, docs"), "{text}");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    // the goal is created at bootstrap: read the stored limits until it lands
    let db = state.join("session.sqlite");
    let mut limits = String::new();
    for _ in 0..200 {
        if let Ok(control) = teamagents_core::v2::Control::open(&db, "test", false) {
            if let Ok(stored) = control
                .connection()
                .query_row("SELECT limits_json FROM goals LIMIT 1", [], |row| row.get::<_, String>(0))
            {
                limits = stored;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let limits: serde_json::Value = serde_json::from_str(&limits).expect("the goal carries limits");
    let checks = limits["required_checks"].as_array().unwrap_or_else(|| panic!("no checks in {limits}"));
    assert_eq!(checks.len(), 2, "{limits}");
    assert_eq!(checks[0]["id"], "tests");
    assert_eq!(checks[0]["command"], "cargo test --offline");
    assert_eq!(checks[0]["timeout"], 600);
    assert_eq!(checks[0]["inputs"], serde_json::json!(["src"]));
    assert!(checks[0].get("network").is_none(), "an unset flag stays absent: {}", checks[0]);
    assert_eq!(checks[1]["id"], "docs");
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// A14: with no bubblewrap the shell must refuse to run **anything**, and the client's
/// own acceptance commands are no exception — a `--check` command that quietly ran on
/// the host would defeat the isolation the user asked for. The test drives the real
/// binary in a child process whose `PATH` holds no `bwrap` (a child's environment,
/// never the test process's own: other tests resolve tools through `PATH`), with a
/// dead model endpoint, so the turn fails first and the verdict is where the refusal
/// shows up — the same shape a user meets it in.
///
/// The positive control (the same command *with* bwrap runs and leaves its trace) needs a machine that has
/// bubblewrap; the refusal does not, so on a machine without one — the GitHub runner, D-113 — the test asserts
/// the refusal instead of printing "skipped" and leaving the A14 claim unchecked where CI runs.
#[test]
fn an_unisolated_shell_refuses_instead_of_running_on_the_host() {
    let have_sandbox = teamagents_engine::tools::sandbox_usable();
    let root = Scratch::new("isolation");
    let (config_home, state, workspace, empty_bin) =
        (root.join("config"), root.join("root"), root.join("ws"), root.join("empty-bin"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&empty_bin).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_ISOLATION_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();

    let run = |workspace: &std::path::Path, state: &std::path::Path, path: &std::path::Path| {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(["exec", "--state-root"])
            .arg(state)
            .args(["--full-auto", "--json", "--timeout", "20", "--cwd"])
            .arg(workspace)
            // the check leaves a file in its cwd when it runs anywhere at all
            .args(["--check", "touch ran-unisolated", "say hi"])
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("xdg-state"))
            .env("TA_ISOLATION_KEY", "test-value")
            .env("PATH", path)
            .output()
            .expect("run exec");
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        serde_json::from_str::<serde_json::Value>(printed.trim())
            .unwrap_or_else(|e| panic!("the run must print its JSON report ({e}): {printed:?}"))
    };

    let control_state = root.join("root-control");
    if have_sandbox {
        // Control first: the same command with the machine's own PATH runs inside the
        // sandbox and really does leave the file — without this, the absence below would
        // prove nothing (a check whose assertion cannot fail is not a check).
        let control_workspace = root.join("ws-control");
        std::fs::create_dir_all(&control_workspace).unwrap();
        let host_path = std::env::var("PATH").unwrap_or_default();
        let control = run(&control_workspace, &control_state, std::path::Path::new(&host_path));
        assert_eq!(control["verification"][0]["ok"], serde_json::json!(true), "the control check runs: {control}");
        assert!(control_workspace.join("ran-unisolated").is_file(), "and leaves its trace: {control}");
    }

    // Then the refusal: no bwrap anywhere, so `which("bwrap")` fails inside the client
    let report = run(&workspace, &state, &empty_bin);
    let verdict = &report["verification"][0];
    assert_eq!(verdict["ok"], serde_json::json!(false), "{report}");
    let error = verdict["error"].as_str().unwrap_or("");
    assert!(error.contains("IsolationUnavailable"), "the refusal names the isolation failure: {report}");
    assert!(!workspace.join("ran-unisolated").exists(), "the check ran somewhere: {report}");
    assert!(std::fs::read_dir(&workspace).unwrap().next().is_none(), "and it must have left nothing behind");
    // `exec` detaches the daemon it starts — that is its contract, the session outlives the client — so the
    // test has to stop both of them (the same rule the `Daemon` guard at the top of this file states): two
    // leaked daemons per run stay behind, holding their coordinator locks, and the state root cannot be
    // removed. Asserting they are gone is what keeps this from being a silent leak again.
    for state in [&control_state, &state] {
        // no assertion here: the control state has a daemon only where the sandbox exists (`if have_sandbox`
        // above), and this loop runs on both kinds of machine — the socket assertion below is what proves the
        // stop worked. Stopping is still by pid, never by pattern (D-144/D-148/D-150).
        let _ = stop_detached_daemon(state);
    }
    let mut alive = String::new();
    for _ in 0..100 {
        let output = Command::new("pgrep")
            .args(["-f", &format!("daemon --state-root {}", root.display())])
            .output()
            .expect("pgrep");
        alive = String::from_utf8_lossy(&output.stdout).into_owned();
        if alive.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(alive.is_empty(), "the daemons this test started are gone: {alive}");
    std::fs::remove_dir_all(&root).expect("the state roots go away with their daemons");
}

/// `--full-auto` used to be parsed and thrown away by both entry points, so a
/// documented flag did nothing. It now reaches the daemon this client starts,
/// and against a session that is already running (whose mode is fixed, D-41)
/// the client says so with the mode the daemon reports.
#[test]
fn full_auto_reaches_a_started_daemon_and_is_reported_against_a_live_one() {
    fn greeting(socket: &std::path::Path) -> serde_json::Value {
        use std::io::{BufRead, BufReader};
        let stream = std::os::unix::net::UnixStream::connect(socket).expect("connect");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("greeting");
        serde_json::from_str(&line).expect("greeting JSON")
    }
    let root = Scratch::new("full-auto");
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_FULL_AUTO_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_FULL_AUTO_KEY", "test-value");
    };

    // full-auto on a fresh state root: the daemon this run starts boots in it
    let mut exec = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut exec);
    let output = exec
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--full-auto", "--timeout", "5", "hello"])
        .output()
        .expect("run exec");
    let socket = state.join("daemon.sock");
    assert!(socket.exists(), "exec started the daemon");
    assert_eq!(greeting(&socket)["permissions"], "full_auto", "the flag reached the daemon");
    let _ = output;

    // the session keeps its mode: asking again cannot change a running daemon
    let mut again = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut again);
    let output = again
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--full-auto", "--timeout", "5", "hello"])
        .output()
        .expect("run exec again");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already running for this state root in full_auto mode"),
        "the client reports the real mode instead of pretending: {stderr}"
    );
    // Stop that daemon (exec detaches it on purpose) and prove it is gone: a
    // leaked session would keep this state root alive after the test.
    assert!(stop_detached_daemon(&state), "the daemon this test started is stopped by pid, not by pattern");
    for _ in 0..100 {
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        std::os::unix::net::UnixStream::connect(&socket).is_err(),
        "the daemon must stop so the test leaks nothing"
    );

    // D-75: the mode is also a config decision — `[permissions] mode` in the user's
    // own file is the session default, and the flag still wins over it. The key used
    // to be parsed, validated and ignored, so a user who wrote `full_auto` silently
    // ran in approved_scope.
    let write_config = |mode: &str| {
        std::fs::write(
            config_home.join("teamagents/config.toml"),
            format!(
                "[permissions]\nmode = \"{mode}\"\n\n[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
                 api_key_env = \"TA_FULL_AUTO_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n"
            ),
        )
        .unwrap();
    };
    let boot_with = |args: &[&str], state: &std::path::Path| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        command.args(["exec", "--state-root"]).arg(state).args(args).args(["--timeout", "5", "hello"]);
        let _ = command.output().expect("run exec");
        let greeting = greeting(&state.join("daemon.sock"));
        assert!(stop_detached_daemon(state), "the daemon this test started is stopped by pid, not by pattern");
        greeting["permissions"].as_str().unwrap_or("").to_string()
    };
    write_config("full_auto");
    assert_eq!(boot_with(&[], &root.join("configured")), "full_auto", "the config sets the session's mode");
    write_config("approved_scope");
    assert_eq!(boot_with(&[], &root.join("default")), "approved_scope", "the documented default stays");
    assert_eq!(
        boot_with(&["--full-auto"], &root.join("flagged")),
        "full_auto",
        "and the flag still asks for host execution for one boot"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `--cwd` used to be dropped when a client started the daemon, so the session
/// (and every instance in it) worked in whatever directory the client happened
/// to be started from — a dogfooding run spent sixty turns exploring the wrong
/// tree before it found the intended one (D-57). The flag now reaches the
/// daemon, and the instance's recorded workspace proves where the tools work.
#[test]
fn cwd_reaches_a_started_daemon_and_is_reported_against_a_live_one() {
    fn greeting(socket: &std::path::Path) -> serde_json::Value {
        use std::io::{BufRead, BufReader};
        let stream = std::os::unix::net::UnixStream::connect(socket).expect("connect");
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).expect("greeting");
        serde_json::from_str(&line).expect("greeting JSON")
    }
    let root = Scratch::new("cwd");
    let (config_home, state, first, second) =
        (root.join("config"), root.join("root"), root.join("one"), root.join("two"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    for dir in [&first, &second] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_CWD_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_CWD_KEY", "test-value");
    };

    let mut exec = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut exec);
    exec.args(["exec", "--state-root"])
        .arg(&state)
        .args(["--cwd"])
        .arg(&first)
        .args(["--timeout", "5", "hello"])
        .output()
        .expect("run exec");
    let socket = state.join("daemon.sock");
    let live = greeting(&socket);
    assert_eq!(live["workspace"], first.canonicalize().unwrap().to_string_lossy().as_ref(), "{live}");
    // the instance the daemon drives works in exactly that directory
    let db = state.join("session.sqlite");
    let mut stored = String::new();
    for _ in 0..200 {
        if let Ok(control) = teamagents_core::v2::Control::open(&db, "test", false) {
            if let Ok(value) =
                control
                    .connection()
                    .query_row("SELECT workspace_ref FROM instances LIMIT 1", [], |row| row.get::<_, String>(0))
            {
                stored = value;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert_eq!(stored, first.to_string_lossy(), "the tools' workspace is the requested one");

    // a running session keeps its workspace: the client says so instead of
    // silently working somewhere else
    let mut again = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut again);
    let output = again
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--cwd"])
        .arg(&second)
        .args(["--timeout", "5", "hello"])
        .output()
        .expect("run exec again");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("works in") && stderr.contains("--cwd") && stderr.contains(second.to_str().unwrap()),
        "the note names the live workspace and the ignored flag: {stderr}"
    );

    assert!(stop_detached_daemon(&state), "the daemon this test started is stopped by pid, not by pattern");
    for _ in 0..100 {
        if std::os::unix::net::UnixStream::connect(&socket).is_err() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(std::os::unix::net::UnixStream::connect(&socket).is_err(), "the daemon must stop");
    let _ = std::fs::remove_dir_all(&root);
}

/// A `--cwd` that is not an existing directory cannot become a session's workspace: every file tool and
/// shell command is confined to that root and `tools.rs` resolves it with `canonicalize`, so accepting it
/// left the session running against a root nothing could resolve — the model saw a bare `No such file or
/// directory (os error 2)` that never named the flag, while the greeting reported the bad path as the
/// session's workspace (measured 2026-09-27, D-163). Both the client that autostarts a daemon and `daemon`
/// itself refuse it, before any session exists.
#[test]
fn a_cwd_that_is_not_a_directory_is_refused_before_a_session_starts() {
    let root = Scratch::new("cwd-refused");
    let config_home = root.join("config");
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_CWD_REFUSED_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let missing = root.join("missing");
    let file = root.join("afile");
    std::fs::write(&file, "not a directory").unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_CWD_REFUSED_KEY", "test-value");
    };

    for (index, label, bad) in [(0, "a path that does not exist", &missing), (1, "a path that is a file", &file)] {
        let state = root.join(format!("state-client-{index}"));
        let mut exec = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut exec);
        let output = exec
            .args(["exec", "--state-root"])
            .arg(&state)
            .args(["--cwd"])
            .arg(bad)
            .args(["--timeout", "5", "hello"])
            .output()
            .expect("run exec");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{label}: {stderr}");
        assert!(
            stderr.contains("--cwd") && stderr.contains(bad.to_str().unwrap()) && stderr.contains("not a directory"),
            "{label} must name the flag and the path: {stderr}"
        );
        assert!(!state.join("daemon.sock").exists(), "{label}: the client started nothing");
        assert!(!stop_detached_daemon(&state), "{label}: no daemon exists for that state root");
    }

    // `daemon` refuses it too: a session started by hand must not boot with a workspace that is not one
    let state = root.join("state-direct");
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let output =
        daemon.args(["daemon", "--state-root"]).arg(&state).args(["--cwd"]).arg(&missing).output().expect("run daemon");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("--cwd") && stderr.contains("not a directory"), "{stderr}");
    assert!(!state.join("daemon.sock").exists(), "the refusal happens before the socket exists");

    // Against a *live* session the same path is refused too: the flag can never be honoured there (the
    // session keeps its own workspace) and the path is what `--check` would run the user's acceptance
    // commands in, so the client refuses it instead of printing the "did not apply" note and then running
    // those commands somewhere that does not exist.
    let real = root.join("real");
    std::fs::create_dir_all(&real).unwrap();
    let state = root.join("state-live");
    let mut boot = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut boot);
    boot.args(["exec", "--state-root"]).arg(&state).args(["--cwd"]).arg(&real).args(["--timeout", "5", "hello"]);
    let _ = boot.output().expect("start the session");
    assert!(state.join("daemon.sock").exists(), "the session started with a real workspace");

    let mut join = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut join);
    let output = join
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--cwd"])
        .arg(&missing)
        .args(["--timeout", "5", "hello"])
        .output()
        .expect("join the session");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("not a directory"), "{stderr}");
    assert!(!stderr.contains("did not apply"), "a path that is not a directory is refused, not reported: {stderr}");
    assert!(stop_detached_daemon(&state), "the session this test started is stopped by pid, not by pattern");

    let _ = std::fs::remove_dir_all(&root);
}

/// D-164: a leader that cannot start must not leave a headless run waiting out its whole deadline. The
/// supervisor parks such an instance with the runtime's own words (D-104), but nothing carried them to a
/// client: the TUI logged only the lifecycle word and `exec` reported `end=timeout`/exit 124 with
/// `failure: null` — 15 minutes of silence on the default timeout, with the one sentence that says what to
/// fix sitting in `daemon.log`.
///
/// The service here fails *after* the client submitted (its handshake blocks for a few seconds), which is the
/// ordering the pre-submit guard cannot cover: the instance is ACTIVE when `exec` reads its checkpoint, and
/// parks afterwards.
///
/// `mcp_execution = "host"` is what makes that ordering hold in both CI conditions: under the workspace default
/// the server cannot start at all without bubblewrap (`IsolationUnavailable`, A14), so the park would land
/// before the client's checkpoint and the guard — not the path under test — would report it (`make check-nobwrap`
/// caught exactly that).
#[test]
fn a_leader_parked_under_a_waiting_run_reports_the_park_instead_of_timing_out() {
    let root = Scratch::new("park");
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_PARK_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\n\
         [tools.slow]\nkind = \"mcp\"\nmcp_transport = \"stdio\"\ncommand = \"/bin/sh\"\n\
         args = [\"-c\", \"sleep 4; exit 1\"]\nmcp_execution = \"host\"\nrequired = true\n",
    )
    .unwrap();
    let started = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
        .args(["exec", "--state-root"])
        .arg(&state)
        .args(["--json", "--timeout", "60", "hello"])
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("TA_PARK_KEY", "test-value")
        .output()
        .expect("run exec");
    let elapsed = started.elapsed();
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("a JSON report ({e}): {output:?}"));
    assert_eq!(report["end"], "failed", "{report}");
    assert_eq!(report["instance_lifecycle"], "PARKED", "{report}");
    let failure = report["failure"].as_str().unwrap_or("");
    assert!(failure.contains("PARKED") && failure.contains("required tool service \"slow\""), "{failure}");
    assert!(failure.contains("instances resume"), "the message carries the lever: {failure}");
    assert_eq!(output.status.code(), Some(1), "{report}");
    // The report itself is the proof the *pre-submit* guard did not produce this outcome: that path prints no
    // report at all ("nothing was submitted", exit 2).
    assert!(elapsed.as_secs() < 30, "the run ends on the park, not on its 60 s deadline: {elapsed:?}");
    assert!(stop_detached_daemon(&state), "the daemon this test started is stopped by pid, not by pattern");
    let _ = std::fs::remove_dir_all(&root);
}

/// D-165: `teamagents instances` says *why* an instance is not running. D-164 carried the park reason to the
/// run's own report and the TUI note, but the list a user reads first showed `PARKED` and stopped there — the
/// checkpoint's row had no reason (it lived in the event log), so the guide's promise that `instances` "shows
/// `PARKED` and the reason" was not true of it. A required MCP service whose command does not exist parks the
/// leader's driver at boot, which is the cheapest real park to produce: no model call is involved.
#[test]
fn the_instances_list_says_why_an_instance_is_parked() {
    let root = Scratch::new("instances");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_INSTANCES_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\n\
         [tools.broken]\nkind = \"mcp\"\nmcp_transport = \"stdio\"\ncommand = \"/nonexistent/mcp-server\"\n\
         required = true\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_INSTANCES_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .arg("--cwd")
        .arg(&ws)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..400 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // wait for the park over the socket (cheaper than spawning the CLI per poll), then read the product's own
    // surface: the assertion is about what `instances` prints, not about the database behind it
    let mut rpc = Rpc::connect(&socket);
    let mut parked = false;
    for _ in 0..600 {
        let checkpoint = rpc.call("checkpoint", serde_json::json!({}));
        let instances = checkpoint["result"]["snapshot"]["instances"].as_array().cloned().unwrap_or_default();
        if instances.iter().any(|row| row["lifecycle"] == "PARKED") {
            parked = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(parked, "a required MCP service whose command does not exist parks the leader's driver");
    let run_instances = |json_out: bool| -> String {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        command.args(["instances", "--state-root"]).arg(&state);
        if json_out {
            command.arg("--json");
        }
        let output = command.output().expect("run instances");
        format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
    };
    let list = run_instances(false);
    assert!(list.contains("i-leader") && list.contains("PARKED"), "{list}");
    assert!(list.contains("required tool service \"broken\" is unavailable"), "the row says why: {list}");

    // the JSON form carries the same field, which is what a script reads
    let report: serde_json::Value = serde_json::from_str(&run_instances(true)).expect("a JSON report on stdout");
    let row = report["instances"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["id"] == "i-leader"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(row["lifecycle"], "PARKED", "{report}");
    assert!(row["reason"].as_str().unwrap_or("").contains("is unavailable"), "{report}");
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// D-166: a `--state-root` that is a *file* must be refused with the flag named, in every entry point. The two
/// shapes a user confuses are one path segment apart — the root directory and the `session.sqlite` inside it —
/// and each entry point answered with a different raw errno (measured 2026-09-27): `File exists (os error 17)`
/// from `daemon` and `init`, `Not a directory (os error 20)` from a read verb, and a `doctor` WARN that told
/// the user to run `init` — a command that then failed the same way. A root that does not exist yet stays
/// legal: it is created (`init`, `daemon`, or the client that starts the daemon).
#[test]
fn a_state_root_that_is_a_file_is_refused_by_every_entry_point() {
    let root = Scratch::new("state-root");
    let (config_home, dir, db_like, plain, uninitialized) =
        (root.join("config"), root.join("dir"), root.join("db-file"), root.join("plain"), root.join("uninitialized"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    // the mistake this pins: the session database itself, passed where its directory belongs
    std::fs::write(dir.join("session.sqlite"), "not a database").unwrap();
    std::fs::write(&db_like, "not a database").unwrap();
    std::fs::write(&plain, "just a file").unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\napi_key_env = \"TA_SR_KEY\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| -> (i32, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_SR_KEY", "test-value")
            .args(args)
            .output()
            .expect("run teamagents");
        (
            output.status.code().unwrap_or(-1),
            format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)),
        )
    };
    for bad in [dir.join("session.sqlite"), db_like, plain] {
        let bad = bad.to_str().unwrap();
        // doctor: a FAIL row; the WARN it printed before pointed at a command that cannot work
        let (code, out) = run(&["doctor", "--state-root", bad]);
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("[FAIL] v2 state root") && out.contains("is not a directory"), "{out}");
        // the entry points that need the root: none of them leaves the errno to speak for itself
        for (args, expected) in [
            (vec!["init", "--state-root", bad], 1),
            (vec!["daemon", "--state-root", bad], 1),
            (vec!["authority", "--state-root", bad], 2),
        ] {
            let (code, out) = run(&args);
            assert_eq!(code, expected, "{args:?}: {out}");
            assert!(out.contains("--state-root") && out.contains("is not a directory"), "{args:?}: {out}");
        }
        // a client that would start a daemon refuses before spawning one
        let (code, out) = run(&["exec", "--state-root", bad, "--timeout", "5", "hello"]);
        assert_eq!(code, 2, "{out}");
        assert!(out.contains("--state-root") && out.contains("is not a directory"), "{out}");
    }
    // control: a directory that is not initialized yet stays legal — WARN, then `init` makes it a root
    let (code, out) = run(&["doctor", "--state-root", uninitialized.to_str().unwrap()]);
    assert!(out.contains("[WARN] v2 state root") && out.contains("not initialized yet"), "{out}");
    // the verdict is doctor's, over every row: an isolation FAIL (no bubblewrap, A14) is not about this root
    assert!(code == 0 || out.contains("[FAIL] bubblewrap isolation"), "{code} without an isolation failure: {out}");
    let (code, out) = run(&["init", "--state-root", uninitialized.to_str().unwrap()]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("state root ready"), "{out}");
    assert!(uninitialized.join("session.sqlite").is_file(), "init created the root");
    let _ = std::fs::remove_dir_all(&root);
}

/// The daemon a user actually starts must authorize its Leader: before D-58 the
/// only grant in a real session was `shell@workspace`, so the model was never
/// offered spawn/delegate/send and the team feature was unreachable from
/// `teamagents`, `exec` and `daemon` alike.
#[test]
fn the_daemon_grants_the_leader_the_team_authority() {
    let root = Scratch::new("grants");
    let (config_home, state) = (root.join("config"), root.join("root"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_GRANTS_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_GRANTS_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    // read the grant table through the daemon's own protocol
    use std::io::{BufRead, BufReader, Write};
    let stream = std::os::unix::net::UnixStream::connect(&socket).expect("connect");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    let mut greeting = String::new();
    reader.read_line(&mut greeting).expect("greeting");
    let mut grants = serde_json::Value::Null;
    for _ in 0..200 {
        writer
            .write_all(
                format!(
                    "{}\n",
                    serde_json::json!({"protocol_version": 1, "request_id": "g1", "method": "grants", "params": {}})
                )
                .as_bytes(),
            )
            .expect("write");
        let mut line = String::new();
        reader.read_line(&mut line).expect("reply");
        let reply: serde_json::Value = serde_json::from_str(&line).expect("reply JSON");
        let held = reply["result"]["grants"].as_array().cloned().unwrap_or_default();
        if held.iter().any(|grant| grant["action"] == "manage") {
            grants = reply["result"]["grants"].clone();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let granted = |action: &str| {
        grants
            .as_array()
            .map(|all| {
                all.iter().any(|grant| {
                    grant["subject"] == "i-leader" && grant["action"] == action && grant["resource_scope"] == "session"
                })
            })
            .unwrap_or(false)
    };
    for action in ["manage", "delegate", "message"] {
        assert!(granted(action), "the session must grant the leader {action}@session: {grants}");
    }
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// One instance whose model cannot be built must not take the session with it.
/// The daemon's provider factory used to panic for such an instance — inside the
/// supervisor's discovery loop, so no further instance was ever driven and every
/// delegating Leader waited forever (D-59). It now returns a provider that fails
/// that instance's own requests permanently: the instance parks through the
/// ordinary classified path (A07) with the reason, and the session lives on.
#[test]
fn an_unbootable_instance_parks_itself_and_the_session_survives() {
    use std::io::{BufRead, BufReader, Write};
    let root = Scratch::new("unbootable");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_UNBOOTABLE_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_UNBOOTABLE_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let stream = std::os::unix::net::UnixStream::connect(&socket).expect("connect");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    let mut greeting = String::new();
    reader.read_line(&mut greeting).expect("greeting");
    let mut command = |id: &str, method: &str, params: serde_json::Value| {
        writer
            .write_all(
                format!(
                    "{}\n",
                    serde_json::json!({"protocol_version": 1, "request_id": id, "command_id": id,
                                       "method": method, "params": params})
                )
                .as_bytes(),
            )
            .expect("write");
        let mut line = String::new();
        reader.read_line(&mut line).expect("reply");
        serde_json::from_str::<serde_json::Value>(&line).expect("reply JSON")
    };

    // an instance whose model is not in the catalog, given work to do
    let broken = command(
        "c-bad",
        "create_instance",
        serde_json::json!({"id": "i-broken", "workspace_ref": ws.to_string_lossy(),
                           "profile": {"model": "gpt-9", "instructions": "never boots"}}),
    );
    assert_eq!(broken["ok"], true, "{broken}");
    command("c-goal", "create_goal", serde_json::json!({"id": "g-broken", "instance_id": "i-broken"}));
    command(
        "c-input",
        "submit_input",
        serde_json::json!({"instance_id": "i-broken", "envelope_id": "env-broken", "text": "do the work"}),
    );

    // it parks with the reason, and the leader's session keeps running
    let mut parked = false;
    let mut reason = String::new();
    for _ in 0..400 {
        let reply = command("c-check", "checkpoint", serde_json::json!({}));
        let instances = reply["result"]["snapshot"]["instances"].as_array().cloned().unwrap_or_default();
        let leader = instances.iter().find(|entry| entry["id"] == serde_json::json!("i-leader"));
        assert_eq!(
            leader.map(|entry| entry["lifecycle"].clone()),
            Some(serde_json::json!("ACTIVE")),
            "the session must live on: {reply}"
        );
        if let Some(broken) = instances.iter().find(|entry| entry["id"] == serde_json::json!("i-broken")) {
            if broken["lifecycle"] == serde_json::json!("PARKED") {
                parked = true;
                // the reason travels with the classified failure the instance
                // parked on, not in the snapshot
                let events = command("c-events", "events", serde_json::json!({"since": 0}));
                reason = events["result"]["events"]
                    .as_array()
                    .map(|events| {
                        events
                            .iter()
                            .filter_map(|event| event["payload"]["reason"].as_str())
                            .collect::<Vec<&str>>()
                            .join(" | ")
                    })
                    .unwrap_or_default();
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(parked, "the unbootable instance parks itself through the classified path");
    assert!(reason.contains("gpt-9"), "the reason names the model: {reason:?}");
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// A socket client for the daemon's JSON-lines protocol, as a test would write it:
/// `authority` is the product's client, and these tests use the same wire shape to
/// set a session up and to read back what the surface did.
struct Rpc {
    reader: std::io::BufReader<std::os::unix::net::UnixStream>,
    writer: std::os::unix::net::UnixStream,
    counter: u64,
}

impl Rpc {
    fn connect(socket: &std::path::Path) -> Rpc {
        use std::io::{BufRead, BufReader};
        let stream = std::os::unix::net::UnixStream::connect(socket).expect("connect");
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut greeting = String::new();
        reader.read_line(&mut greeting).expect("greeting");
        Rpc { reader, writer: stream, counter: 0 }
    }

    fn roundtrip(&mut self, method: &str, params: serde_json::Value, command_id: Option<&str>) -> serde_json::Value {
        use std::io::{BufRead, Write};
        self.counter += 1;
        let mut frame = serde_json::json!({"protocol_version": 1, "request_id": format!("r{}", self.counter),
                                           "method": method, "params": params});
        if let Some(command_id) = command_id {
            frame["command_id"] = serde_json::json!(command_id);
        }
        self.writer.write_all(format!("{frame}\n").as_bytes()).expect("write");
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("reply");
        serde_json::from_str(&line).expect("reply JSON")
    }

    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.roundtrip(method, params, None)
    }

    fn command(&mut self, id: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.roundtrip(method, params, Some(id))
    }

    fn grants(&mut self) -> Vec<serde_json::Value> {
        self.call("grants", serde_json::json!({}))["result"]["grants"].as_array().cloned().unwrap_or_default()
    }
}

/// The user's authority surface (D-61), end to end: the real binary, the real
/// daemon socket, the real control plane. Before it existed the capability
/// boundary was real but unreachable for a user — `issue_grant`/`revoke_grant` had
/// no caller outside tests, and the daemon's grant view did not even carry the id
/// a revoke must name, so no client could have revoked anything.
#[test]
fn the_authority_surface_grants_and_revokes_through_the_daemon() {
    let root = Scratch::new("authority");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_AUTHORITY_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_AUTHORITY_KEY", "test-value");
    };
    // `teamagents authority …` with the state root this session lives in
    let authority = |args: &[&str]| -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        let output = command.args(args).arg("--state-root").arg(&state).output().expect("run authority");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let json = |text: &str| -> serde_json::Value { serde_json::from_str(text).expect("JSON report") };

    // no session yet: the surface says so instead of writing into the void
    let (code, _, stderr) = authority(&["authority"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("start one with"), "the error points at a session: {stderr}");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..400 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let mut rpc = Rpc::connect(&socket);
    // wait for the bootstrap (the leader's own grants)
    for _ in 0..400 {
        if rpc.grants().iter().any(|grant| grant["action"] == serde_json::json!("manage")) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    // 1. the list carries what a revoke needs: an id, its issuer and the revision
    let (code, out, stderr) = authority(&["authority", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let listed = json(&out);
    assert!(listed["revision"].as_i64().unwrap_or(0) > 0, "{listed}");
    let leader_shell = listed["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .find(|grant| {
            grant["subject"] == serde_json::json!("i-leader") && grant["action"] == serde_json::json!("shell")
        })
        .expect("the leader's workspace shell");
    assert!(leader_shell["id"].as_str().is_some_and(|id| !id.is_empty()), "the view carries the id: {listed}");
    assert_eq!(leader_shell["issuer"], serde_json::json!("user"), "{listed}");
    assert_eq!(leader_shell["revoked"], serde_json::json!(false), "{listed}");

    // human output is a table a user can read, and it names the id
    let (code, out, _) = authority(&["authority"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("SUBJECT") && out.contains("i-leader") && out.contains(leader_shell["id"].as_str().unwrap()),
        "{out}"
    );

    // 2. a worker the leader spawned holds no shell grant (§5.1)
    let spawned = rpc.command(
        "a-spawn",
        "spawn_instance",
        serde_json::json!({"instance_id": "i-worker-1", "workspace_ref": ws.to_string_lossy(),
                           "profile": {"model": "leader_main", "instructions": "helper"}}),
    );
    assert_eq!(spawned["ok"], true, "{spawned}");
    let holds_shell = |rpc: &mut Rpc| {
        rpc.grants().iter().any(|grant| {
            grant["subject"] == serde_json::json!("i-worker-1")
                && grant["action"] == serde_json::json!("shell")
                && !grant["revoked"].as_bool().unwrap_or(false)
        })
    };
    assert!(!holds_shell(&mut rpc), "a spawned worker holds no shell grant");

    // 3. a parent the session does not know is the control plane's refusal
    let (code, _, stderr) = authority(&[
        "authority",
        "grant",
        "--subject",
        "i-worker-1",
        "--action",
        "shell",
        "--scope",
        "workspace",
        "--parent",
        "g-does-not-exist",
    ]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("parent grant g-does-not-exist"), "{stderr}");

    // 4. the user grants the shared-workspace shell, derived from the leader's own
    //    grant: the user's authority is the root, and the derived grant goes with
    //    the parent (A03)
    let (code, out, stderr) = authority(&[
        "authority",
        "grant",
        "--subject",
        "i-worker-1",
        "--action",
        "shell",
        "--scope",
        "workspace",
        "--parent",
        leader_shell["id"].as_str().unwrap(),
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let granted = json(&out);
    let worker_grant = granted["grant_id"].as_str().expect("the new grant id").to_string();
    assert_eq!(granted["subject"], serde_json::json!("i-worker-1"), "{granted}");
    assert_eq!(granted["parent_grant_id"], leader_shell["id"], "the derived grant names its parent");
    assert!(holds_shell(&mut rpc), "the grant is live in the session");
    // the dispatch question A03/A04 re-checks at the linearization point
    let holds_now = |action: &str| {
        teamagents_core::v2::Control::open(&state.join("session.sqlite"), "test", false)
            .expect("control")
            .holds_covering_grant("i-worker-1", action, "workspace")
            .expect("grant read")
    };
    assert!(holds_now("shell"), "the worker may now dispatch the workspace shell");

    // 5. a pair no check consults is refused instead of written
    for (action, scope, expected) in [
        ("shell", "instance:i-worker-1", "no check asks about shell@instance:i-worker-1"),
        ("shel", "workspace", "is not an action the runtime checks"),
    ] {
        let (code, _, stderr) =
            authority(&["authority", "grant", "--subject", "i-worker-1", "--action", action, "--scope", scope]);
        assert_eq!(code, 2, "{stderr}");
        assert!(stderr.contains(expected), "{stderr}");
    }
    // and a subject that does not exist is a warning, not a refusal: instance ids
    // belong to the spawner, so a grant may legitimately come first
    let (code, _, stderr) =
        authority(&["authority", "grant", "--subject", "i-typo", "--action", "message", "--scope", "session"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("no instance \"i-typo\""), "{stderr}");
    assert!(stderr.contains("i-worker-1"), "the warning names the known instances: {stderr}");

    // 6. revoking the parent takes the derived grant with it, and the dispatch
    //    question is false again: revocation is final (A03)
    let (code, out, stderr) =
        authority(&["authority", "revoke", "--grant", &leader_shell["id"].as_str().unwrap()[..12], "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let revocation = json(&out);
    let cascade: Vec<&str> = revocation["revoked"].as_array().unwrap().iter().filter_map(|id| id.as_str()).collect();
    assert!(cascade.contains(&worker_grant.as_str()), "the derived grant goes with its parent: {revocation}");
    assert_eq!(cascade.len(), 2, "{revocation}");
    assert!(!holds_shell(&mut rpc), "no live shell grant for the worker is left");
    assert!(!holds_now("shell"), "a revocation is final: the dispatch question is false again");

    // 7. an id no grant matches is refused before anything is written
    let (code, _, stderr) = authority(&["authority", "revoke", "--grant", "g-nope"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("no grant id starts with"), "{stderr}");
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// D-267: the user's goal surface, through the real binary and a real daemon. A session is anchored to a goal and
/// a settled goal cannot be reopened, so without a lever "what do I do next in this session?" had no answer a user
/// could act on: `create_goal` is an ordinary user command the daemon forwards, but nothing in the product sent it
/// (and until D-266 a goal opened this way could not be charged either).
#[test]
fn the_goal_surface_lists_and_opens_goals_through_the_daemon() {
    let root = Scratch::new("goals");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_GOALS_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\
         [limits]\nmax_total_tokens = 400000\ndeadline_minutes = 15\n\
         [[checks]]\nid = \"session-tests\"\ncommand = \"true\"\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_GOALS_KEY", "test-value")
            // the goal carries required checks, and a check runs through the runner: point it at the built binary
            // (the rule `jobs_runner.rs` follows) or the daemon would fail the check and settle the goal FAILED
            .env("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    };
    let goals = |args: &[&str]| -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        let output = command.args(args).arg("--state-root").arg(&state).output().expect("run goals");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let json = |text: &str| -> serde_json::Value { serde_json::from_str(text).expect("JSON report") };

    // no session yet: the surface says so instead of writing into the void
    let (code, _, stderr) = goals(&["goals"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("start one with"), "the error points at a session: {stderr}");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let _guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..400 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // wait for the bootstrap's own goal
    let mut listed = serde_json::Value::Null;
    for _ in 0..400 {
        let (code, out, _) = goals(&["goals", "--json"]);
        if code == 0 {
            listed = json(&out);
            if !listed["goals"].as_array().map(|g| g.is_empty()).unwrap_or(true) {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let boot = listed["goals"].as_array().cloned().unwrap_or_default();
    assert_eq!(boot.len(), 1, "the bootstrap opens exactly one goal: {listed}");
    assert_eq!(boot[0]["status"], serde_json::json!("ACTIVE"), "{listed}");
    assert_eq!(
        boot[0]["attached_instances"],
        serde_json::json!(["i-leader"]),
        "the boot goal is attached to the leader: {listed}"
    );
    // (opening and attaching a second goal moves the instance's pointer — asserted where it happens, below)
    let first = boot[0]["id"].as_str().unwrap_or_default().to_string();

    // a usage error stays a usage error
    let (code, _, stderr) = goals(&["goals", "open", "--id", "g2", "--check", "no-equals-sign"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("ID=COMMAND"), "{stderr}");
    let (code, _, stderr) = goals(&["goals", "open"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("needs --id"), "{stderr}");

    // 1. open the next goal, attached to the leader, carrying the user's own required check
    let (code, out, stderr) = goals(&[
        "goals",
        "open",
        "--id",
        "goal-second",
        "--attach",
        "i-leader",
        "--check",
        "later-tests=true",
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(json(&out)["goal_id"], serde_json::json!("goal-second"), "{out}");
    // D-283: the report says what the goal *got*. One --check was asked for and the session's own
    // `session-tests` is unioned in by the daemon (D-268), so the goal carries two; before this the
    // report echoed the asked-for count.
    assert_eq!(json(&out)["checks"], serde_json::json!(2), "the report carries the goal's own checks: {out}");

    // 2. the list carries both, names the active one, and says what is attached to what
    let (code, out, stderr) = goals(&["goals", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let report = json(&out);
    let rows = report["goals"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 2, "the settled-or-not pair the session now carries: {report}");
    let second = rows.iter().find(|g| g["id"] == serde_json::json!("goal-second")).expect("the opened goal");
    assert_eq!(second["status"], serde_json::json!("ACTIVE"), "{report}");
    assert_eq!(second["attached_instances"], serde_json::json!(["i-leader"]), "{report}");
    let checks = second["limits"]["required_checks"].as_array().cloned().unwrap_or_default();
    let ids: Vec<&str> = checks.iter().filter_map(|c| c["id"].as_str()).collect();
    assert!(ids.contains(&"later-tests"), "the check the CLI named rides on the goal: {report}");
    // D-268: the *session's* configured ceiling and checks bound a goal a client opens too (D-64) — the bootstrap
    // is not the only path that creates goals any more
    assert_eq!(
        second["limits"]["max_total_tokens"],
        serde_json::json!(400000),
        "the session's ceiling travels to a client-opened goal: {report}"
    );
    assert!(ids.contains(&"session-tests"), "the session's configured check is unioned in: {report}");
    assert!(
        second["deadline"].as_f64().is_some(),
        "the session's deadline_minutes becomes an absolute deadline: {report}"
    );
    // the *live* goal is the first row: the one an instance is attached to (attaching moves that pointer, so the
    // boot goal keeps its row and loses the attachment — which is why the order is "attached first", not "newest")
    assert_eq!(rows[0]["id"], serde_json::json!("goal-second"), "{report}");
    let boot_row = rows.iter().find(|g| g["id"] == serde_json::json!(first)).expect("the boot goal");
    assert_eq!(boot_row["attached_instances"], serde_json::json!([]), "{report}");

    // 3. the plain-text form names the goal, its attachments and its checks (read *before* the removal below,
    // which detaches the leader: the text form reports the attachments the record holds at read time)
    let (code, out, stderr) = goals(&["goals"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("goal-second"), "{out}");
    assert!(!out.contains("later-tests"), "the text form counts checks rather than listing them: {out}");
    assert!(out.contains("checks: 1"), "{out}");
    assert!(out.contains("attached: i-leader"), "{out}");

    // D-270: once no active goal is attached to the leader, `exec` says so *before* the turn spends anything. The
    // runtime still opens no goal for a later input (that is the design's own known gap), so without this line the
    // user reads the refusal inside the model's answer and blames the model. Settling `goal-second` detaches the
    // leader, and the still-ACTIVE boot goal points at nobody — the exact shape where the checkpoint's single goal
    // object would have reported the *boot* goal and hid the state, which is why the advisory reads the `goals`
    // list instead (D-267's ordering exists for the same reason).
    let mut rpc = Rpc::connect(&socket);
    let settled = rpc.command(
        "settle-goal-second",
        "complete_goal",
        serde_json::json!({"goal_id": "goal-second", "instance_id": "i-leader", "status": "SUCCEEDED",
                           "summary": "the probe settles it to test the advisory"}),
    );
    assert_eq!(settled["ok"], serde_json::json!(true), "{settled}");
    let (code, out, stderr) = goals(&["goals", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let after = json(&out);
    // the premise: the goal is terminal and the leader is detached from it (`complete_goal` derives the status
    // from the instance's last model completion, and there is none with a stub provider, so it reads FAILED — the
    // advisory keys on the attachment, not on the status, so the assertion follows the record rather than
    // assuming SUCCEEDED). Settling detaches, so the row is found by id, not by the list's position.
    let settled = after["goals"]
        .as_array()
        .and_then(|rows| rows.iter().find(|g| g["id"] == serde_json::json!("goal-second")))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    assert_ne!(settled["status"], serde_json::json!("ACTIVE"), "{after}");
    assert_eq!(settled["attached_instances"], serde_json::json!([]), "{after}");
    let mut exec = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut exec);
    let output = exec.args(["exec", "--state-root"]).arg(&state).arg("say something").output().expect("run exec");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no active goal is attached to i-leader"), "the advisory names the state: {stderr}");
    assert!(stderr.contains("teamagents goals open"), "and the lever: {stderr}");
    assert!(stderr.contains("--attach i-leader"), "and what to attach it to: {stderr}");

    // D-283: the shape the operator watched — a goal opened with no --check at all still carries the
    // session's configured check, and the plain line must report it instead of "(0 required check(s))".
    let (code, out, stderr) = goals(&["goals", "open", "--id", "goal-third", "--attach", "i-leader"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("opened goal goal-third (1 required check(s))"), "{out}");
}

/// D-349 (the decision D-341 made for D-143): `teamagents surface` reads back, per model request, the tool
/// *names* the request was offered and whether the surface check authorized that set — a fact in the record
/// instead of an inference from a `TEAMAGENTS_LOG_SURFACE=1` log line. The record is written when the request
/// is registered, so a turn that then fails at the provider still leaves one: that is the shape the witness
/// exists for, and it is what this test drives through the verb.
#[test]
fn the_surface_verb_reads_back_what_a_request_was_offered() {
    let root = Scratch::new("surface-verb");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_SURFACE_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\ntimeout = 5\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_SURFACE_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let report = |args: &[&str]| -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        let output = command.args(args).arg("--state-root").arg(&state).output().expect("run the CLI");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    // nothing has asked yet: the reader says so rather than pretending to a record
    let (code, out, stderr) = report(&["surface", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let empty: serde_json::Value = serde_json::from_str(&out).expect("JSON report");
    assert_eq!(empty["surfaces"], serde_json::json!([]), "{out}");
    // one input: the provider is unreachable, the turn fails — and the request's record still exists
    let _ = report(&["exec", "say something"]);
    for _ in 0..200 {
        let (code, out, _) = report(&["surface", "--json"]);
        if code == 0
            && serde_json::from_str::<serde_json::Value>(&out)
                .is_ok_and(|value| value["surfaces"] != serde_json::json!([]))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let (code, out, stderr) = report(&["surface", "--json", "--id", "i-leader"]);
    assert_eq!(code, 0, "{stderr}");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("JSON report");
    let rows = parsed["surfaces"].as_array().cloned().unwrap_or_default();
    assert!(!rows.is_empty(), "{out}");
    let offered: Vec<String> = rows[0]["offered_tools"]
        .as_array()
        .expect("the record carries names")
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect();
    assert!(offered.iter().any(|name| name == "wait"), "{offered:?}");
    assert!(offered.iter().any(|name| name == "delegate"), "{offered:?}");
    assert_eq!(rows[0]["surface_authorized"], serde_json::json!(true), "{out}");
    assert_eq!(rows[0]["instance_id"], serde_json::json!("i-leader"));
    // a prefix that names no instance is a client error (2, as the shared resolver uses), not an empty answer
    let (code, _, stderr) = report(&["surface", "--id", "i-nobody"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("i-nobody"), "{stderr}");
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// D-341/D-344: the user's lever on a goal nothing can spend — A18's known gap. A goal whose ceiling or
/// deadline is gone stays `ACTIVE` for ever, refusing new work and still listed as the status `goals list`
/// reports, and the instance its refusal parked has no path back; `goals cancel --id ID` is the missing half.
/// It settles the goal terminal (`CANCELLED`) and releases the instances that goal's refusal parked, and it
/// answers what it cannot do with a reason instead of failing silently.
#[test]
fn goals_cancel_closes_a_spent_goal_and_the_parked_leader_works_again() {
    let root = Scratch::new("goals-cancel");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_CANCEL_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\ntimeout = 5\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_CANCEL_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let report = |args: &[&str]| -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        let output = command.args(args).arg("--state-root").arg(&state).output().expect("run the CLI");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let json = |text: &str| -> serde_json::Value { serde_json::from_str(text).expect("JSON report") };
    let instance = |report: &serde_json::Value, id: &str| -> serde_json::Value {
        report["instances"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == id))
            .cloned()
            .unwrap_or_default()
    };
    let goal = |report: &serde_json::Value, id: &str| -> serde_json::Value {
        report["goals"].as_array().and_then(|rows| rows.iter().find(|row| row["id"] == id)).cloned().unwrap_or_default()
    };

    // the premise: the session's leader exists and can work
    let (code, out, stderr) = report(&["instances", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(instance(&json(&out), "i-leader")["lifecycle"], serde_json::json!("ACTIVE"), "{out}");

    // the user's own goal on the leader, then the park a spent goal's refusal causes (the driver parks
    // through `set_lifecycle` with the reason; the user's own park reaches the same state)
    let (code, out, stderr) = report(&["goals", "open", "--id", "g-cancel", "--attach", "i-leader"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("opened goal g-cancel"), "{out}");
    let mut rpc = Rpc::connect(&socket);
    let parked = rpc.command(
        "park-leader",
        "set_lifecycle",
        serde_json::json!({"instance_id": "i-leader", "lifecycle": "PARKED",
                           "reason": "goal g-cancel ceiling spent"}),
    );
    assert_eq!(parked["ok"], serde_json::json!(true), "{parked}");
    let (code, out, stderr) = report(&["instances", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(instance(&json(&out), "i-leader")["lifecycle"], serde_json::json!("PARKED"), "{out}");

    // the lever: the goal closes, the leader is released
    let (code, out, stderr) = report(&["goals", "cancel", "--id", "g-cancel"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("cancelled goal g-cancel"), "{out}");
    assert!(out.contains("released i-leader"), "{out}");
    let (code, out, stderr) = report(&["goals", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let after = json(&out);
    assert_eq!(goal(&after, "g-cancel")["status"], serde_json::json!("CANCELLED"), "{after}");
    assert_eq!(goal(&after, "g-cancel")["attached_instances"], serde_json::json!([]), "{after}");
    // and the instance it parked accepts new work again (ACTIVE is what the dispatch gate requires)
    let (code, out, stderr) = report(&["instances", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(instance(&json(&out), "i-leader")["lifecycle"], serde_json::json!("ACTIVE"), "{out}");
    let (code, _, stderr) = report(&["goals", "open", "--id", "g-next", "--attach", "i-leader"]);
    assert_eq!(code, 0, "{stderr}");

    // the receipt rule the surface already has: the command id is derived from the goal, so a second run
    // replays the stored success rather than releasing twice
    let (code, out, stderr) = report(&["goals", "cancel", "--id", "g-cancel"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(out.contains("cancelled goal g-cancel"), "{out}");

    // what it cannot do, it says with a reason: an id nothing resolves to, a goal another path settled, and a
    // missing --id (usage, not the session's refusal)
    let (code, _, stderr) = report(&["goals", "cancel", "--id", "ghost"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("not in this session"), "{stderr}");
    let (code, _, stderr) = report(&["goals", "open", "--id", "g-closed", "--attach", "i-leader"]);
    assert_eq!(code, 0, "{stderr}");
    let settled = rpc.command(
        "settle-g-closed",
        "complete_goal",
        serde_json::json!({"goal_id": "g-closed", "instance_id": "i-leader"}),
    );
    assert_eq!(settled["ok"], serde_json::json!(true), "{settled}");
    let (code, _, stderr) = report(&["goals", "cancel", "--id", "g-closed"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("not ACTIVE"), "{stderr}");
    let (code, _, stderr) = report(&["goals", "cancel"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("needs --id"), "{stderr}");

    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// D-284, the reporting half of the budget-exhausted-goal gap in `docs/ACCEPTANCE.md`: a goal the runtime will
/// refuse used to read exactly like one taking work — `ACTIVE`, the status column's only word — while the
/// `goals` read already carried the numbers that show it. The plain line now carries the goal's budget and,
/// when the record proves it, why it cannot accept a new request: A18's ceiling (known usage leaves no room for
/// even a one-token request) and A35's deadline. No lever, no new read, and no new field in the JSON report.
#[test]
fn a_goal_that_cannot_accept_work_is_not_presented_as_in_force() {
    let root = Scratch::new("goals-exhausted");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_GOALS_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\
         [limits]\nmax_total_tokens = 5000000\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_GOALS_KEY", "test-value")
            .env("TEAMAGENTS_RUNNER_BIN", env!("CARGO_BIN_EXE_teamagents"));
    };
    let goals = |args: &[&str]| -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
        env(&mut command);
        let output = command.args(args).arg("--state-root").arg(&state).output().expect("run goals");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let json = |text: &str| -> serde_json::Value { serde_json::from_str(text).expect("JSON report") };

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let _guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..400 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }

    // the record these three goals need, made through the ordinary user command: no model, no tokens spent
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let mut rpc = Rpc::connect(&socket);
    let exhausted = rpc.command(
        "mk-exhausted",
        "create_goal",
        serde_json::json!({"id": "goal-exhausted", "limits": {"max_total_tokens": 0}}),
    );
    assert_eq!(exhausted["ok"], serde_json::json!(true), "{exhausted}");
    let expired =
        rpc.command("mk-expired", "create_goal", serde_json::json!({"id": "goal-expired", "deadline": now - 60.0}));
    assert_eq!(expired["ok"], serde_json::json!(true), "{expired}");
    let live = rpc.command(
        "mk-live",
        "create_goal",
        serde_json::json!({"id": "goal-live", "limits": {"max_total_tokens": 1000000}}),
    );
    assert_eq!(live["ok"], serde_json::json!(true), "{live}");

    let (code, out, stderr) = goals(&["goals"]);
    assert_eq!(code, 0, "{stderr}");
    let line = |id: &str| {
        out.lines().find(|l| l.contains(id)).unwrap_or_else(|| panic!("no line for {id}: {out}")).to_string()
    };
    assert!(
        line("goal-exhausted").contains("cannot accept new work: ceiling reached (0/0)"),
        "the ceiling the runtime refuses on is shown: {out}"
    );
    assert!(
        line("goal-expired").contains("cannot accept new work: deadline passed"),
        "the passed deadline the runtime refuses on is shown: {out}"
    );
    let live = line("goal-live");
    assert!(!live.contains("cannot accept new work"), "a goal with room is not marked: {live}");
    assert!(live.contains("tokens: 0/1000000"), "and its budget is visible: {live}");

    // the JSON report keeps the read's own field names: the marker is what a client renders, not protocol
    let (code, out, stderr) = goals(&["goals", "--json"]);
    assert_eq!(code, 0, "{stderr}");
    for row in json(&out)["goals"].as_array().cloned().unwrap_or_default() {
        let mut keys: Vec<&str> = row.as_object().expect("a goal row").keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            ["attached_instances", "deadline", "id", "known_usage", "limits", "status", "unknown_usage"],
            "the goal row's shape is unchanged: {row}"
        );
    }
}

/// `[limits]` in the user config bounds every goal the session creates (D-64): the
/// usage ceiling travels on the goal's `limits` and the deadline is an absolute
/// timestamp the bootstrap derives from the configured minutes. The daemon really
/// applies them: a tiny ceiling parks the leader with the budget as the reason
/// instead of letting the session run.
#[test]
fn configured_limits_reach_the_goal_and_really_bound_the_session() {
    let root = Scratch::new("limits");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    let config = |limits: &str| {
        std::fs::write(
            config_home.join("teamagents/config.toml"),
            format!(
                "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
                 api_key_env = \"TA_LIMITS_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\n{limits}"
            ),
        )
        .unwrap();
    };
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_LIMITS_KEY", "test-value");
    };
    // doctor says what bounds the goals (nothing, when nothing is configured)
    config("");
    let mut doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut doctor);
    let doctor = doctor.arg("doctor").output().expect("run doctor");
    let text = format!("{}{}", String::from_utf8_lossy(&doctor.stdout), String::from_utf8_lossy(&doctor.stderr));
    assert!(text.contains("[WARN] goal limits"), "an unbounded session is reported: {text}");
    assert!(text.contains("runs until you stop it"), "{text}");

    // a ceiling and a deadline: doctor reports both and the goal carries both
    config("[limits]\nmax_total_tokens = 400000\ndeadline_minutes = 15\n");
    let mut doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut doctor);
    let doctor = doctor.arg("doctor").output().expect("run doctor");
    let text = format!("{}{}", String::from_utf8_lossy(&doctor.stdout), String::from_utf8_lossy(&doctor.stderr));
    assert!(text.contains("max_total_tokens=400000, deadline_minutes=15"), "{text}");
    // a zero is a config error, not a silent default
    config("[limits]\nmax_total_tokens = 0\n");
    let mut doctor = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut doctor);
    let doctor = doctor.arg("doctor").output().expect("run doctor");
    let text = format!("{}{}", String::from_utf8_lossy(&doctor.stdout), String::from_utf8_lossy(&doctor.stderr));
    assert!(text.contains("[FAIL] user config"), "{text}");
    assert!(text.contains("max_total_tokens must be a positive"), "{text}");

    config("[limits]\nmax_total_tokens = 400000\ndeadline_minutes = 15\n");
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let db = state.join("session.sqlite");
    let mut row = None;
    for _ in 0..400 {
        if let Ok(control) = teamagents_core::v2::Control::open(&db, "test", false) {
            row = control
                .connection()
                .query_row("SELECT limits_json, deadline FROM goals LIMIT 1", [], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?))
                })
                .ok();
            if row.is_some() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let (limits, deadline) = row.expect("the bootstrap creates the goal");
    let limits: serde_json::Value = serde_json::from_str(&limits).unwrap();
    assert_eq!(limits["max_total_tokens"], serde_json::json!(400000), "{limits}");
    assert!(
        limits.get("deadline_minutes").is_none(),
        "the duration is consumed by the bootstrap, never stored on the goal: {limits}"
    );
    let deadline = deadline.expect("a configured deadline lands on the goal");
    let expected = before + 15.0 * 60.0;
    assert!((deadline - expected).abs() < 120.0, "the deadline is ~15 minutes out: {deadline} vs {expected}");
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// The ceiling is not decoration: a goal whose budget cannot cover even one request
/// parks the instance with the budget as the reason (A18) instead of running.
#[test]
fn a_tiny_configured_ceiling_parks_the_session_instead_of_running_it() {
    let root = Scratch::new("limit-tiny");
    let (config_home, state, ws) = (root.join("config"), root.join("root"), root.join("ws"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_TINY_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n\n\
         [limits]\nmax_total_tokens = 4\n",
    )
    .unwrap();
    let env = |command: &mut Command| {
        command
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("TA_TINY_KEY", "test-value");
    };
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    env(&mut daemon);
    let daemon = daemon
        .args(["daemon", "--state-root"])
        .arg(&state)
        .arg("--cwd")
        .arg(&ws)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("start the daemon");
    let mut daemon_guard = Daemon(daemon);
    let socket = state.join("daemon.sock");
    for _ in 0..400 {
        if socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let mut rpc = Rpc::connect(&socket);
    // wait for the bootstrap: the socket exists before the leader does
    for _ in 0..400 {
        let checkpoint = rpc.call("checkpoint", serde_json::json!({}));
        if checkpoint["snapshot"]["instances"].as_array().is_some_and(|instances| !instances.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    rpc.command(
        "limits-input",
        "submit_input",
        serde_json::json!({"instance_id": "i-leader", "envelope_id": "env-limits", "text": "do something"}),
    );
    // Read the persisted state directly (as the other daemon tests do): the park and
    // its reason are facts in the session database, not a client's view of them.
    let db = state.join("session.sqlite");
    let mut parked = false;
    let mut reason = String::new();
    for _ in 0..600 {
        if let Ok(control) = teamagents_core::v2::Control::open(&db, "test", false) {
            let lifecycle: Option<String> = control
                .connection()
                .query_row("SELECT lifecycle FROM instances WHERE id = 'i-leader'", [], |row| row.get(0))
                .ok();
            reason = control
                .connection()
                .query_row(
                    "SELECT payload_json FROM events WHERE kind = 'instance_lifecycle' ORDER BY sequence DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .ok()
                .and_then(|payload| serde_json::from_str::<serde_json::Value>(&payload).ok())
                .and_then(|payload| payload["reason"].as_str().map(str::to_string))
                .unwrap_or_default();
            if lifecycle.as_deref() == Some("PARKED") && reason.contains("budget") {
                parked = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(parked, "a 4-token ceiling parks the leader instead of running it: {reason:?}");
    assert!(reason.contains("budget"), "and the reason names the budget: {reason}");
    daemon_guard.stop();
    std::fs::remove_dir_all(&root).expect("the state root goes away with its daemon");
}

/// D-150: the daemon users actually have is the *detached* one (`teamagents`/`exec` start it, §1), and a
/// detached process has no terminal to Ctrl-C in — so the only stop a user can perform is `kill <pid>`, i.e.
/// **SIGTERM**. It has to reach the same shutdown Ctrl-C does; before this the daemon died by default action,
/// skipping the designed shutdown ("freezes new dispatch, persists pending work and then stops itself") and
/// leaving its socket behind.
///
/// D-288: A33 claimed *both* signals it advertises reach that shutdown ("SIGINT and SIGTERM both exit 0 with
/// `stopping...` and remove the socket") and the message the daemon prints when it starts names Ctrl-C too, but
/// only SIGTERM was ever driven — the SIGINT arm (`cli.rs`'s `ctrl_c()`) was cited by nothing. The contract
/// lives here once so each signal is a test of its own.
fn daemon_stops_gracefully_when_signalled(signal: &str) {
    use std::io::Read;
    let root = Scratch::new(&format!("daemon-signal-{}", signal.to_lowercase()));
    let (config_home, state) = (root.join("config"), root.join("state/teamagents/v2"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_TERM_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    command
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("TA_TERM_KEY", "test-value");
    let mut daemon = command
        .args(["daemon", "--state-root"])
        .arg(&state)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("start the daemon");
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(socket.exists(), "the daemon must listen before it is signalled");

    // by pid — what `kill <pid>` / Ctrl-C sends. The daemon is this test's own child, so no pattern is needed.
    let signalled =
        Command::new("kill").args(["-s", signal, &daemon.id().to_string()]).status().expect("send the signal");
    assert!(signalled.success(), "kill -s {signal} must reach the daemon");
    let started = std::time::Instant::now();
    let status = daemon.wait().expect("the daemon exits");
    assert_eq!(status.code(), Some(0), "a {signal} stop is the designed shutdown, not a signal death: {status:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "the stop must be bounded");
    assert!(!socket.exists(), "the shutdown removes the socket: {socket:?}");
    let mut stderr = String::new();
    daemon.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    assert!(stderr.contains("stopping..."), "the shutdown says what it is doing: {stderr}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_daemon_stops_gracefully_on_sigterm() {
    daemon_stops_gracefully_when_signalled("TERM");
}

/// D-288: the other signal A33's claim names — Ctrl-C in the foreground, which the startup message advertises.
/// Without the `ctrl_c()` arm the default action kills the process (no exit code) and leaves the socket, so this
/// test fails rather than passing on a weaker assertion.
#[test]
fn a_daemon_stops_gracefully_on_sigint() {
    daemon_stops_gracefully_when_signalled("INT");
}

/// D-253: the artifact census and the on-demand half of §4.3's collection — the cadence §4.4 left at a driver's
/// boot, so a state root whose last driver never boots again kept its DELETING rows and their bytes.
///
/// The test seeds the one state the collector exists for (a LIVE artifact nobody references) through the
/// product's own commands — the state a crash can leave, not one the current staging paths produce — and then
/// measures the three shapes: the census names the bytes and the missing owner, the sweep frees them, and the
/// sweep refuses while a session holds §6.1's coordinator lock.
#[test]
fn artifacts_census_and_gc_free_what_nothing_references() {
    use serde_json::json;
    use teamagents_core::v2::{Command as ControlCommand, Control, Identity};
    let root = Scratch::new("artifacts");
    let (config_home, state) = (root.join("config"), root.join("state/teamagents/v2"));
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::create_dir_all(state.join("artifacts")).unwrap();
    let bytes = state.join("artifacts/orphan.log");
    std::fs::write(&bytes, "x".repeat(4096)).unwrap();
    // staged without an owner, then published: LIVE and unreferenced — the collector's only candidate
    let mut control = Control::open(&state.join("session.sqlite"), "s-main", true).expect("open the store");
    for (id, method, params) in [
        (
            "t-stage",
            "artifact_stage",
            json!({"id": "orphan", "digest": "d", "size": 4096, "kind": "tool_output",
                                            "owner_scope": "operation", "storage_ref": bytes.to_string_lossy(),
                                            "owner_ref": null}),
        ),
        ("t-publish", "artifact_publish", json!({"id": "orphan"})),
    ] {
        control
            .submit(ControlCommand { command_id: id.into(), method: method.into(), params }, Identity::System)
            .unwrap_or_else(|error| panic!("{method}: {error}"));
    }
    drop(control);
    let run = |args: Vec<String>| -> (Option<i32>, String, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(&args)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", root.join("state"))
            .output()
            .expect("run teamagents");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    let call = |verb: &str| {
        run(vec![
            "artifacts".into(),
            verb.into(),
            "--json".into(),
            "--state-root".into(),
            state.to_string_lossy().into_owned(),
        ])
    };
    // the census: one artifact, its bytes present, no owner, and the size it claims
    let (code, out, err) = call("list");
    assert_eq!(code, Some(0), "{out}{err}");
    let listed: serde_json::Value = serde_json::from_str(&out).expect("JSON report");
    assert_eq!(listed["count"], json!(1), "{listed}");
    assert_eq!(listed["bytes"], json!(4096), "{listed}");
    assert_eq!(listed["artifacts"][0]["owner_ref"], json!(null), "{listed}");
    assert_eq!(listed["artifacts"][0]["bytes_present"], json!(true), "{listed}");
    // D-253's second half: a *read* verb must not need write access. A root this process cannot write (an
    // `EVIDENCE` root on read-only media, a root another user owns) still lists — the census opens the store
    // read-only — while the sweep says what it cannot do instead of pretending.
    // (the *directory* is what cannot be written: a WAL writer has to be able to create the side files, which is
    // the shape the sandbox produced when this was first run by hand)
    use std::os::unix::fs::PermissionsExt;
    // A root this process cannot write: no shared-memory file (removed here, so the fixture is deterministic) and
    // a directory that forbids creating one — the state SQLite cannot attach a write-ahead log from, which is why
    // the refusal is a *named* one instead of SQLite's raw "attempt to write a readonly database".
    let _ = std::fs::remove_file(state.join("session.sqlite-wal"));
    let _ = std::fs::remove_file(state.join("session.sqlite-shm"));
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o555)).unwrap();
    let listed = call("list");
    let swept = call("gc");
    // restore *before* asserting: a failing assertion must not leave a directory nothing can remove
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_ne!(listed.0, Some(0), "an unattachable log is refused, not under-reported: {listed:?}");
    assert!(listed.2.contains("shared-memory"), "the census refusal names the shape: {listed:?}");
    assert_ne!(swept.0, Some(0), "a sweep cannot run where the root cannot be written: {swept:?}");

    // a live session owns the state root: the sweep refuses and names the levers
    let held = teamagents_engine::jobs::state_lock(&state.join("coordinator.lock")).expect("take the lock");
    let (code, out, err) = call("gc");
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("already has a coordinator"), "{err}");
    assert!(err.contains("daemon --stop"), "the refusal names the stop lever (D-248): {err}");
    drop(held);
    // with the lock free the sweep collects it: the row and the bytes both go
    let (code, out, err) = call("gc");
    assert_eq!(code, Some(0), "{out}{err}");
    let swept: serde_json::Value = serde_json::from_str(&out).expect("JSON report");
    assert_eq!(swept["collected"], json!(["orphan"]), "{swept}");
    assert_eq!(swept["freed_bytes"], json!(4096), "{swept}");
    assert!(!bytes.exists(), "the bytes are gone");
    let (_, out, _) = call("list");
    let after: serde_json::Value = serde_json::from_str(&out).expect("JSON report");
    assert_eq!(after["count"], json!(0), "{after}");
    // and a root that never held a session says so instead of inventing a database
    let (code, _, err) =
        run(vec!["artifacts".into(), "--state-root".into(), root.join("nowhere").to_string_lossy().into_owned()]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("never held a session"), "{err}");
    let _ = std::fs::remove_dir_all(&root);
}

/// D-248: the session's stop lever, which D-150 left to `ps` and a pid. `teamagents daemon --stop` addresses the
/// daemon by the state root's socket — the socket *is* the identity, so there is no pid file, no stale-pid
/// question and nothing to guess — asks it over the protocol, and waits for the socket to go. The three shapes
/// are all answers, not failures: a session that runs is stopped; a session that does not, and a socket a
/// crashed daemon left behind, are both reported as nothing running (0), and the lever never removes a socket
/// (the next client that *starts* a daemon replaces it). The refusing half is D-73's rule for a verb: the flags
/// of the *starting* shape are named, not ignored.
#[test]
fn daemon_stop_stops_the_session_by_its_socket() {
    let root = Scratch::new("daemon-stop");
    let (config_home, state_home) = (root.join("config"), root.join("state"));
    let state = state_home.join("teamagents/v2");
    std::fs::create_dir_all(config_home.join("teamagents")).unwrap();
    std::fs::write(
        config_home.join("teamagents/config.toml"),
        "[models.leader_main]\nprovider = \"openai\"\nmodel = \"test\"\n\
         api_key_env = \"TA_STOP_KEY\"\nbase_url = \"http://127.0.0.1:1/v1\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| -> (Option<i32>, String, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_teamagents"))
            .args(args)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_STATE_HOME", &state_home)
            .env("TA_STOP_KEY", "test-value")
            .output()
            .expect("run teamagents");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };

    // nothing is running yet: that is an answer, not a failure
    let (code, out, err) = run(&["daemon", "--stop"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("no daemon is running"), "{out}");

    // a flag of the *starting* shape is named rather than ignored, and a refused stop starts nothing
    for (flag, extra) in [("--cwd", vec!["/tmp"]), ("--model", vec!["leader_main"]), ("--full-auto", vec![])] {
        let mut args = vec!["daemon", "--stop", flag];
        args.extend(extra);
        let (code, out, err) = run(&args);
        assert_eq!(code, Some(2), "{flag} must be refused, not ignored: {out}{err}");
        assert!(err.contains(flag), "the refusal names {flag}: {out}{err}");
        assert!(!state.join("daemon.sock").exists(), "a refused stop must not start a session: {out}{err}");
    }

    // a real detached daemon, started the way §1 says and stopped by the lever
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_teamagents"));
    daemon.env("XDG_CONFIG_HOME", &config_home).env("XDG_STATE_HOME", &state_home).env("TA_STOP_KEY", "test-value");
    let mut daemon_guard = Daemon(
        daemon
            .args(["daemon", "--state-root"])
            .arg(&state)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("start the daemon"),
    );
    let socket = state.join("daemon.sock");
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(socket.exists(), "the daemon must listen before it is stopped");

    let state_arg = state.to_string_lossy().into_owned();
    let (code, out, err) = run(&["daemon", "--stop", "--state-root", &state_arg]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("stopped the session"), "{out}");
    assert!(!socket.exists(), "the socket goes with the accept loop: {socket:?}");
    // the daemon itself reaches the designed shutdown (D-150's "stopping…", a zero exit), not a signal death
    let status = daemon_guard.0.wait().expect("the daemon exits");
    assert_eq!(status.code(), Some(0), "the lever reaches the designed shutdown: {status:?}");
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut daemon_guard.0.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(stderr.contains("stopping..."), "the shutdown says what it is doing: {stderr}");

    // and stopping again is still not a failure
    let (code, out, _) = run(&["daemon", "--stop", "--state-root", &state_arg]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("no daemon is running"), "{out}");
    let _ = std::fs::remove_dir_all(&root);
}
