//! Bounded storage worker (§4.1): one thread owns the session Control; every
//! SQLite call is serialized through a bounded queue with explicit
//! backpressure, so synchronous SQLite never blocks async I/O threads.
//! Cancelling the waiting future does not undo the submitted command — the
//! caller re-queries by command_id (§4.1).

use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use teamagents_core::v2::Control;

type Task = Box<dyn FnOnce(&mut Control) + Send + 'static>;

#[derive(Clone)]
pub struct Storage {
    queue: SyncSender<Task>,
}

impl Storage {
    pub fn open(path: &std::path::Path, session_id: &str, create: bool, bound: usize) -> Result<Storage, String> {
        let mut control = Control::open(path, session_id, create)?;
        let (tx, rx) = sync_channel::<Task>(bound);
        std::thread::Builder::new()
            .name(format!("v2-store-{session_id}"))
            .spawn(move || {
                // when every sender is gone the worker exits; pending replies
                // are dropped with their cancelled waiters
                while let Ok(task) = rx.recv() {
                    task(&mut control);
                }
            })
            .map_err(|e| format!("storage worker spawn: {e}"))?;
        Ok(Storage { queue: tx })
    }

    /// Run one closure on the storage thread. A full queue is explicit
    /// backpressure (§4.1); a dead worker fails the waiter.
    pub async fn call<R, F>(&self, f: F) -> Result<R, String>
    where
        R: Send + 'static,
        F: FnOnce(&mut Control) -> R + Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task: Task = Box::new(move |control: &mut Control| {
            let _ = tx.send(f(control));
        });
        self.queue.try_send(task).map_err(|e| match e {
            TrySendError::Full(_) => "storage queue full: backpressure".to_string(),
            TrySendError::Disconnected(_) => "storage worker gone".to_string(),
        })?;
        rx.await.map_err(|_| "storage worker dropped the reply".to_string())
    }
}
