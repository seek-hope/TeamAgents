//! Attach outbound IPv4 to a private network namespace, before releasing the tool.
//! No forwarding/management socket. slirp's gateway cannot proxy host loopback.
use crate::Result;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::{net::UnixStream, process::CommandExt};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const SETUP: Duration = Duration::from_secs(5);

fn err(e: impl std::fmt::Display) -> String {
    format!("workspace network setup: {e}")
}

// Keep every descriptor CLOEXEC in the multithreaded daemon. Pass only selected setup
// handles across the exec in *this* child; bwrap consumes its setup FDs before the payload.
fn inherit(cmd: &mut Command, fds: Vec<RawFd>) {
    unsafe {
        cmd.pre_exec(move || {
            for &fd in &fds {
                if libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

struct SetupFds {
    _info: UnixStream,
    _block: UnixStream,
    _dns: File,
}

pub struct Network {
    info: UnixStream,
    block: UnixStream,
    setup: Option<SetupFds>,
    helper: Option<Child>,
    exit: Option<UnixStream>,
    binary: PathBuf,
}

impl Network {
    pub fn prepare(cmd: &mut Command) -> Result<Self> {
        let binary = std::env::var_os("TEAMAGENTS_SLIRP4NETNS")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/usr/bin/slirp4netns"));
        if !binary.is_absolute() || !binary.is_file() {
            return Err("online workspace needs slirp4netns; install it in /usr/bin or set TEAMAGENTS_SLIRP4NETNS to a trusted absolute executable before starting the daemon".into());
        }
        let (info, send_info) = UnixStream::pair().map_err(err)?;
        info.set_read_timeout(Some(SETUP)).map_err(err)?;
        let (block, wait) = UnixStream::pair().map_err(err)?;
        // The resolver is slirp's guest-side DNS, not a bind of a host loopback stub.
        let fd = unsafe { libc::memfd_create(c"swarm-dns".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(err(std::io::Error::last_os_error()));
        }
        let mut dns = unsafe { File::from_raw_fd(fd) };
        dns.write_all(b"nameserver 10.0.2.3\noptions timeout:2 attempts:2\n").map_err(err)?;
        dns.rewind().map_err(err)?;
        let fds = vec![send_info.as_raw_fd(), wait.as_raw_fd(), dns.as_raw_fd()];
        cmd.arg("--unshare-net")
            .arg("--info-fd")
            .arg(fds[0].to_string())
            .arg("--block-fd")
            .arg(fds[1].to_string())
            .arg("--ro-bind-data")
            .arg(fds[2].to_string())
            .arg("/etc/resolv.conf");
        inherit(cmd, fds);
        Ok(Self {
            info,
            block,
            setup: Some(SetupFds { _info: send_info, _block: wait, _dns: dns }),
            helper: None,
            exit: None,
            binary,
        })
    }

    pub fn connect(&mut self, child: &Child) -> Result<()> {
        self.setup.take();
        let mut packet = vec![];
        // Stop at the closing brace: some bwrap versions retain the fd until sandbox exit.
        BufReader::new((&self.info).take(8192)).read_until(b'}', &mut packet).map_err(err)?;
        let info: Value = serde_json::from_slice(&packet).map_err(|_| {
            err("bwrap did not report a ready namespace; check bwrap and kernel user-namespace permissions")
        })?;
        let pid = info["child-pid"]
            .as_u64()
            .filter(|n| *n > 1 && *n <= i32::MAX as u64)
            .ok_or_else(|| err("invalid namespace pid"))?;
        let host = std::fs::read_link("/proc/self/ns/net").map_err(err)?;
        let guest = std::fs::read_link(format!("/proc/{pid}/ns/net")).map_err(err)?;
        let children = std::fs::read_to_string(format!("/proc/{0}/task/{0}/children", child.id())).map_err(err)?;
        if host == guest || !children.split_whitespace().any(|p| p.parse::<u64>() == Ok(pid)) {
            return Err(err("bwrap must own a separate network namespace"));
        }
        // bwrap can switch the child to a second, less-privileged user namespace
        // while slirp starts. Pin the network namespace AND its actual owning user
        // namespace; looking up /proc/PID/ns/user later races that transition.
        let netns = File::open(format!("/proc/{pid}/ns/net")).map_err(err)?;
        const NS_GET_USERNS: libc::c_ulong = 0xb701;
        let owner_fd = unsafe { libc::ioctl(netns.as_raw_fd(), NS_GET_USERNS) };
        if owner_fd < 0 {
            return Err(err(std::io::Error::last_os_error()));
        }
        let owner = unsafe { File::from_raw_fd(owner_fd) };
        let (mut ready, signal) = UnixStream::pair().map_err(err)?;
        ready.set_read_timeout(Some(SETUP)).map_err(err)?;
        let (exit, watch) = UnixStream::pair().map_err(err)?;
        let mut cmd = Command::new(&self.binary);
        cmd.env_clear()
            .arg("--configure")
            .arg("--disable-host-loopback")
            .arg(format!("--ready-fd={}", signal.as_raw_fd()))
            .arg(format!("--exit-fd={}", watch.as_raw_fd()))
            .arg("--netns-type=path")
            .arg(format!("--userns-path=/proc/self/fd/{}", owner.as_raw_fd()))
            .arg(format!("/proc/self/fd/{}", netns.as_raw_fd()))
            .arg("tap0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        inherit(&mut cmd, vec![signal.as_raw_fd(), watch.as_raw_fd(), owner.as_raw_fd(), netns.as_raw_fd()]);
        self.helper = Some(cmd.spawn().map_err(err)?);
        self.exit = Some(exit);
        drop(signal);
        drop(watch);
        let mut byte = [0];
        if let Err(e) = ready.read_exact(&mut byte) {
            let helper = self.helper.as_mut().expect("helper started");
            let _ = helper.kill();
            let _ = helper.wait();
            let mut why = String::new();
            if let Some(stderr) = helper.stderr.take() {
                let _ = stderr.take(2048).read_to_string(&mut why);
            }
            return Err(err(format!("slirp4netns did not configure the private interface: {e}: {}", why.trim())));
        }
        if &byte != b"1" {
            return Err(err("invalid network readiness signal"));
        }
        self.block.write_all(b"1").map_err(err)
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        // Also on startup error. The exit fd closes on daemon death, even without Drop.
        self.exit.take();
        if let Some(mut child) = self.helper.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, UdpSocket};

    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("teamagents-net-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn shell(&self, script: &str) -> Result<String> {
            crate::tools::execute(&self.0, &serde_json::json!({"op":"shell", "command":script}))
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn private_interface_reaches_a_non_loopback_service_but_not_host_loopback() {
        // No external server or private project data. UDP connect selects a source address,
        // without sending a datagram. Serve one disposable payload on this machine's LAN IP.
        let route = UdpSocket::bind("0.0.0.0:0").unwrap();
        route.connect("192.0.2.1:9").expect("test host needs an IPv4 route");
        let addr = route.local_addr().unwrap().ip();
        assert!(!addr.is_loopback());
        let service = TcpListener::bind((addr, 0)).unwrap();
        service.set_nonblocking(true).unwrap();
        let port = service.local_addr().unwrap().port();
        let private = TcpListener::bind("127.0.0.1:0").unwrap();
        let private_port = private.local_addr().unwrap().port();
        let workspace = Workspace::new();
        let script = format!("set -e; curl -4fsS --noproxy '*' --max-time 4 http://{addr}:{port}/fixture -o downloaded; for ip in 10.0.2.2 127.0.0.1; do if curl -4fsS --noproxy '*' --max-time 1 http://$ip:{private_port}/; then exit 1; fi; done");
        let server = std::thread::spawn(move || {
            let until = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                match service.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        let mut header = [0; 1024];
                        let n = stream.read(&mut header).unwrap();
                        assert!(header[..n].starts_with(b"GET /fixture "));
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nfixture")
                            .unwrap();
                        return;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < until => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    e => panic!("network fixture did not receive the outbound request: {e:?}"),
                }
            }
        });
        workspace.shell(&script).unwrap();
        server.join().unwrap();
        assert_eq!(std::fs::read(workspace.0.join("downloaded")).unwrap(), b"fixture");
        private.set_nonblocking(true).unwrap();
        assert_eq!(private.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }

    #[test]
    fn unavailable_helper_never_falls_back_to_host_execution() {
        // Pass the daemon settings to a fresh process, not to concurrent tests.
        const CASE: &str = "TEAMAGENTS_TEST_NETWORK_SETUP";
        if let Ok(expected) = std::env::var(CASE) {
            let workspace = Workspace::new();
            let outside = Workspace::new();
            let marker = outside.0.join("ran");
            let script = format!("touch ran '{}'", marker.display());
            let error = workspace.shell(&script).unwrap_err();
            assert!(error.contains(&expected), "{error}");
            assert!(!workspace.0.join("ran").exists() && !marker.exists());
            return;
        }
        for (helper, expected) in
            [("/does-not-exist/teamagents-slirp", "needs slirp4netns"), ("/usr/bin/false", "did not configure")]
        {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "net::tests::unavailable_helper_never_falls_back_to_host_execution"])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("TEAMAGENTS_SLIRP4NETNS", helper)
                .env(CASE, expected)
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
        }
    }
}
