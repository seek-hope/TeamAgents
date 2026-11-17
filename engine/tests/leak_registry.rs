//! The gate scratch registry under real parallel writers (D-528).
//!
//! It lives in its own test binary on purpose: the shared helper is included by every crate's test code and by
//! several integration suites, so a control inside it would be counted — and run — once per inclusion site.

#[path = "../../test_scratch_owner.rs"]
mod scratch_owner;

use scratch_owner::record_ownership_in;

/// Real parallel writers through the real append path: every root is recorded exactly once, and every record is
/// whole. The registry is private to this control, so the assertions describe only records this control wrote.
#[test]
fn parallel_records_are_whole_and_complete() {
    let marker = format!("concurrency-{}", std::process::id());
    let base = std::env::temp_dir().join(format!("ta-concurrency-control-{marker}"));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    // `base` is itself a guarded scratch root of this gate, so it is recorded in the gate's registry like any other.
    // The *roots this control creates* go to a private registry instead: sharing the gate's would let every
    // concurrently running test binary interleave its own records into the assertions.
    scratch_owner::record_ownership(&base);
    let registry = base.join("registry");

    let handles: Vec<_> = (0..8)
        .map(|worker| {
            let base = base.clone();
            let registry = registry.clone();
            std::thread::spawn(move || {
                let mut made = Vec::new();
                for index in 0..4 {
                    let dir = base.join(format!("root-{worker}-{index}"));
                    std::fs::create_dir_all(&dir).unwrap();
                    record_ownership_in(&registry, &dir);
                    made.push(dir);
                }
                made
            })
        })
        .collect();
    let expected: Vec<_> = handles.into_iter().flat_map(|handle| handle.join().unwrap()).collect();
    let text = std::fs::read_to_string(&registry).expect("registry written");
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();

    for dir in &expected {
        let wanted = dir.to_string_lossy();
        let seen = lines.iter().filter(|line| **line == wanted).count();
        assert_eq!(seen, 1, "{wanted} must be recorded exactly once, saw {seen}");
    }
    // A torn record is a line that names no root this control created: with one append per record, every line
    // must be complete, so the count of lines is exactly the count of roots.
    assert_eq!(
        lines.len(),
        expected.len(),
        "one line per recorded root: a torn record would add lines that name nothing"
    );
    for line in &lines {
        assert!(
            expected.iter().any(|dir| dir.to_string_lossy() == *line),
            "a record that names no root this control created is a torn or interleaved line: {line}"
        );
    }
    let _ = std::fs::remove_dir_all(&base);
}
