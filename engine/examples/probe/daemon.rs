use super::{ensure, init_root, now_ms, rpc, runner, store::Store, Request, Result};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot};

type Work = (Request, oneshot::Sender<std::result::Result<Value, String>>);

async fn query(tx: &mpsc::Sender<Work>, request: Request) -> Result<Value> {
    let (reply, rx) = oneshot::channel();
    tx.send((request, reply)).await.map_err(|_| "the storage queue is closed")?;
    rx.await?.map_err(Into::into)
}

pub async fn serve(root: &Path) -> Result<()> {
    init_root(root)?;
    let _lock = runner::file_lock(root)?;
    let mut store = Store::open(root)?;
    let mut runners = Vec::new();
    let job_root = root.join("job");
    // A fixed local job fixture makes daemon/runner parentage and lock inheritance testable.
    if job_root.join("job.json").exists() {
        let job: runner::Job = serde_json::from_slice(&std::fs::read(job_root.join("job.json"))?)?;
        store.prepare_job(&job.id)?;
        let socket = job_root.join("runner.sock");
        if rpc(&socket, "status").is_err() {
            runners.push(runner::spawn(&job_root, &job)?);
            let until = now_ms() + 3000;
            while rpc(&socket, "status").is_err() {
                ensure(now_ms() < until, "the runner is not ready")?;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let status = rpc(&socket, "status")?;
        if runner::terminal(status["journal"]["state"].as_str().unwrap_or("")) {
            store.import_receipt(&job.id, &status["journal"])?;
        } else {
            let state: String =
                store.db.query_row("SELECT state FROM operations WHERE id=?1", [&job.id], |r| r.get(0))?;
            if state == "PREPARED" {
                store.dispatch(&job.id)?;
            }
            rpc(&socket, store.recovery_method(&job.id)?)?;
        }
    }
    let (tx, mut rx) = mpsc::channel::<Work>(64);
    let writer = std::thread::spawn(move || {
        while let Some((request, reply)) = rx.blocking_recv() {
            let result = store.demo_command(&request.command_id, &request.method).map_err(|e| e.to_string());
            let _ = reply.send(result);
        }
    });
    let socket = root.join("daemon.sock");
    if socket.exists() {
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    let mut clients = tokio::task::JoinSet::new();
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    // A retained waiting instance consumes neither a thread nor the storage writer.
    let waiter = tokio::spawn(std::future::pending::<()>());
    loop {
        tokio::select! {
            _=stop_rx.recv()=>break,
            _=tick.tick()=>{
                let request=Request{version:1,command_id:format!("tick-{}",now_ms()),method:"tick".into()};
                query(&tx,request).await?;
                for child in &mut runners { let _=child.try_wait(); }
            },
            accepted=listener.accept(),if clients.len()<32=>{
                let (stream,_)=accepted?;
                let tx=tx.clone();
                let stop=stop_tx.clone();
                clients.spawn(async move {
                    let (read,mut write)=stream.into_split();
                    let mut reader=BufReader::new(read).take(65_537);
                    let mut bytes=Vec::new();
                    let result:Result<Value>=async {
                        tokio::time::timeout(Duration::from_secs(2),reader.read_until(b'\n',&mut bytes)).await??;
                        ensure(bytes.len()<=65_536,"control request too large")?;
                        let request:Request=serde_json::from_slice(&bytes)?;
                        ensure(request.version==1,"protocol version mismatch")?;
                        if request.method=="shutdown" {
                            stop.send(()).await?;
                            return Ok(json!({"ok":true}));
                        }
                        query(&tx,request).await
                    }.await;
                    let value=match result { Ok(value)=>value,Err(e)=>json!({"ok":false,"error":e.to_string()}) };
                    let _=write.write_all(format!("{value}\n").as_bytes()).await;
                });
            },
            Some(_)=clients.join_next(),if !clients.is_empty()=>{},
        }
    }
    waiter.abort();
    let _ = waiter.await;
    while clients.join_next().await.is_some() {}
    drop(tx);
    writer.join().map_err(|_| "the storage thread failed to exit")?;
    drop(listener);
    std::fs::remove_file(socket)?;
    Ok(())
}
