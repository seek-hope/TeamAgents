//! Real sandbox build workflows and private tool configuration/cache storage.

mod support;

use serde_json::json;
use std::path::{Path, PathBuf};
use teamagents_engine::gateway::TurnControl;
use teamagents_engine::{mcp::McpClient, tools};

fn available(commands: &[&str]) -> bool {
    if !tools::bwrap_available() {
        eprintln!("skip: bwrap is not available");
        return false;
    }
    for command in commands {
        if system_tool(command).is_none() {
            eprintln!("skip: system tool {command} is not available in the sandbox");
            return false;
        }
    }
    true
}

fn system_tool(command: &str) -> Option<PathBuf> {
    ["/usr/local/bin", "/usr/bin", "/bin"].iter().map(|dir| Path::new(dir).join(command)).find(|path| path.is_file())
}

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn run(root: &Path, state: Option<&Path>, command: &str) -> String {
    tools::shell_run_stateful(command, root, 90, false, None, state, &TurnControl::default()).unwrap()
}

fn success(root: &Path, state: Option<&Path>, command: &str) -> String {
    let output = run(root, state, command);
    assert!(!output.lines().last().unwrap_or("").starts_with("(exit "), "{output}");
    output
}

fn entries(root: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn shell_home_preserves_member_caches_without_polluting_or_sharing_the_project() {
    if !available(&["git"]) {
        return;
    }
    let mut env = support::isolated_state_home("shell-private-home");
    env.set("TA_TOOLCHAIN_TEST_SECRET", "test-only-must-not-reach-shell");
    let root = env.join("project");
    write(&root, "keep.txt", "user input\n");
    let state = env.join("member-a/shell");
    let other = env.join("member-b/shell");
    let command = r#"set -e
test -z "${TA_TOOLCHAIN_TEST_SECRET:-}"
test "$HOME" != "$PWD"
mkdir -p "$HOME/.cache"
printf member-a > "$HOME/.cache/marker"
git config --global teamagents.marker member-a
export TA_TOOLCHAIN_VALUE=keep-me
cd "$HOME"
"#;
    success(&root, Some(&state), command);
    success(
        &root,
        Some(&state),
        r#"set -e
test "$PWD" = "$HOME"
test "$TA_TOOLCHAIN_VALUE" = keep-me
test "$(cat "$HOME/.cache/marker")" = member-a
test "$(git config --global teamagents.marker)" = member-a
"#,
    );
    for own_state in [Some(other.as_path()), None] {
        success(
            &root,
            own_state,
            r#"set -e
test -d "$HOME"
test "$HOME" != "$PWD"
test ! -e "$HOME/.cache/marker"
test -z "${TA_TOOLCHAIN_VALUE:-}"
test -z "${TA_TOOLCHAIN_TEST_SECRET:-}"
mkdir -p "$HOME/.cache"
printf other > "$HOME/.cache/marker"
"#,
        );
    }
    success(&root, None, "test ! -e \"$HOME/.cache/marker\"");
    success(
        &std::env::temp_dir(),
        None,
        "set -e; test -d \"$HOME\"; test \"$HOME\" != \"$PWD\"; test ! -e \"$HOME/.cache/marker\"; mkdir -p \"$HOME/.cache\"; printf temporary > \"$HOME/.cache/marker\"",
    );
    success(&std::env::temp_dir(), None, "test ! -e \"$HOME/.cache/marker\"");
    assert_eq!(std::fs::read_to_string(state.join("home/.cache/marker")).unwrap(), "member-a");
    assert_eq!(entries(&root), ["keep.txt"]);
    assert_eq!(std::fs::read_to_string(root.join("keep.txt")).unwrap(), "user input\n");
}

#[test]
fn legacy_shell_home_migrates_but_explicit_home_and_exports_survive() {
    if !available(&[]) {
        return;
    }
    let env = support::isolated_state_home("shell-home-migration");
    let root = env.join("project");
    let state = env.join("member/shell");
    write(&root, ".npm/existing.log", "preserve old project files\n");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    write(
        &state,
        "state.sh",
        &format!(
            "cd '{}'\ndeclare -x HOME='{}'\ndeclare -x TA_TOOLCHAIN_VALUE=legacy\n",
            root.join("sub").display(),
            root.display()
        ),
    );
    success(
        &root,
        Some(&state),
        r#"set -e
test "$(basename "$PWD")" = sub
test "$HOME" != "$(dirname "$PWD")"
test "$TA_TOOLCHAIN_VALUE" = legacy
printf migrated > "$HOME/marker"
"#,
    );
    assert_eq!(std::fs::read_to_string(state.join("home/marker")).unwrap(), "migrated");
    assert_eq!(std::fs::read_to_string(root.join(".npm/existing.log")).unwrap(), "preserve old project files\n");
    success(&root, Some(&state), "export HOME=\"$(dirname \"$PWD\")\"; export TA_TOOLCHAIN_VALUE=custom");
    success(&root, Some(&state), "test \"$HOME\" = \"$(dirname \"$PWD\")\" && test \"$TA_TOOLCHAIN_VALUE\" = custom");
    assert_eq!(entries(&root), [".npm", "sub"]);
}

#[test]
fn node_project_installs_tests_and_packs_without_runtime_cache_files() {
    if !available(&["node", "npm", "tar"]) {
        return;
    }
    let env = support::isolated_state_home("node-project");
    let root = env.join("project");
    let state = env.join("member/shell");
    write(
        &root,
        "package.json",
        r#"{"name":"ta-node-project","version":"1.0.0","scripts":{"test":"node --test test/*.cjs","build":"node build.cjs"}}"#,
    );
    write(&root, "vendor/math/package.json", r#"{"name":"ta-local-math","version":"1.0.0","main":"index.cjs"}"#);
    write(&root, "vendor/math/index.cjs", "exports.add = (a, b) => a + b;\n");
    write(
        &root,
        "src/index.cjs",
        "const {add} = require('ta-local-math');\nexports.total = xs => xs.reduce(add, 1);\n",
    );
    write(&root, "test/total.cjs", "const {test} = require('node:test');\nconst assert = require('node:assert/strict');\nconst {total} = require('../src/index.cjs');\ntest('totals preserve the identity', () => { assert.equal(total([]), 0); assert.equal(total([2, 3]), 5); });\n");
    write(&root, "build.cjs", "const fs = require('node:fs');\nfs.mkdirSync('dist', {recursive: true});\nfs.copyFileSync('src/index.cjs', 'dist/index.cjs');\n");
    let output = success(
        &root,
        Some(&state),
        "set -e; node --version; npm --version; npm pack ./vendor/math --pack-destination vendor --offline --ignore-scripts; npm install --offline --ignore-scripts --no-audit --no-fund ./vendor/ta-local-math-1.0.0.tgz",
    );
    eprintln!("Node install: {output}");
    let failed = run(&root, Some(&state), "npm test");
    assert!(failed.contains("(exit 1)") && failed.contains("totals preserve"), "{failed}");
    let edit = tools::workspace_executor(root.clone(), None);
    edit(
        "edit_file",
        &json!({"path":"src/index.cjs","old_string":"xs.reduce(add, 1)","new_string":"xs.reduce(add, 0)"}),
    )
    .unwrap();
    let output = success(
        &root,
        Some(&state),
        r#"set -e
test -d "$HOME/.npm/_cacache"
npm ci --offline --ignore-scripts --no-audit --no-fund
npm test
npm run build
npm pack --offline --ignore-scripts --json > "$HOME/package-report.json"
tar -tzf ta-node-project-1.0.0.tgz > "$HOME/package-files.txt"
node -e 'const fs = require("node:fs"); const assert = require("node:assert/strict"); const report = JSON.parse(fs.readFileSync(process.env.HOME + "/package-report.json")); const p = Array.isArray(report) ? report[0] : report["ta-node-project"]; assert(p.files.some(f => f.path === "dist/index.cjs")); assert(!p.files.some(f => f.path.startsWith(".npm/") || f.path.startsWith(".cache/") || f.path.includes("state.sh")), JSON.stringify(p.files)); const paths = fs.readFileSync(process.env.HOME + "/package-files.txt", "utf8").trim().split("\n"); assert(paths.includes("package/dist/index.cjs"), JSON.stringify(paths)); assert(!paths.some(p => p.startsWith("package/.npm/") || p.startsWith("package/.cache/") || p.includes("state.sh")), JSON.stringify(paths)); assert.equal(require("./dist/index.cjs").total([7, 8]), 15); console.log("NODE_PROJECT_OK", JSON.stringify(paths));'
"#,
    );
    assert!(output.contains("NODE_PROJECT_OK"), "{output}");
    eprintln!("Node delivery: {output}");
    assert_eq!(
        entries(&root),
        [
            "build.cjs",
            "dist",
            "node_modules",
            "package-lock.json",
            "package.json",
            "src",
            "ta-node-project-1.0.0.tgz",
            "test",
            "vendor"
        ]
    );
    assert!(state.join("home/.npm/_cacache").is_dir());
}

#[test]
fn python_project_builds_and_installs_a_wheel_after_venv_resume() {
    if !available(&["python3"]) {
        return;
    }
    let prerequisite = std::process::Command::new(system_tool("python3").unwrap())
        .args(["-I", "-c", "import venv, ensurepip"])
        .output()
        .unwrap();
    if !prerequisite.status.success() {
        eprintln!("skip: the system Python installation needs venv and ensurepip");
        return;
    }
    let env = support::isolated_state_home("python-project");
    let root = env.join("project");
    let state = env.join("member/shell");
    write(
        &root,
        "pyproject.toml",
        "[build-system]\nrequires = []\nbuild-backend = 'build_backend'\nbackend-path = ['.']\n",
    );
    write(&root, "ta_demo.py", "def total(values):\n    return sum(values, 1)\n");
    write(&root, "test_demo.py", "import unittest\nfrom ta_demo import total\nclass Totals(unittest.TestCase):\n    def test_identity(self):\n        self.assertEqual(total([]), 0)\n    def test_values(self):\n        self.assertEqual(total([2, 3]), 5)\n");
    // An in-tree PEP 517 backend keeps this build independent of downloads.
    write(
        &root,
        "build_backend.py",
        r#"from pathlib import Path
import zipfile

def build_wheel(wheel_directory, config_settings=None, metadata_directory=None):
    name = 'ta_demo-1.0.0-py3-none-any.whl'
    files = {
        'ta_demo.py': Path('ta_demo.py').read_text(),
        'ta_demo-1.0.0.dist-info/METADATA': 'Metadata-Version: 2.1\nName: ta-demo\nVersion: 1.0.0\n',
        'ta_demo-1.0.0.dist-info/WHEEL': 'Wheel-Version: 1.0\nGenerator: teamagents-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n',
    }
    record = 'ta_demo-1.0.0.dist-info/RECORD'
    files[record] = ''.join(path + ',,\n' for path in [*files, record])
    with zipfile.ZipFile(Path(wheel_directory) / name, 'w') as archive:
        for path, content in files.items():
            archive.writestr(path, content)
    return name
"#,
    );
    eprintln!(
        "Python venv: {}",
        success(
            &root,
            Some(&state),
            "set -e; python3 --version; python3 -m venv .venv; . .venv/bin/activate; python -m pip --version"
        )
    );
    let failed = run(&root, Some(&state), "python -m unittest -v test_demo");
    assert!(failed.contains("FAILED (failures=2)") && failed.contains("(exit 1)"), "{failed}");
    let edit = tools::workspace_executor(root.clone(), None);
    edit("edit_file", &json!({"path":"ta_demo.py","old_string":"sum(values, 1)","new_string":"sum(values, 0)"}))
        .unwrap();
    let output = success(
        &root,
        Some(&state),
        r#"set -e
test -n "$VIRTUAL_ENV"
python -m unittest -v test_demo
python -m pip wheel --no-index --no-build-isolation --wheel-dir dist .
python -m pip install --no-index --force-reinstall dist/ta_demo-1.0.0-py3-none-any.whl
cd /tmp
python -c 'import os, ta_demo; assert "site-packages" in ta_demo.__file__, ta_demo.__file__; assert ta_demo.total([4, 5]) == 9; assert os.environ["VIRTUAL_ENV"] in ta_demo.__file__; print("PYTHON_PROJECT_OK")'
"#,
    );
    assert!(output.contains("PYTHON_PROJECT_OK"), "{output}");
    eprintln!("Python delivery: {output}");
    assert_eq!(
        entries(&root),
        [".venv", "__pycache__", "build_backend.py", "dist", "pyproject.toml", "ta_demo.py", "test_demo.py"]
    );
}

#[test]
fn workspace_mcp_home_does_not_write_its_cache_into_the_project() {
    if !available(&[]) {
        return;
    }
    let env = support::isolated_state_home("mcp-private-home");
    let root = env.join("project");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_fake-mcp-server"), root.join("server")).unwrap();
    let client = McpClient::connect_stdio_in(
        "/bin/sh",
        &["-c".into(), "set -e; mkdir -p \"$HOME/.cache\"; printf private > \"$HOME/.cache/server\"; printf '%s' \"$HOME\" > server-home.txt; exec ./server".into()],
        &[], &root, "workspace", false, 10, 10,
    ).unwrap();
    let result = client.call_tool("echo", &json!({"text":"ready"}));
    client.close();
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(entries(&root), ["server", "server-home.txt"]);
    assert_ne!(PathBuf::from(std::fs::read_to_string(root.join("server-home.txt")).unwrap()), root);
}
