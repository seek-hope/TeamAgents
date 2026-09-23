//! R2-P0 executable probes, deliberately separate from the production backend.
mod daemon;
mod io_probe;
mod runner;
mod store;
mod suite;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Serialize, Deserialize)]
struct Request {
    version: u32,
    command_id: String,
    method: String,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn ensure(value: bool, message: &str) -> Result<()> {
    if value {
        Ok(())
    } else {
        Err(message.to_owned().into())
    }
}

fn init_root(root: &Path) -> Result<()> {
    if !root.exists() {
        std::fs::create_dir_all(root)?;
    }
    ensure(!root.symlink_metadata()?.file_type().is_symlink(), "探针根不能是符号链接")?;
    let marker = root.join("p0-format");
    if !marker.exists() {
        ensure(std::fs::read_dir(root)?.next().is_none(), "拒绝打开非空的非 P0 状态目录")?;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
        atomic_bytes(&marker, b"teamagents-r2-p0-v1")?;
    }
    ensure(std::fs::read(marker)? == b"teamagents-r2-p0-v1", "P0 状态版本不匹配")
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("pending");
    let mut file = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)?;
    File::open(path.parent().ok_or("缺少父目录")?)?.sync_all()?;
    Ok(())
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_bytes(path, &serde_json::to_vec(value)?)
}

fn rpc(socket: &Path, method: &str) -> Result<Value> {
    rpc_id(socket, method, &uuid::Uuid::new_v4().to_string())
}

fn rpc_id(socket: &Path, method: &str, id: &str) -> Result<Value> {
    let mut conn = UnixStream::connect(socket)?;
    conn.set_read_timeout(Some(Duration::from_secs(2)))?;
    conn.set_write_timeout(Some(Duration::from_secs(2)))?;
    serde_json::to_writer(&mut conn, &Request { version: 1, command_id: id.into(), method: method.into() })?;
    conn.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(conn).read_line(&mut line)?;
    ensure(line.len() <= 65_536, "控制回执过大")?;
    let result: Value = serde_json::from_str(&line)?;
    ensure(result["ok"] == true, &format!("控制失败：{result}"))?;
    Ok(result)
}

fn executable() -> Result<PathBuf> {
    Ok(std::env::current_exe()?)
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(String::as_str).unwrap_or("help");
    match command {
        "suite" => suite::run(args.get(2).map(PathBuf::from).ok_or("suite 需要新的证据目录")?).await,
        "runner" => runner::serve(Path::new(args.get(2).ok_or("缺少 job 目录")?)).await,
        "daemon" => daemon::serve(Path::new(args.get(2).ok_or("缺少状态目录")?)).await,
        "transaction-child" => store::transaction_child(&args[2..]),
        "artifact-child" => store::artifact_child(&args[2..]),
        "rpc" => {
            let result = rpc(Path::new(args.get(2).ok_or("缺少 socket")?), args.get(3).ok_or("缺少方法")?)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(())
        }
        _ => {
            eprintln!("R2-P0 本地探针：suite EVIDENCE_DIR | daemon STATE_DIR | rpc SOCKET METHOD");
            Ok(())
        }
    }
}
