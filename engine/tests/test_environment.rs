//! The test fixture must restore process state even when a test panics.

mod support;

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

#[test]
fn test_environment_restores_values_and_cleans_up_after_unwinding() {
    let state = std::env::var_os("XDG_STATE_HOME");
    let config = std::env::var_os("XDG_CONFIG_HOME");
    let existing = "TA_FIXTURE_EXISTING";
    let absent = "TA_FIXTURE_ABSENT";
    let original_existing = std::env::var_os(existing);
    let original_absent = std::env::var_os(absent);
    let non_unicode = OsString::from_vec(vec![0xff, b'x']);
    std::env::set_var(existing, &non_unicode);
    std::env::remove_var(absent);

    let mut root = None;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut env = support::isolated_state_home("fixture-unwind");
        root = Some(env.to_path_buf());
        assert_eq!(std::env::var_os("XDG_STATE_HOME").as_deref(), Some(env.as_os_str()));
        assert_eq!(std::env::var_os("XDG_CONFIG_HOME"), Some(env.join("config").into_os_string()));
        env.set(existing, "first");
        env.set(existing, "second");
        env.set(absent, "temporary");
        std::fs::write(env.join("artifact"), "temporary").unwrap();
        panic!("simulate a failing test");
    }));
    assert!(panic.is_err());
    assert!(!root.unwrap().exists());
    assert_eq!(std::env::var_os("XDG_STATE_HOME"), state);
    assert_eq!(std::env::var_os("XDG_CONFIG_HOME"), config);
    assert_eq!(std::env::var_os(existing), Some(non_unicode));
    assert!(std::env::var_os(absent).is_none());

    // A poisoned lock from the failed test must not break the next test.
    let env = support::isolated_state_home("fixture-next");
    assert!(env.is_dir());
    drop(env);
    for (key, value) in [(existing, original_existing), (absent, original_absent)] {
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}
