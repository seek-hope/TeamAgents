//! Trusted evaluation staging; candidate code executes only through bwrap.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use teamagents_engine::tools;

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_TREE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TREE_FILES: usize = 4096;

type Files = BTreeMap<PathBuf, Vec<u8>>;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!("ta-hidden-grade-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).map_err(|e| format!("不能创建评分临时目录：{e}"))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn regular_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("不能读取 {}：{e}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("拒绝符号链接或特殊文件：{}", path.display()));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(format!("文件超过评分大小上限：{}", path.display()));
    }
    fs::read(path).map_err(|e| format!("不能读取 {}：{e}", path.display()))
}

fn read_tree(root: &Path) -> Result<Files, String> {
    fn visit(root: &Path, relative: &Path, files: &mut Files, bytes: &mut u64) -> Result<(), String> {
        let directory = root.join(relative);
        let metadata =
            fs::symlink_metadata(&directory).map_err(|e| format!("不能读取 {}：{e}", directory.display()))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!("拒绝符号链接或非目录：{}", directory.display()));
        }
        let entries = fs::read_dir(&directory).map_err(|e| format!("不能列出 {}：{e}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("不能读取目录项：{e}"))?;
            let path = relative.join(entry.file_name());
            let metadata =
                fs::symlink_metadata(entry.path()).map_err(|e| format!("不能读取 {}：{e}", path.display()))?;
            if metadata.file_type().is_symlink() {
                return Err(format!("拒绝符号链接：{}", path.display()));
            }
            // Build products and Git metadata never enter the trusted copy.
            if relative.as_os_str().is_empty() && (path == Path::new("target") || path == Path::new(".git")) {
                continue;
            }
            if metadata.is_dir() {
                if path.components().count() > 32 {
                    return Err("目录层级超过评分上限".into());
                }
                visit(root, &path, files, bytes)?;
            } else {
                let contents = regular_bytes(&entry.path())?;
                *bytes += contents.len() as u64;
                if *bytes > MAX_TREE_BYTES || files.len() >= MAX_TREE_FILES {
                    return Err("候选文件树超过评分大小上限".into());
                }
                files.insert(path, contents);
            }
        }
        Ok(())
    }
    let mut files = Files::new();
    visit(root, Path::new(""), &mut files, &mut 0)?;
    Ok(files)
}

fn allowed_files(task: &Path, fixture: &Files) -> Result<BTreeSet<PathBuf>, String> {
    let bytes = regular_bytes(&task.join("allowed-files.txt"))?;
    let text = std::str::from_utf8(&bytes).map_err(|e| format!("允许文件列表不是 UTF-8：{e}"))?;
    let mut allowed = BTreeSet::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let path = Path::new(line);
        if !path.components().all(|part| matches!(part, Component::Normal(_)))
            || !path.starts_with("src")
            || path.extension().and_then(|x| x.to_str()) != Some("rs")
            || !fixture.contains_key(path)
        {
            return Err(format!("允许修改的路径必须是 fixture 中现有的 src/*.rs：{line}"));
        }
        if !allowed.insert(path.to_path_buf()) {
            return Err(format!("允许文件列表有重复项：{line}"));
        }
    }
    if allowed.is_empty() {
        return Err("允许文件列表为空".into());
    }
    Ok(allowed)
}

fn write_file(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), String> {
    let destination = root.join(path);
    fs::create_dir_all(destination.parent().ok_or("评分文件没有父目录")?).map_err(|e| e.to_string())?;
    fs::write(destination, bytes).map_err(|e| format!("不能写入评分文件：{e}"))
}

fn prepare(task: &Path, candidate: &Path, hashes: &mut Value) -> Result<Scratch, String> {
    let fixture = read_tree(&task.join("fixture"))?;
    let allowed = allowed_files(task, &fixture)?;
    let submitted = read_tree(candidate)?;
    *hashes = Value::Object(
        allowed
            .iter()
            .filter_map(|path| {
                submitted
                    .get(path)
                    .map(|bytes| (path.to_string_lossy().into_owned(), json!(format!("{:x}", Sha256::digest(bytes)))))
            })
            .collect(),
    );
    for (path, expected) in &fixture {
        let actual = submitted.get(path).ok_or_else(|| format!("候选删除了 fixture 文件：{}", path.display()))?;
        if !allowed.contains(path) && actual != expected {
            return Err(format!("候选修改了受保护文件：{}", path.display()));
        }
    }
    for path in submitted.keys() {
        if !fixture.contains_key(path) {
            return Err(format!("候选新增了未允许的文件：{}", path.display()));
        }
    }
    if fixture.contains_key(Path::new("tests/hidden.rs")) {
        return Err("fixture 不能占用评分路径 tests/hidden.rs".into());
    }
    let hidden = regular_bytes(&task.join("hidden_tests.rs"))?;
    let scratch = Scratch::new()?;
    for (path, bytes) in &fixture {
        write_file(&scratch.0, path, if allowed.contains(path) { &submitted[path] } else { bytes })?;
    }
    write_file(&scratch.0, Path::new("tests/hidden.rs"), &hidden)?;
    Ok(scratch)
}

fn tests_completed(output: &str, hidden: bool) -> bool {
    let mut running = None;
    let mut suites = 0;
    let mut total_passed = 0;
    for line in output.lines().map(str::trim) {
        if let Some(count) = line
            .strip_prefix("running ")
            .and_then(|text| text.strip_suffix(" tests").or_else(|| text.strip_suffix(" test")))
            .and_then(|text| text.parse::<usize>().ok())
        {
            if running.replace(count).is_some() {
                return false;
            }
        }
        if let Some(summary) = line.strip_prefix("test result: ok. ") {
            let fields: Vec<_> = summary.split(';').map(str::trim).collect();
            let counts: Option<Vec<usize>> = [" passed", " failed", " ignored", " measured", " filtered out"]
                .iter()
                .enumerate()
                .map(|(index, suffix)| fields.get(index)?.strip_suffix(suffix)?.parse().ok())
                .collect();
            let Some(counts) = counts else {
                return false;
            };
            let [passed, failed, ignored, measured, filtered] = counts.as_slice() else {
                return false;
            };
            if *failed != 0 || running.take() != passed.checked_add(*ignored).and_then(|n| n.checked_add(*measured)) {
                return false;
            }
            if hidden && (*ignored != 0 || *measured != 0 || *filtered != 0) {
                return false;
            }
            total_passed += passed;
            suites += 1;
        }
    }
    running.is_none() && suites > 0 && (!hidden || (suites == 1 && total_passed > 0))
}

fn grade(task: &Path, candidate: &Path) -> Value {
    let mut hashes = json!({});
    let mut output = String::new();
    let result = (|| {
        let scratch = prepare(task, candidate, &mut hashes)?;
        // shell_run appends a nonzero exit marker after the captured output,
        // including when that output is truncated. No candidate code runs here
        // on the host, and no shell state is restored from the candidate tree.
        for (command, hidden) in [
            ("cargo test --offline --test hidden -- --nocapture --color never", true),
            ("cargo test --offline --all-targets -- --color never", false),
        ] {
            output.push_str(&format!("$ {command}\n"));
            match tools::shell_run(command, &scratch.0, 120, false, None) {
                Ok(text) => {
                    let failed = text
                        .trim_end()
                        .rsplit_once("\n(exit ")
                        .and_then(|(_, last)| last.strip_suffix(')'))
                        .and_then(|code| code.parse::<i32>().ok())
                        .is_some();
                    output.push_str(&text);
                    output.push('\n');
                    if failed {
                        return Err("隐藏或公开测试失败".to_string());
                    }
                    if !tests_completed(&text, hidden) {
                        return Err("测试未完整结束或测试结果缺失，不能仅凭退出码判为通过".to_string());
                    }
                }
                Err(error) => {
                    output.push_str(&error);
                    return Err("隐藏评分执行失败（隔离不可用、超时或输出错误）".into());
                }
            }
        }
        Ok::<(), String>(())
    })();
    json!({
        "ok": result.is_ok(),
        "reason": result.err().unwrap_or_else(|| "隐藏及公开测试通过，受保护文件未改变".into()),
        "candidate_sha256": hashes,
        "output": output,
    })
}

/// run.sh invokes this only after the agent exits. Ordinary Cargo test runs
/// must not accidentally grade an environment-dependent candidate directory.
#[test]
#[ignore = "仅由评测 runner 指定任务、候选目录及输出路径后运行"]
fn grade_candidate() {
    let task = PathBuf::from(std::env::var_os("TA_EVAL_TASK_DIR").expect("TA_EVAL_TASK_DIR"));
    let candidate = PathBuf::from(std::env::var_os("TA_EVAL_CANDIDATE_DIR").expect("TA_EVAL_CANDIDATE_DIR"));
    let output = PathBuf::from(std::env::var_os("TA_EVAL_GRADE_OUTPUT").expect("TA_EVAL_GRADE_OUTPUT"));
    let report = grade(&task, &candidate);
    fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).expect("写入评分 JSON");
    assert_eq!(report["ok"], true, "{}；证据：{}", report["reason"], output.display());
}

fn probe_fixture() -> (Scratch, PathBuf, PathBuf) {
    let root = Scratch::new().unwrap();
    let task = root.0.join("task");
    let candidate = root.0.join("candidate");
    write_file(
        &task,
        Path::new("fixture/Cargo.toml"),
        b"[package]\nname = \"eval_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    write_file(&task, Path::new("fixture/src/lib.rs"), b"pub fn add(a: i32, b: i32) -> i32 { a - b }\n").unwrap();
    write_file(
        &task,
        Path::new("fixture/tests/public.rs"),
        b"#[test] fn zero() { assert_eq!(eval_probe::add(0, 0), 0); }\n",
    )
    .unwrap();
    write_file(&task, Path::new("allowed-files.txt"), b"src/lib.rs\n").unwrap();
    write_file(
        &task,
        Path::new("hidden_tests.rs"),
        b"#[test] fn different_values() { assert_eq!(eval_probe::add(5, 3), 8); }\n",
    )
    .unwrap();
    for (path, bytes) in read_tree(&task.join("fixture")).unwrap() {
        write_file(&candidate, &path, &bytes).unwrap();
    }
    (root, task, candidate)
}

#[test]
fn hidden_grader_rejects_bad_code_and_accepts_correct_patch() {
    let (_root, task, candidate) = probe_fixture();
    let bad = grade(&task, &candidate);
    assert_eq!(bad["ok"], false, "{bad}");
    if !tools::bwrap_available() {
        assert_eq!(bad["reason"], "隐藏评分执行失败（隔离不可用、超时或输出错误）", "{bad}");
        eprintln!("skipped: bwrap is unavailable; verified grading fails closed, cannot execute candidate tests");
        return;
    }
    assert_eq!(bad["reason"], "隐藏或公开测试失败", "sandbox must actually execute the bad tests: {bad}");
    assert!(bad["output"].as_str().unwrap().contains("different_values"), "{bad}");
    write_file(&candidate, Path::new("src/lib.rs"), b"pub fn add(_: i32, _: i32) -> i32 { std::process::exit(0) }\n")
        .unwrap();
    let incomplete = grade(&task, &candidate);
    assert_eq!(incomplete["ok"], false, "{incomplete}");
    assert!(incomplete["reason"].as_str().unwrap().contains("测试未完整结束"), "{incomplete}");
    write_file(&candidate, Path::new("src/lib.rs"), b"pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();
    // Build outputs are excluded from both grading and the trusted copy.
    write_file(&candidate, Path::new("target/poison"), b"candidate build output").unwrap();
    let good = grade(&task, &candidate);
    assert_eq!(good["ok"], true, "{good}");
    assert!(good["candidate_sha256"]["src/lib.rs"].as_str().unwrap().len() == 64);
    assert!(!candidate.join("tests/hidden.rs").exists(), "hidden tests must stay outside the agent workspace");
}

#[test]
fn hidden_grader_rejects_added_build_script_before_execution() {
    let (_root, task, candidate) = probe_fixture();
    write_file(&candidate, Path::new("build.rs"), b"compile_error!(\"untrusted build script\");").unwrap();
    let report = grade(&task, &candidate);
    assert_eq!(report["ok"], false, "{report}");
    assert!(report["reason"].as_str().unwrap().contains("新增了未允许的文件：build.rs"), "{report}");
    assert_eq!(report["output"], "", "new files must be rejected before execution");
}

#[test]
fn hidden_grader_rejects_public_test_tampering() {
    let (_root, task, candidate) = probe_fixture();
    fs::write(candidate.join("tests/public.rs"), "").unwrap();
    let report = grade(&task, &candidate);
    assert_eq!(report["ok"], false, "{report}");
    assert!(report["reason"].as_str().unwrap().contains("受保护文件：tests/public.rs"), "{report}");
    assert_eq!(report["output"], "", "protection checks must precede candidate execution");
}

#[cfg(unix)]
#[test]
fn hidden_grader_rejects_candidate_symlinks() {
    let (_root, task, candidate) = probe_fixture();
    fs::remove_file(candidate.join("src/lib.rs")).unwrap();
    std::os::unix::fs::symlink(task.join("fixture/src/lib.rs"), candidate.join("src/lib.rs")).unwrap();
    let report = grade(&task, &candidate);
    assert_eq!(report["ok"], false, "{report}");
    assert!(report["reason"].as_str().unwrap().contains("符号链接"), "{report}");
    assert_eq!(report["output"], "");
}
