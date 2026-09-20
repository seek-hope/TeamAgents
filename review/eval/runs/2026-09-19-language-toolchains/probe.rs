use std::path::Path;
fn main() {
    let base = std::env::args().nth(1).expect("probe root");
    for (name, command) in [
        ("python", "set -e; command -v python3; python3 --version; python3 -m venv .venv; .venv/bin/python -m pip --version; .venv/bin/python -c 'import ssl, sqlite3, sys; print(sys.prefix); print(ssl.OPENSSL_VERSION)'"),
        ("node", "set -e; command -v node; node --version; npm --version; npm ci --offline --ignore-scripts --no-audit --no-fund; npm test; npm pack --offline --ignore-scripts")
    ] {
        let project = Path::new(&base).join(name);
        std::fs::create_dir_all(&project).unwrap();
        if name == "node" {
            std::fs::write(project.join("package.json"), r#"{"name":"ta-language-probe","version":"1.0.0","scripts":{"test":"node --test"}}"#).unwrap();
            std::fs::write(project.join("package-lock.json"), r#"{"name":"ta-language-probe","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"ta-language-probe","version":"1.0.0"}}}"#).unwrap();
            std::fs::write(project.join("sum.test.cjs"), "const {test}=require('node:test'); const assert=require('node:assert/strict'); test('adds',()=>assert.equal(1+2,3));").unwrap();
        }
        println!("CASE {name}: {:?}", teamagents_engine::tools::shell_run(command, &project, 90, false, None));
    }
}
