//! Offline release bootstrap tests; no credentials or real installation paths.
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PACKAGE: &str = "teamagents-9.8.7-x86_64-unknown-linux-musl";

struct Fixture { root: PathBuf }

fn executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ta-install-{}", uuid::Uuid::new_v4()));
        for dir in ["release", "tools", "bin with spaces ' quote", "config/teamagents"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let fixture = Self { root };
        fixture.pack(true, true);
        fixture
    }

    fn archive(&self) -> PathBuf { self.root.join(format!("release/{PACKAGE}.tar.gz")) }
    fn bin(&self) -> PathBuf { self.root.join("bin with spaces ' quote") }
    fn config(&self) -> PathBuf { self.root.join("config/teamagents/config.toml") }

    fn pack(&self, supports_init: bool, include_tui: bool) {
        let release = self.root.join("release");
        let package = release.join(PACKAGE);
        let _ = fs::remove_dir_all(&package);
        fs::create_dir_all(&package).unwrap();
        executable(&package.join("teamagents"), if supports_init {
            "#!/bin/sh\nprintf 'teamagents init\\n'\n"
        } else { "#!/bin/sh\nprintf 'legacy help\\n'; exit 2\n" });
        if include_tui { executable(&package.join("teamagents-tui"), "#!/bin/sh\nexit 0\n"); }
        fs::write(package.join("config.example.toml"), teamagents_engine::config::INITIAL_CONFIG).unwrap();
        assert!(Command::new("tar").args(["-czf"]).arg(self.archive())
            .arg("-C").arg(&release).arg(PACKAGE).status().unwrap().success());
        let hash = Sha256::digest(fs::read(self.archive()).unwrap());
        // Unrelated assets must not make a single-platform installation fail.
        fs::write(release.join("SHA256SUMS"), format!("{hash:x}  {PACKAGE}.tar.gz\n{}  other-platform.tar.gz\n", "0".repeat(64))).unwrap();
    }

    fn run(&self, local: bool) -> Output {
        let mut command = Command::new("/bin/sh");
        command.arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../install.sh"))
            .arg("--bin-dir").arg(self.bin())
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("TA_FIXTURE_DIR", self.root.join("release"))
            .env("TA_TRANSPORT_LOG", self.root.join("transport.log"));
        let mut paths = vec![self.root.join("tools")];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()));
        command.env("PATH", std::env::join_paths(paths).unwrap());
        if local { command.arg("--archive").arg(self.archive()); }
        command.output().unwrap()
    }
}

impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }

#[test]
fn local_install_and_legacy_config_preservation() {
    let f = Fixture::new();
    let output = f.run(true);
    assert!(output.status.success(), "{output:?}");
    assert!(f.bin().join("teamagents-tui").is_file());
    assert!(!f.config().exists(), "modern releases defer config creation to init");
    assert!(String::from_utf8_lossy(&output.stdout).contains("teamagents init"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let export = stdout.lines().find(|line| line.starts_with("export PATH=")).unwrap();
    let path = Command::new("/bin/sh").arg("-c")
        .arg(format!("{export}\nprintf '%s' \"$PATH\""))
        .env("PATH", "/usr/bin").output().unwrap();
    assert!(path.status.success(), "{path:?}");
    assert_eq!(String::from_utf8(path.stdout).unwrap(), format!("{}:/usr/bin", f.bin().display()));
    f.pack(false, true);
    let output = f.run(true);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(f.config()).unwrap(), teamagents_engine::config::INITIAL_CONFIG);
    assert_eq!(fs::metadata(f.config()).unwrap().permissions().mode() & 0o777, 0o600);
    fs::write(f.config(), "user config").unwrap();
    assert!(f.run(true).status.success());
    assert_eq!(fs::read_to_string(f.config()).unwrap(), "user config");
    fs::remove_file(f.config()).unwrap();
    symlink(f.root.join("missing"), f.config()).unwrap();
    assert!(f.run(true).status.success());
    assert!(!f.root.join("missing").exists());
}

#[test]
fn corrupt_or_incomplete_download_never_changes_installed_pair() {
    let f = Fixture::new();
    fs::write(f.bin().join("teamagents"), "old engine").unwrap();
    fs::write(f.bin().join("teamagents-tui"), "old tui").unwrap();
    fs::write(f.archive(), "corrupt archive").unwrap();
    assert!(!f.run(true).status.success());
    f.pack(true, false);
    assert!(!f.run(true).status.success());
    f.pack(true, true);
    let sums = f.root.join("release/SHA256SUMS");
    let text = fs::read_to_string(&sums).unwrap();
    fs::write(&sums, format!("{text}{text}")).unwrap();
    assert!(!f.run(true).status.success(), "duplicate checksum entries are ambiguous");
    assert_eq!(fs::read_to_string(f.bin().join("teamagents")).unwrap(), "old engine");
    assert_eq!(fs::read_to_string(f.bin().join("teamagents-tui")).unwrap(), "old tui");
    assert!(!f.config().exists());
}

#[test]
fn failed_second_replacement_restores_both_old_programs() {
    let f = Fixture::new();
    let old = f.root.join("old-engine");
    fs::write(&old, "old engine").unwrap();
    symlink(&old, f.bin().join("teamagents")).unwrap();
    fs::write(f.bin().join("teamagents-tui"), "old tui").unwrap();
    executable(&f.root.join("tools/mv"), r#"#!/bin/sh
if [ "$1" = -f ]; then shift; fi
case "$1" in */.teamagents-install.*/teamagents-tui) exit 1 ;; esac
exec /bin/mv "$@"
"#);
    assert!(!f.run(true).status.success());
    assert!(fs::symlink_metadata(f.bin().join("teamagents")).unwrap().is_symlink());
    assert_eq!(fs::read_to_string(&old).unwrap(), "old engine");
    assert_eq!(fs::read_to_string(f.bin().join("teamagents-tui")).unwrap(), "old tui");
}

#[test]
fn authenticated_download_resolves_latest_and_installs() {
    let f = Fixture::new();
    executable(&f.root.join("tools/gh"), r#"#!/bin/sh
printf '%s\n' "$*" >> "$TA_TRANSPORT_LOG"
case "$1 $2" in
    'auth status') exit 0 ;;
    'release view') printf 'v9.8.7\n'; exit 0 ;;
    'release download')
        while [ "$#" -gt 0 ]; do
            if [ "$1" = --dir ]; then dest=$2; break; fi
            shift
        done
        cp "$TA_FIXTURE_DIR"/*.tar.gz "$TA_FIXTURE_DIR/SHA256SUMS" "$dest/" ;;
    *) exit 1 ;;
esac
"#);
    let output = f.run(false);
    assert!(output.status.success(), "{output:?}");
    let log = fs::read_to_string(f.root.join("transport.log")).unwrap();
    assert!(log.contains("release download v9.8.7"), "{log}");
    assert!(log.contains(&format!("--pattern {PACKAGE}.tar.gz")), "{log}");
    assert!(f.bin().join("teamagents-tui").exists());
}

#[test]
fn public_download_works_without_github_login_and_rejects_unsupported_os() {
    let f = Fixture::new();
    executable(&f.root.join("tools/gh"), "#!/bin/sh\nexit 1\n");
    executable(&f.root.join("tools/curl"), r#"#!/bin/sh
printf '%s\n' "$*" >> "$TA_TRANSPORT_LOG"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) dest=$2; shift 2 ;;
        https://*) url=$1; shift ;;
        *) shift ;;
    esac
done
case "$url" in
    */releases/latest) printf 'https://github.com/seek-hope/TeamAgents/releases/tag/v9.8.7' ;;
    */SHA256SUMS) cp "$TA_FIXTURE_DIR/SHA256SUMS" "$dest" ;;
    */teamagents-9.8.7-x86_64-unknown-linux-musl.tar.gz) cp "$TA_FIXTURE_DIR"/*.tar.gz "$dest" ;;
    *) exit 1 ;;
esac
"#);
    let output = f.run(false);
    assert!(output.status.success(), "{output:?}");
    let log = fs::read_to_string(f.root.join("transport.log")).unwrap();
    assert!(log.contains("--proto =https --proto-redir =https"), "{log}");
    fs::remove_file(f.root.join("transport.log")).unwrap();
    executable(&f.root.join("tools/uname"), "#!/bin/sh\nprintf 'Darwin\\n'\n");
    assert!(!f.run(false).status.success());
    assert!(!f.root.join("transport.log").exists(), "reject unsupported platforms before downloading");
}
