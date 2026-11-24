//! Shells see an independent copy. Only validated regular files and directories can be published.
use crate::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io::{self, Read, Seek, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Component, Path, PathBuf},
    process::Command,
};

const MAX_ENTRIES: usize = 32_768;
const MAX_BYTES: u64 = 256 << 20;
const MAX_DEPTH: usize = 64;
const CACHE_TAG: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

fn err(e: io::Error) -> String {
    format!("sandbox filesystem: {e}")
}
pub(crate) fn fd_path(f: &File) -> PathBuf {
    format!("/proc/self/fd/{}", f.as_raw_fd()).into()
}
fn pin(path: &Path) -> io::Result<File> {
    OpenOptions::new().read(true).custom_flags(libc::O_PATH | libc::O_NOFOLLOW).open(path)
}
fn mount(f: &File) -> Result<u64> {
    // SAFETY: statx receives an open fd, an empty pathname, and writable storage.
    unsafe {
        let mut s: libc::statx = std::mem::zeroed();
        if libc::statx(f.as_raw_fd(), c"".as_ptr(), libc::AT_EMPTY_PATH, libc::STATX_MNT_ID, &mut s) == -1 {
            return Err(err(io::Error::last_os_error()));
        }
        if s.stx_mask & libc::STATX_MNT_ID == 0 {
            return Err("sandbox requires Linux mount IDs".into());
        }
        Ok(s.stx_mnt_id)
    }
}
fn regular(m: &Metadata) -> Result<()> {
    if m.is_file() && m.nlink() == 1 {
        Ok(())
    } else {
        Err("sandbox requires a single-link regular file".into())
    }
}
fn same_mount(f: &File, root: &File) -> Result<()> {
    if mount(f)? != mount(root)? || f.metadata().map_err(err)?.dev() != root.metadata().map_err(err)?.dev() {
        return Err("sandbox refuses nested mounts and subvolumes".into());
    }
    Ok(())
}
fn exists(p: &Path) -> Result<bool> {
    match fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(err(e)),
    }
}
fn admin_dir(p: &Path) -> Result<bool> {
    // Bare and separately named Git administrative directories must not become alternate writable views.
    Ok(exists(&p.join("HEAD"))? && (exists(&p.join("objects"))? || exists(&p.join("commondir"))?))
}
fn cached_target(p: &Path, dir: &File) -> Result<bool> {
    if p.file_name() != Some(OsStr::new("target")) {
        return Ok(false);
    }
    let tag = match pin(&fd_path(dir).join("CACHEDIR.TAG")) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(err(e)),
    };
    same_mount(&tag, dir)?;
    regular(&tag.metadata().map_err(err)?)?;
    let mut prefix = vec![0; CACHE_TAG.len()];
    let mut tag = File::open(fd_path(&tag)).map_err(err)?;
    match tag.read_exact(&mut prefix) {
        Ok(()) => Ok(prefix == CACHE_TAG),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(err(e)),
    }
}
fn reserved(p: &Path) -> bool {
    p.components().any(|c| c.as_os_str().eq_ignore_ascii_case(".git"))
}
fn root_dir(root: &Path) -> Result<File> {
    if root.canonicalize().map_err(err)? != root {
        return Err("workspace path changed or is not canonical".into());
    }
    if reserved(root) || root == Path::new("/") || root == Path::new("/tmp") {
        return Err("reserved workspace root".into());
    }
    for ancestor in root.ancestors() {
        if admin_dir(ancestor)?
            || (ancestor.file_name() == Some(OsStr::new("target"))
                && cached_target(ancestor, &pin(ancestor).map_err(err)?)?)
        {
            return Err("reserved workspace root or ancestor".into());
        }
    }
    OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW).open(root).map_err(err)
}
fn checked_dir(root: &File, parent: &File, name: &OsStr) -> Result<File> {
    let child = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(fd_path(parent).join(name))
        .map_err(err)?;
    same_mount(&child, root)?;
    if admin_dir(&fd_path(&child))? || cached_target(Path::new(name), &child)? {
        return Err("reserved workspace directory".into());
    }
    Ok(child)
}

/// Directory descriptors pin every lookup; the final component is checked separately before reading.
fn parent_at(root: &File, path: &Path) -> Result<(File, PathBuf)> {
    let components = path.components();
    if reserved(path) || !components.clone().all(|c| matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err("reserved or non-relative workspace path".into());
    }
    let mut parts = components.filter_map(|c| if let Component::Normal(p) = c { Some(p) } else { None });
    let name = parts.next_back().ok_or("path must name a file")?;
    let mut dir = root.try_clone().map_err(err)?;
    for part in parts {
        dir = checked_dir(root, &dir, part)?;
    }
    let target = fd_path(&dir).join(name);
    Ok((dir, target))
}

pub(crate) fn open_regular(dir: &File, path: &Path) -> Result<File> {
    let f = pin(path).map_err(err)?;
    regular(&f.metadata().map_err(err)?)?;
    same_mount(&f, dir)?;
    File::open(fd_path(&f)).map_err(err)
}

pub(crate) struct Lock(File);
impl Lock {
    pub(crate) fn parent(&self, path: &Path) -> Result<(File, PathBuf)> {
        parent_at(&self.0, path)
    }
    pub(crate) fn workspace(root: &Path) -> Result<Self> {
        let f = root_dir(root)?;
        // SAFETY: f owns a valid directory fd; dropping f releases the advisory lock.
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == -1 {
            return Err(err(io::Error::last_os_error()));
        }
        Ok(Self(f))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: also release any short-lived duplicate in a concurrently forking child.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[derive(Clone, Eq, PartialEq)]
struct Node {
    dir: bool,
    dev: u64,
    ino: u64,
    size: u64,
    mode: u32,
    ctime: (i64, i64),
}
impl Node {
    fn of(m: &Metadata) -> Self {
        Self {
            dir: m.is_dir(),
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            mode: m.mode() & 0o777,
            ctime: (m.ctime(), m.ctime_nsec()),
        }
    }
    fn changed(&self, other: &Self) -> bool {
        if self.dir && other.dir {
            self.mode != other.mode
        } else {
            self != other
        }
    }
}
#[derive(Default)]
struct Tree {
    entries: BTreeMap<PathBuf, Node>,
    hidden: BTreeSet<PathBuf>,
    count: usize,
    bytes: u64,
}
impl Tree {
    fn read(root: &File, copy: Option<&Path>, host: bool) -> Result<Self> {
        let mut tree = Self::default();
        tree.walk(root, root, Path::new(""), copy, host, 0)?;
        Ok(tree)
    }
    fn walk(
        &mut self,
        root: &File,
        dir: &File,
        rel: &Path,
        copy: Option<&Path>,
        host: bool,
        depth: usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err("sandbox tree exceeds depth limit (64)".into());
        }
        for entry in fs::read_dir(fd_path(dir)).map_err(err)? {
            let name = entry.map_err(err)?.file_name();
            self.count += 1;
            if self.count > MAX_ENTRIES {
                return Err("sandbox tree exceeds entry limit (32768)".into());
            }
            let path = rel.join(&name);
            let f = pin(&fd_path(dir).join(&name)).map_err(err)?;
            same_mount(&f, root)?;
            let m = f.metadata().map_err(err)?;
            if reserved(&path) || (m.is_dir() && (admin_dir(&fd_path(&f))? || cached_target(&path, &f)?)) {
                if !host {
                    return Err(format!("sandbox refused publication of reserved path {}", path.display()));
                }
                self.hidden.insert(path);
                continue;
            }
            if !m.is_dir() {
                regular(&m).map_err(|e| format!("{}: {e}", path.display()))?;
                self.bytes = self.bytes.saturating_add(m.len());
                if self.bytes > MAX_BYTES {
                    return Err("sandbox tree exceeds 256 MiB; use a smaller workspace".into());
                }
            }
            self.entries.insert(path.clone(), Node::of(&m));
            if m.is_dir() {
                if let Some(dst) = copy {
                    fs::create_dir(dst.join(&path)).map_err(err)?;
                }
                self.walk(root, &f, &path, copy, host, depth + 1)?;
                if let Some(dst) = copy {
                    fs::set_permissions(dst.join(&path), fs::Permissions::from_mode(m.mode() & 0o777)).map_err(err)?;
                }
            } else if let Some(dst) = copy {
                copy_file(&f, &dst.join(&path), m.len(), m.mode())?;
            }
        }
        Ok(())
    }
}
fn copy_file(src: &File, dst: &Path, size: u64, mode: u32) -> Result<()> {
    let mut out = OpenOptions::new().write(true).create_new(true).mode(0o600).open(dst).map_err(err)?;
    let mut src = File::open(fd_path(src)).map_err(err)?.take(size + 1);
    if io::copy(&mut src, &mut out).map_err(err)? != size {
        return Err("sandbox source changed during copy".into());
    }
    out.set_permissions(fs::Permissions::from_mode(mode & 0o777)).map_err(err)
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        let path = Path::new("/tmp").join(format!("teamagents-stage-{}", uuid::Uuid::new_v4()));
        DirBuilder::new().mode(0o700).create(&path).map_err(err)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if fs::remove_dir_all(&self.0).is_err() {
            // A tool may have made its own staging directories read-only. Recover permissions only on
            // pinned directory fds; do not follow links or chmod files (an inode could have aliases).
            if let Ok(dir) = pin(&self.0) {
                restore_dirs(&dir);
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

fn restore_dirs(top: &File) {
    let Ok(first) = top.try_clone() else { return };
    let mut dirs = vec![first];
    while let Some(dir) = dirs.pop() {
        if same_mount(&dir, top).is_err() || !dir.metadata().is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let path = fd_path(&dir);
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    if let Ok(f) = pin(&entry.path()) {
                        dirs.push(f);
                    }
                }
            }
        }
    }
}

pub(crate) struct Stage {
    workspace: PathBuf,
    identity: File,
    scratch: Scratch,
    original: Tree,
    baseline: Tree,
}
impl Stage {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let lock = Lock::workspace(root)?;
        let scratch = Scratch::new()?;
        if scratch.0.starts_with(root) {
            return Err("sandbox staging and workspace overlap".into());
        }
        let original = Tree::read(&lock.0, Some(&scratch.0), true)?;
        let baseline = Tree::read(&File::open(&scratch.0).map_err(err)?, None, false)?;
        Ok(Self { workspace: root.to_owned(), identity: lock.0.try_clone().map_err(err)?, scratch, original, baseline })
    }

    pub(crate) fn command(&self) -> Result<(Command, File)> {
        let mut cmd = Command::new("/usr/bin/bwrap");
        cmd.current_dir("/").env_clear().process_group(0).args([
            "--unshare-user",
            "--disable-userns",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup-try",
            "--new-session",
            "--die-with-parent",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--dir",
            "/home/agent",
            "--dir",
            "/etc/ssl/certs",
        ]);
        // Only system runtime trees, with the distribution's /bin and loader paths mirrored.
        for path in [
            "/usr/bin",
            "/usr/sbin",
            "/usr/lib",
            "/usr/lib64",
            "/usr/libexec",
            "/usr/include",
            "/usr/share",
            "/bin",
            "/sbin",
            "/lib",
            "/lib64",
        ] {
            if Path::new(path).is_dir() {
                cmd.arg("--ro-bind").arg(fs::canonicalize(path).map_err(err)?).arg(path);
            }
        }
        let cert = Path::new("/etc/ssl/certs/ca-certificates.crt");
        if cert.is_file() {
            cmd.arg("--ro-bind").arg(cert.canonicalize().map_err(err)?).arg(cert);
        }
        cmd.arg("--bind").arg(&self.scratch.0).arg("/workspace").args(["--chdir", "/workspace"]);
        let path = rust_tools(&mut cmd, &self.scratch.0)?;
        for (name, value) in [
            ("PATH", path.as_str()),
            ("HOME", "/home/agent"),
            ("LANG", "C"),
            ("CARGO_HOME", "/home/agent/.cargo"),
            ("CARGO_TARGET_DIR", "/tmp/target"),
            ("SSL_CERT_FILE", "/etc/ssl/certs/ca-certificates.crt"),
            ("GIT_SSL_CAINFO", "/etc/ssl/certs/ca-certificates.crt"),
        ] {
            cmd.args(["--setenv", name, value]);
        }
        // SAFETY: the hook only issues the async-signal-safe close_range syscall. Network::prepare
        // registers its own hook afterwards and selectively inherits the handshake descriptors.
        unsafe {
            cmd.pre_exec(|| {
                if libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let filter = payload_filter(&mut cmd)?;
        Ok((cmd, filter))
    }

    pub(crate) fn publish(self) -> Result<()> {
        let stage = File::open(&self.scratch.0).map_err(err)?;
        if admin_dir(&self.scratch.0)? {
            return Err("Git administrative workspace cannot be published".into());
        }
        let after = Tree::read(&stage, None, false)?;
        let changed: BTreeSet<_> = after
            .entries
            .keys()
            .chain(self.baseline.entries.keys())
            .filter(|key| match (self.baseline.entries.get(*key), after.entries.get(*key)) {
                (Some(a), Some(b)) => a.changed(b),
                _ => true,
            })
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        let lock = Lock::workspace(&self.workspace)?;
        same_mount(&lock.0, &self.identity)?;
        if lock.0.metadata().map_err(err)?.ino() != self.identity.metadata().map_err(err)?.ino() {
            return Err("workspace root changed during command".into());
        }
        let current = Tree::read(&lock.0, None, true)?;
        for &path in &changed {
            if self.original.hidden.iter().chain(&current.hidden).any(|h| path.starts_with(h) || h.starts_with(path)) {
                return Err("sandbox change overlaps protected workspace contents".into());
            }
            if self.original.entries.get(path) != current.entries.get(path) {
                return Err(format!("workspace changed concurrently: {}", path.display()));
            }
            for ancestor in path.ancestors().skip(1) {
                if let Some(n) = self.original.entries.get(ancestor) {
                    if current.entries.get(ancestor).is_none_or(|c| (c.dev, c.ino) != (n.dev, n.ino)) {
                        return Err("workspace ancestor changed concurrently".into());
                    }
                }
            }
        }
        // Preflight the full delta, then publish one entry at a time. Never recursively remove host data.
        for &path in changed.iter().rev() {
            if let Some(old) = self.original.entries.get(path) {
                if after.entries.get(path).is_none_or(|new| new.dir != old.dir) {
                    let (_dir, dst) = parent_at(&lock.0, path)?;
                    if old.dir { fs::remove_dir(dst) } else { fs::remove_file(dst) }.map_err(err)?;
                }
            }
        }
        for &path in &changed {
            let Some(new) = after.entries.get(path) else { continue };
            let (dir, dst) = parent_at(&lock.0, path)?;
            if new.dir {
                if !self.original.entries.get(path).is_some_and(|n| n.dir) {
                    fs::create_dir(&dst).map_err(err)?;
                }
            } else {
                let tmp = fd_path(&dir).join(format!(".teamagents-{}", uuid::Uuid::new_v4()));
                let src = pin(&self.scratch.0.join(path)).map_err(err)?;
                let result =
                    copy_file(&src, &tmp, new.size, new.mode).and_then(|()| fs::rename(&tmp, dst).map_err(err));
                if result.is_err() {
                    let _ = fs::remove_file(tmp);
                }
                result?;
            }
        }
        // Descending order allows a directory to become read-only after all its children are in place.
        for &path in changed.iter().rev() {
            if let Some(new) = after.entries.get(path).filter(|n| n.dir) {
                let (_dir, dst) = parent_at(&lock.0, path)?;
                fs::set_permissions(dst, fs::Permissions::from_mode(new.mode)).map_err(err)?;
            }
        }
        Ok(())
    }
}

// BPF over Linux seccomp_data: nr at byte 0, arch at 4, and args[0] at 16 on these little-endian ABIs.
// Filter the payload after bwrap has finished setting up mounts, not the daemon or networking helper.
fn payload_filter(cmd: &mut Command) -> Result<File> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => 0xc000003e,
        "aarch64" if cfg!(target_endian = "little") => 0xc00000b7,
        _ => return Err("unsupported Linux sandbox syscall ABI".into()),
    };
    let op = |code: u32, jt, jf, k| libc::sock_filter { code: code as u16, jt, jf, k };
    let load = |offset| op(libc::BPF_LD | libc::BPF_W | libc::BPF_ABS, 0, 0, offset);
    let eq = |n, yes, no| op(libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K, yes, no, n);
    let ret = |v| op(libc::BPF_RET | libc::BPF_K, 0, 0, v);
    let denied = ret(libc::SECCOMP_RET_ERRNO | libc::EPERM as u32);
    let allow = ret(libc::SECCOMP_RET_ALLOW);
    let mut filter = vec![
        load(4),
        eq(arch, 1, 0),
        denied,
        load(0),
        op(libc::BPF_JMP | libc::BPF_JGE | libc::BPF_K, 0, 1, 0x40000000),
        denied,
    ]; // no x32 ABI
    for call in [
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_register,
        libc::SYS_io_uring_enter,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
    ] {
        filter.extend([eq(call as u32, 0, 1), denied]);
    }
    filter.extend([eq(libc::SYS_socket as u32, 2, 0), eq(libc::SYS_socketpair as u32, 1, 0), allow, load(16)]);
    for domain in [libc::AF_UNIX, libc::AF_INET, libc::AF_INET6, libc::AF_NETLINK] {
        filter.extend([eq(domain as u32, 0, 1), allow]);
    }
    filter.push(denied);
    // SAFETY: successful memfd_create returns an owned close-on-exec fd. sock_filter is a padding-free
    // Linux UAPI structure (u16,u8,u8,u32); the slice borrows initialized data only until write returns.
    let mut file = unsafe {
        let fd = libc::memfd_create(c"teamagents-filter".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return Err(err(io::Error::last_os_error()));
        }
        File::from_raw_fd(fd)
    };
    let bytes = unsafe { std::slice::from_raw_parts(filter.as_ptr().cast::<u8>(), std::mem::size_of_val(&filter[..])) };
    file.write_all(bytes).and_then(|()| file.rewind()).map_err(err)?;
    let fd = file.as_raw_fd();
    cmd.arg("--seccomp").arg(fd.to_string());
    // SAFETY: only fcntl is called after fork. The caller retains this file through spawn.
    unsafe {
        cmd.pre_exec(
            move || {
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            },
        );
    }
    Ok(file)
}

fn rust_tools(cmd: &mut Command, workspace: &Path) -> Result<String> {
    // Read a channel name only. A workspace toolchain path is never a host mount source.
    let setting = File::open(workspace.join("rust-toolchain.toml")).ok().and_then(|f| {
        let mut text = String::new();
        f.take(8192).read_to_string(&mut text).ok().map(|_| text)
    });
    let channel = setting
        .as_deref()
        .and_then(|s| {
            s.lines().find_map(|l| {
                let (key, value) = l.split_once('=')?;
                (key.trim() == "channel").then(|| value.trim().trim_matches('"'))
            })
        })
        .unwrap_or("stable");
    let base = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".rustup")));
    let default = "/usr/bin:/bin".to_string();
    if !channel.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-')) || channel.is_empty() {
        return Ok(default);
    }
    let Some(base) = base else { return Ok(default) };
    let triplet =
        format!("{}-unknown-linux-{}", std::env::consts::ARCH, if cfg!(target_env = "musl") { "musl" } else { "gnu" });
    let name = if channel.ends_with(&triplet) { channel.to_string() } else { format!("{channel}-{triplet}") };
    let tool = base.join("toolchains").join(name);
    if !tool.is_absolute() || tool.canonicalize().ok().as_ref() != Some(&tool) || !tool.join("bin/rustc").is_file() {
        return Ok(default);
    }
    for name in ["bin", "lib", "libexec"] {
        let src = tool.join(name);
        if !exists(&src)? {
            continue;
        }
        let dir = pin(&src).map_err(err)?;
        if !dir.metadata().map_err(err)?.is_dir() {
            return Err("invalid installed Rust toolchain".into());
        }
        same_mount(&dir, &pin(&tool).map_err(err)?)?;
        check_runtime_tree(&dir, &dir, &mut 0, 0)?;
        cmd.arg("--ro-bind").arg(src).arg(format!("/opt/rust/{name}"));
    }
    Ok(format!("/opt/rust/bin:{default}"))
}
fn check_runtime_tree(root: &File, dir: &File, count: &mut usize, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err("invalid Rust runtime depth".into());
    }
    for e in fs::read_dir(fd_path(dir)).map_err(err)? {
        *count += 1;
        if *count > MAX_ENTRIES {
            return Err("Rust runtime exceeds inspection limit".into());
        }
        let f = pin(&e.map_err(err)?.path()).map_err(err)?;
        same_mount(&f, root)?;
        let m = f.metadata().map_err(err)?;
        if m.is_dir() {
            check_runtime_tree(root, &f, count, depth + 1)?;
        } else {
            regular(&m)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "sandbox_tests.rs"]
mod tests;
