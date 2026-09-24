use super::{ensure, now_ms, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub fn process_metric(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|line| line.strip_prefix(key).and_then(|tail| tail.split_whitespace().next()?.parse().ok()))
        .unwrap_or(0)
}

pub fn cpu_us() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // Linux fills the complete structure on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0;
    }
    let usage = unsafe { usage.assume_init() };
    ((usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) * 1_000_000 + usage.ru_utime.tv_usec + usage.ru_stime.tv_usec)
        as u64
}

pub fn percentiles(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    let at = |p: usize| values[(values.len().saturating_sub(1) * p) / 100];
    json!({"samples":values.len(),"p50":at(50),"p95":at(95),"max":values.last().copied().unwrap_or(0)})
}

struct Fixture {
    url: String,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}/stream", listener.local_addr()?);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let join = std::thread::spawn(move || {
            let mut workers = Vec::new();
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut conn, _)) => {
                        let stop = thread_stop.clone();
                        workers.push(std::thread::spawn(move || {
                            let _ = conn.set_read_timeout(Some(Duration::from_secs(1)));
                            let _ = conn.set_write_timeout(Some(Duration::from_secs(1)));
                            let mut header = Vec::new();
                            let mut byte = [0; 1];
                            while !header.ends_with(b"\r\n\r\n") && header.len() < 16_384 {
                                if conn.read_exact(&mut byte).is_err() {
                                    return;
                                }
                                header.push(byte[0]);
                            }
                            if conn
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\nConnection: close\r\n\r\nx")
                                .is_err()
                            {
                                return;
                            }
                            while !stop.load(Ordering::SeqCst) {
                                std::thread::sleep(Duration::from_millis(2));
                            }
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                    Err(_) => break,
                }
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Ok(Self { url, stop, join: Some(join) })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub async fn run() -> Result<Value> {
    let mut results = Vec::new();
    for count in [1, 4, 16] {
        for mode in ["async", "bounded_blocking"] {
            let fixture = Fixture::start()?;
            let ready = Arc::new(AtomicUsize::new(0));
            let completed = Arc::new(AtomicUsize::new(0));
            let before_cpu = cpu_us();
            let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(5)).build()?;
            let mut handles = Vec::new();
            for _ in 0..count {
                let url = fixture.url.clone();
                let ready = ready.clone();
                let done = completed.clone();
                if mode == "async" {
                    let client = client.clone();
                    handles.push(tokio::spawn(async move {
                        if let Ok(mut response) = client.get(url).send().await {
                            if response.chunk().await.is_ok() {
                                ready.fetch_add(1, Ordering::SeqCst);
                                let _ = response.chunk().await;
                            }
                        }
                        done.fetch_add(1, Ordering::SeqCst);
                    }));
                } else {
                    handles.push(tokio::task::spawn_blocking(move || {
                        let agent = ureq::AgentBuilder::new()
                            .timeout_connect(Duration::from_secs(1))
                            .timeout_read(Duration::from_millis(350))
                            .build();
                        if let Ok(response) = agent.get(&url).call() {
                            let mut input = response.into_reader();
                            if input.read_exact(&mut [0; 1]).is_ok() {
                                ready.fetch_add(1, Ordering::SeqCst);
                                let _ = input.read_to_end(&mut Vec::new());
                            }
                        }
                        done.fetch_add(1, Ordering::SeqCst);
                    }));
                }
            }
            let until = now_ms() + 2500;
            while ready.load(Ordering::SeqCst) != count {
                ensure(now_ms() < until, "the local HTTP fixture did not enter a blocking read")?;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            let mut control = Vec::new();
            let (control_tx, mut control_rx) = tokio::sync::mpsc::channel::<tokio::sync::oneshot::Sender<()>>(4);
            let controller = tokio::spawn(async move {
                while let Some(reply) = control_rx.recv().await {
                    let _ = reply.send(());
                }
            });
            for _ in 0..12 {
                let start = Instant::now();
                let (reply, done) = tokio::sync::oneshot::channel();
                control_tx.send(reply).await?;
                tokio::time::timeout(Duration::from_secs(1), done).await??;
                control.push(start.elapsed().as_micros() as u64);
            }
            drop(control_tx);
            controller.await?;
            let threads = process_metric("Threads:");
            let rss = process_metric("VmRSS:");
            let start = Instant::now();
            for handle in &handles {
                handle.abort();
            }
            for handle in handles {
                let _ = handle.await;
            }
            let cancel_ms = start.elapsed().as_secs_f64() * 1000.0;
            let completed = completed.load(Ordering::SeqCst);
            if mode == "async" {
                ensure(completed == 0, "the async request was not cancelled while blocking")?;
            } else {
                ensure(completed == count, "the blocking thread has not actually exited")?;
            }
            results.push(json!({"mode":mode,"active_io":count,"cancel_workers_ms":cancel_ms,
                "control_roundtrip_us":percentiles(control),"rss_kib":rss,"threads_including_fixture":threads,
                "cpu_us":cpu_us().saturating_sub(before_cpu)}));
        }
    }
    Ok(json!({"fixture":"loopback stalled HTTP body; no model calls","measurements":results}))
}
