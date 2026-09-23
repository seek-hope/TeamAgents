//! Host Shell regressions use only temporary files and explicitly stop services.
mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use teamagents_engine::gateway::TurnControl;
use teamagents_engine::tools::{shell_run, shell_run_host};

fn host(command: &str, root: &Path, state: Option<&Path>, control: &TurnControl) -> String {
    shell_run_host(command, root, 10, None, state, control).unwrap()
}

#[test]
fn host_shell_preserves_real_files_cwd_exports_and_filters_credentials() {
    let mut env = support::TestEnv::new("host-shell-state");
    env.set("TEAMAGENTS_TEST_MODEL_SECRET", "must-not-reach-shell");
    let root = env.join("project");
    let state = env.join("state space ' $(touch BAD_STATE_PATH)");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let control = TurnControl::default();
    let outside = env.join("outside.txt");
    std::fs::write(&outside, "host-only").unwrap();
    let first = host(
        &format!(
            "cat '{}'; test -z \"${{TEAMAGENTS_TEST_MODEL_SECRET+x}}\" || exit 42; cd sub; export TA_MARK=73",
            outside.display()
        ),
        &root,
        Some(&state),
        &control,
    );
    assert!(first.contains("host-only") && !first.contains("exit 42"), "{first}");
    let second = host("pwd; echo mark=$TA_MARK; echo home=$HOME", &root, Some(&state), &control);
    assert!(second.contains(root.join("sub").to_str().unwrap()), "{second}");
    assert!(second.contains("mark=73"), "{second}");
    assert!(!second.contains("/tmp/.teamagents-shell"), "{second}");
    assert!(!root.join("BAD_STATE_PATH").exists());
    // Reopening uses the stored host snapshot, not a legacy sandbox snapshot.
    std::fs::write(state.join("state.sh"), "cd /not-a-host-directory\nexport TA_MARK=sandbox\n").unwrap();
    let reopened = host("echo mark=$TA_MARK", &root, Some(&state), &TurnControl::default());
    assert!(reopened.contains("mark=73"), "{reopened}");
    let failed = host("echo actual-output; exit 17", &root, None, &control);
    assert!(failed.contains("actual-output") && failed.contains("(exit 17)"), "{failed}");
    let large = host("seq 1 40000", &root, None, &control);
    assert!(large.contains("output truncated"), "large output must remain bounded");
}

#[test]
fn host_shell_background_pipes_do_not_hold_readers_or_lose_foreground_output() {
    let env = support::TestEnv::new("host-shell-background");
    let before = std::fs::read_dir("/proc/self/task").unwrap().count();
    let start = Instant::now();
    let result = host("sleep 2 & echo ready", &env, None, &TurnControl::default());
    assert!(result.contains("ready"), "{result}");
    assert!(start.elapsed() < Duration::from_secs(1), "an inherited stdout must not stall the caller");
    assert_eq!(std::fs::read_dir("/proc/self/task").unwrap().count(), before, "no leaked output readers");
}

#[test]
fn host_shell_timeout_and_cancellation_stop_descendants() {
    let env = support::TestEnv::new("host-shell-cancel");
    for cancel in [false, true] {
        let root = env.join(if cancel { "cancel" } else { "timeout" });
        std::fs::create_dir_all(&root).unwrap();
        let control = Arc::new(TurnControl::default());
        let thread_root = root.clone();
        let thread_control = control.clone();
        let job = std::thread::spawn(move || {
            shell_run_host(
                "sh -c 'sleep 2; touch escaped' & echo ready > ready; echo partial; wait",
                &thread_root,
                if cancel { 20 } else { 1 },
                None,
                None,
                &thread_control,
            )
        });
        assert!(support::wait_for(|| root.join("ready").exists(), 3000));
        if cancel {
            control.cancel();
        }
        let result = job.join().unwrap().unwrap_err();
        assert!(result.contains(if cancel { "interrupted" } else { "timed out" }), "{result}");
        assert!(result.contains("partial"), "{result}");
        std::thread::sleep(Duration::from_millis(2100));
        assert!(!root.join("escaped").exists(), "grandchild survived command stop");
    }
}

#[test]
fn isolation_start_failure_is_an_error_and_never_falls_back_to_host() {
    let mut env = support::TestEnv::new("shell-isolation-failure");
    let bin = env.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let bwrap = bin.join("bwrap");
    std::fs::write(&bwrap, "#!/bin/sh\necho 'bwrap: loopback: Failed RTM_NEWADDR: No child processes' >&2\nexit 1\n")
        .unwrap();
    std::fs::set_permissions(&bwrap, std::fs::Permissions::from_mode(0o755)).unwrap();
    env.set("PATH", &bin);
    let error = shell_run("touch MUST_NOT_EXECUTE", &env, 5, false, None).unwrap_err();
    assert!(error.contains("IsolationUnavailable") && error.contains("Failed RTM_NEWADDR"), "{error}");
    assert!(!env.join("MUST_NOT_EXECUTE").exists());
    // full_auto host execution is explicit; it does not depend on bwrap.
    let output = host("echo native-ok", &env, None, &TurnControl::default());
    assert_eq!(output.trim(), "native-ok");
}
