//! Trusted evaluation staging; candidate code executes only through bwrap.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use teamagents_engine::tools;

const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_TREE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TREE_FILES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
struct InputFile {
    bytes: Vec<u8>,
    mode: u32,
}

type Files = BTreeMap<PathBuf, InputFile>;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RepositoryFixture {
    revision: String,
    paths: Vec<PathBuf>,
}

#[derive(serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Grading {
    manifest: PathBuf,
    public_tests: Vec<String>,
    timeout_seconds: u64,
    config_home: Option<PathBuf>,
}

impl Default for Grading {
    fn default() -> Self {
        Self { manifest: PathBuf::from("Cargo.toml"), public_tests: vec![], timeout_seconds: 120, config_home: None }
    }
}

fn normal_path(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|part| matches!(part, Component::Normal(_)))
}

fn load_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = regular_bytes(path)?;
    toml::from_str(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

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

fn read_tree(root: &Path, ignored: &BTreeSet<PathBuf>) -> Result<Files, String> {
    fn visit(
        root: &Path,
        relative: &Path,
        ignored: &BTreeSet<PathBuf>,
        files: &mut Files,
        bytes: &mut u64,
    ) -> Result<(), String> {
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
            if ignored.contains(&path) {
                continue;
            }
            if metadata.is_dir() {
                if path.components().count() > 32 {
                    return Err("目录层级超过评分上限".into());
                }
                visit(root, &path, ignored, files, bytes)?;
            } else {
                let contents = regular_bytes(&entry.path())?;
                *bytes += contents.len() as u64;
                if *bytes > MAX_TREE_BYTES || files.len() >= MAX_TREE_FILES {
                    return Err("候选文件树超过评分大小上限".into());
                }
                files.insert(path, InputFile { bytes: contents, mode: metadata.permissions().mode() & 0o777 });
            }
        }
        Ok(())
    }
    let mut files = Files::new();
    visit(root, Path::new(""), ignored, &mut files, &mut 0)?;
    Ok(files)
}

fn git_output(args: &[&str]) -> Result<Vec<u8>, String> {
    let manifest = fs::canonicalize(env!("CARGO_MANIFEST_DIR")).map_err(|e| e.to_string())?;
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest.parent().ok_or("找不到仓库根")?)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("不能读取固定 Git 输入：{}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(output.stdout)
}

fn fixture_files(task: &Path) -> Result<Files, String> {
    let source = task.join("fixture-source.toml");
    let mut files = Files::new();
    if source.exists() {
        let spec: RepositoryFixture = load_toml(&source)?;
        if spec.revision.len() != 40
            || !spec.revision.bytes().all(|b| b.is_ascii_hexdigit())
            || spec.paths.is_empty()
            || !spec.paths.iter().all(|path| normal_path(path))
        {
            return Err("Git 夹具必须指定完整 40 位提交及非空相对路径列表".into());
        }
        let commit = git_output(&["rev-parse", "--verify", &format!("{}^{{commit}}", spec.revision)])?;
        if String::from_utf8_lossy(&commit).trim() != spec.revision {
            return Err("Git 夹具 revision 必须直接指向提交".into());
        }
        let listing = git_output(&["ls-tree", "-rlz", "--full-tree", &spec.revision])?;
        let mut total = 0;
        for entry in listing.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
            let entry = std::str::from_utf8(entry).map_err(|e| e.to_string())?;
            let (header, path) = entry.split_once('\t').ok_or("Git 输入条目缺少路径")?;
            let path = PathBuf::from(path);
            if !spec.paths.iter().any(|selected| path.starts_with(selected)) {
                continue;
            }
            let fields: Vec<_> = header.split_whitespace().collect();
            if fields.len() != 4 || fields[1] != "blob" || !matches!(fields[0], "100644" | "100755") {
                return Err(format!("Git 夹具拒绝符号链接、子模块或特殊文件：{}", path.display()));
            }
            let size: u64 = fields[3].parse().map_err(|_| "Git 输入大小无效")?;
            total += size;
            if !normal_path(&path) || size > MAX_FILE_BYTES || total > MAX_TREE_BYTES || files.len() >= MAX_TREE_FILES {
                return Err("Git 夹具超过路径或大小上限".into());
            }
            let bytes = git_output(&["cat-file", "blob", fields[2]])?;
            if bytes.len() as u64 != size {
                return Err("Git 夹具读取不完整".into());
            }
            files.insert(path, InputFile { bytes, mode: if fields[0] == "100755" { 0o755 } else { 0o644 } });
        }
        if spec.paths.iter().any(|selected| !files.keys().any(|path| path.starts_with(selected))) {
            return Err("Git 夹具选择的路径不存在".into());
        }
    }
    // Optional explicit user inputs are applied before staging or grading.
    // Uncommitted files in the source repository never become fixture inputs.
    if task.join("fixture").exists() {
        files.extend(read_tree(&task.join("fixture"), &BTreeSet::new())?);
    }
    if files.is_empty() {
        return Err("评测夹具为空".into());
    }
    if files.len() > MAX_TREE_FILES || files.values().map(|file| file.bytes.len() as u64).sum::<u64>() > MAX_TREE_BYTES
    {
        return Err("合并后的评测夹具超过评分大小上限".into());
    }
    Ok(files)
}

fn grading(task: &Path, fixture: &Files) -> Result<Grading, String> {
    let path = task.join("grading.toml");
    let config: Grading = if path.exists() { load_toml(&path)? } else { Grading::default() };
    if !normal_path(&config.manifest)
        || config.manifest.file_name().and_then(|name| name.to_str()) != Some("Cargo.toml")
        || !fixture.contains_key(&config.manifest)
        || !(1..=600).contains(&config.timeout_seconds)
        || !config
            .public_tests
            .iter()
            .all(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
    {
        return Err("评分配置的 Cargo 清单、测试名称或时限无效".into());
    }
    if config.config_home.as_ref().is_some_and(|path| {
        !normal_path(path) || fixture.contains_key(path) || !fixture.keys().any(|file| file.starts_with(path))
    }) {
        return Err("评分 config_home 必须是固定输入内的相对配置目录".into());
    }
    Ok(config)
}

fn allowed_files(task: &Path, fixture: &Files) -> Result<BTreeSet<PathBuf>, String> {
    let bytes = regular_bytes(&task.join("allowed-files.txt"))?;
    let text = std::str::from_utf8(&bytes).map_err(|e| format!("允许文件列表不是 UTF-8：{e}"))?;
    let mut allowed = BTreeSet::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let path = Path::new(line);
        if !normal_path(path)
            || !path.components().any(|part| part == Component::Normal(std::ffi::OsStr::new("src")))
            || path.extension().and_then(|x| x.to_str()) != Some("rs")
            || !fixture.contains_key(path)
        {
            return Err(format!("允许修改的路径必须是 fixture 中现有的 Rust src/ 源文件：{line}"));
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

fn write_snapshot(root: &Path, files: &Files) -> Result<(), String> {
    for (path, file) in files {
        write_file(root, path, &file.bytes)?;
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(file.mode)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn prepare(task: &Path, candidate: &Path, hashes: &mut Value) -> Result<(Scratch, Grading), String> {
    let mut fixture = fixture_files(task)?;
    let config = grading(task, &fixture)?;
    let allowed = allowed_files(task, &fixture)?;
    let mut ignored = BTreeSet::from([PathBuf::from("target"), PathBuf::from(".git")]);
    for path in fixture.keys().filter(|path| path.file_name().and_then(|name| name.to_str()) == Some("Cargo.toml")) {
        ignored.insert(path.parent().ok_or("Cargo 清单没有父路径")?.join("target"));
    }
    let submitted = read_tree(candidate, &ignored)?;
    *hashes = Value::Object(
        allowed
            .iter()
            .filter_map(|path| {
                submitted.get(path).map(|file| {
                    (path.to_string_lossy().into_owned(), json!(format!("{:x}", Sha256::digest(&file.bytes))))
                })
            })
            .collect(),
    );
    for (path, expected) in &fixture {
        let actual = submitted.get(path).ok_or_else(|| format!("候选删除了 fixture 文件：{}", path.display()))?;
        if !allowed.contains(path) && actual != expected {
            return Err(format!("候选修改了受保护文件：{}", path.display()));
        }
        if actual.mode != expected.mode {
            return Err(format!("候选修改了文件权限：{}", path.display()));
        }
    }
    for path in submitted.keys() {
        if !fixture.contains_key(path) {
            return Err(format!("候选新增了未允许的文件：{}", path.display()));
        }
    }
    let hidden_path = config.manifest.parent().ok_or("Cargo 清单没有父路径")?.join("tests/hidden.rs");
    if fixture.contains_key(&hidden_path) {
        return Err(format!("fixture 不能占用评分路径 {}", hidden_path.display()));
    }
    let hidden = regular_bytes(&task.join("hidden_tests.rs"))?;
    let scratch = Scratch::new()?;
    for path in allowed {
        fixture.insert(path.clone(), submitted[&path].clone());
    }
    write_snapshot(&scratch.0, &fixture)?;
    write_file(&scratch.0, &hidden_path, &hidden)?;
    Ok((scratch, config))
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
        let (scratch, config) = prepare(task, candidate, &mut hashes)?;
        // shell_run appends a nonzero exit marker after the captured output,
        // including when that output is truncated. No candidate code runs here
        // on the host, and no shell state is restored from the candidate tree.
        let manifest = format!("'{}'", config.manifest.to_string_lossy().replace('\'', "'\\''"));
        // Old fixtures may require user-style configuration. Select only an
        // explicit directory copied from trusted input; never import host HOME.
        let environment = config.config_home.as_ref().map_or_else(String::new, |path| {
            format!("XDG_CONFIG_HOME='{}' ", scratch.0.join(path).to_string_lossy().replace('\'', "'\\''"))
        });
        let cargo = format!("{environment}cargo test --offline --manifest-path {manifest}");
        let public = if config.public_tests.is_empty() {
            "--all-targets".into()
        } else {
            config.public_tests.iter().map(|name| format!("--test {name}")).collect::<Vec<_>>().join(" ")
        };
        for (command, hidden) in [
            (format!("{cargo} --test hidden -- --nocapture --color never --test-threads=1"), true),
            (format!("{cargo} {public} -- --color never --test-threads=1"), false),
        ] {
            output.push_str(&format!("$ {command}\n"));
            match tools::shell_run(&command, &scratch.0, config.timeout_seconds, false, None) {
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

/// Materialize only trusted input files; never copy hidden tests to the model.
#[test]
#[ignore = "仅由评测 runner 指定任务和全新工作目录后运行"]
fn stage_fixture() {
    let task = PathBuf::from(std::env::var_os("TA_EVAL_TASK_DIR").expect("TA_EVAL_TASK_DIR"));
    let target = PathBuf::from(std::env::var_os("TA_EVAL_STAGE_OUTPUT").expect("TA_EVAL_STAGE_OUTPUT"));
    assert!(fs::read_dir(&target).expect("读取全新工作目录").next().is_none(), "拒绝覆盖非空评测工作目录");
    let files = fixture_files(&task).expect("读取固定夹具");
    grading(&task, &files).expect("校验评分配置");
    write_snapshot(&target, &files).expect("写入固定夹具");
    println!("已准备 {} 个输入文件；未复制隐藏测试。", files.len());
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
    write_snapshot(&candidate, &fixture_files(&task).unwrap()).unwrap();
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

#[test]
fn hidden_grader_uses_only_explicit_fixture_configuration() {
    let (_root, task, candidate) = probe_fixture();
    let config_home = Path::new("test config's");
    write_file(&task.join("fixture"), &config_home.join("eval/marker"), b"fixture-only").unwrap();
    write_file(&candidate, &config_home.join("eval/marker"), b"fixture-only").unwrap();
    write_file(&candidate, Path::new("src/lib.rs"), b"pub fn add(a: i32, b: i32) -> i32 { a + b }").unwrap();
    write_file(
        &task,
        Path::new("hidden_tests.rs"),
        br#"
#[test]
fn fixture_configuration_is_available_and_home_stays_private() {
    assert_eq!(eval_probe::add(5, 3), 8);
    let config = std::path::PathBuf::from(std::env::var("XDG_CONFIG_HOME").expect("explicit fixture config"));
    assert!(config.is_absolute());
    assert_eq!(std::fs::read_to_string(config.join("eval/marker")).unwrap(), "fixture-only");
    assert_ne!(std::path::PathBuf::from(std::env::var("HOME").unwrap()), std::env::current_dir().unwrap());
}
"#,
    )
    .unwrap();
    let absent = grade(&task, &candidate);
    assert_eq!(absent["ok"], false, "configuration is not selected implicitly: {absent}");
    if !tools::bwrap_available() {
        assert!(absent["reason"].as_str().unwrap().contains("隔离不可用"), "{absent}");
        eprintln!("skipped: bwrap is unavailable; fixture configuration was not executed");
        return;
    }
    assert!(absent["output"].as_str().unwrap().contains("explicit fixture config"), "{absent}");
    write_file(&task, Path::new("grading.toml"), b"config_home = \"test config's\"\n").unwrap();
    let selected = grade(&task, &candidate);
    assert_eq!(selected["ok"], true, "{selected}");
    // Configuration is protected input, even when a candidate wants to change it.
    write_file(&candidate, &config_home.join("eval/marker"), b"candidate-changed").unwrap();
    let changed = grade(&task, &candidate);
    assert_eq!(changed["ok"], false, "{changed}");
    assert!(changed["reason"].as_str().unwrap().contains("受保护文件"), "{changed}");
    assert_eq!(changed["output"], "");
}

#[test]
fn hidden_grader_rejects_configuration_outside_the_fixture() {
    let (_root, task, candidate) = probe_fixture();
    for path in ["../private", "/tmp", ".", "Cargo.toml", "missing", "src/../private"] {
        write_file(&task, Path::new("grading.toml"), format!("config_home = {path:?}\n").as_bytes()).unwrap();
        let report = grade(&task, &candidate);
        assert_eq!(report["ok"], false, "{path}: {report}");
        assert!(report["reason"].as_str().unwrap().contains("config_home"), "{report}");
        assert_eq!(report["output"], "", "configuration must be checked before candidate execution");
    }
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

#[test]
fn hidden_grader_handles_nested_crates_without_ignoring_unrelated_files() {
    let (root, task, candidate) = probe_fixture();
    let nested = Path::new("engine space's");
    fs::rename(task.join("fixture"), root.0.join("original")).unwrap();
    let original = read_tree(&root.0.join("original"), &BTreeSet::new()).unwrap();
    write_snapshot(&task.join("fixture").join(nested), &original).unwrap();
    write_file(
        &task,
        Path::new("grading.toml"),
        b"manifest = \"engine space's/Cargo.toml\"\npublic_tests = [\"public\"]\n",
    )
    .unwrap();
    write_file(&task, Path::new("allowed-files.txt"), b"engine space's/src/lib.rs\n").unwrap();
    fs::remove_dir_all(&candidate).unwrap();
    write_snapshot(&candidate, &fixture_files(&task).unwrap()).unwrap();
    write_file(&candidate, &nested.join("src/lib.rs"), b"pub fn add(a: i32, b: i32) -> i32 { a + b }\n").unwrap();
    write_file(&candidate, &nested.join("target/poison"), b"build output").unwrap();

    let good = grade(&task, &candidate);
    if tools::bwrap_available() {
        assert_eq!(good["ok"], true, "{good}");
        assert!(good["output"].as_str().unwrap().contains("different_values"));
    } else {
        assert_eq!(good["ok"], false, "{good}");
        assert!(good["reason"].as_str().unwrap().contains("隔离不可用"));
    }
    assert!(!candidate.join(nested).join("tests/hidden.rs").exists());
    write_file(&candidate, Path::new("notes/target/protected.txt"), b"unexpected file").unwrap();
    let extra = grade(&task, &candidate);
    assert!(extra["reason"].as_str().unwrap().contains("新增了未允许的文件"), "{extra}");
    assert_eq!(extra["output"], "");
    fs::remove_dir_all(candidate.join("notes")).unwrap();
    fs::set_permissions(candidate.join(nested).join("src/lib.rs"), fs::Permissions::from_mode(0o755)).unwrap();
    let mode = grade(&task, &candidate);
    assert!(mode["reason"].as_str().unwrap().contains("文件权限"), "{mode}");
    assert_eq!(mode["output"], "");
}

#[test]
fn repository_fixture_reads_committed_inputs_and_explicit_overlay_only() {
    let (root, task, _) = probe_fixture();
    let commit = String::from_utf8(git_output(&["rev-parse", "HEAD"]).unwrap()).unwrap().trim().to_string();
    fs::write(
        task.join("fixture-source.toml"),
        format!("revision = \"{commit}\"\npaths = [\"core/Cargo.toml\", \"core/src/lib.rs\"]\n"),
    )
    .unwrap();
    let files = fixture_files(&task).unwrap();
    assert_eq!(
        files[Path::new("core/src/lib.rs")].bytes,
        git_output(&["show", &format!("{commit}:core/src/lib.rs")]).unwrap(),
        "the dirty working tree must not replace the pinned source"
    );
    let overlay = b"explicit user input";
    write_file(&task.join("fixture"), Path::new("core/src/lib.rs"), overlay).unwrap();
    let overlaid = fixture_files(&task).unwrap();
    assert_eq!(overlaid[Path::new("core/src/lib.rs")].bytes, overlay);
    let staged = root.0.join("stage");
    write_snapshot(&staged, &overlaid).unwrap();
    assert_eq!(fs::read(staged.join("core/src/lib.rs")).unwrap(), overlay);
    assert!(!staged.join("hidden_tests.rs").exists());
    assert!(!staged.join("tests/hidden.rs").exists());

    fs::write(task.join("fixture-source.toml"), "revision = \"HEAD\"\npaths = [\"core\"]\n").unwrap();
    assert!(fixture_files(&task).unwrap_err().contains("完整 40 位提交"));
    fs::write(task.join("grading.toml"), "manifest = \"../Cargo.toml\"\n").unwrap();
    assert!(grading(&task, &files).is_err());
}
