use super::*;

unsafe extern "C" fn count_context_reads(
    kind: u32,
    data: *mut std::ffi::c_void,
    statement: *mut std::ffi::c_void,
    _extra: *mut std::ffi::c_void,
) -> i32 {
    if kind == rusqlite::ffi::SQLITE_TRACE_STMT && !statement.is_null() {
        // SAFETY: SQLite supplies a live statement during this callback. The
        // Arc-backed counter outlives registration and is released after removal.
        unsafe {
            let sql = rusqlite::ffi::sqlite3_sql(statement.cast());
            if !sql.is_null() {
                let bytes = std::ffi::CStr::from_ptr(sql).to_bytes();
                if bytes.windows(b"message_json".len()).any(|w| w == b"message_json")
                    && bytes.windows(b"FROM context_entries".len()).any(|w| w == b"FROM context_entries")
                {
                    (*(data as *const std::sync::atomic::AtomicUsize))
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
    }
    0
}

#[tokio::test]
async fn idle_polls_read_metadata_until_new_input_requires_context() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = root("idle-context");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let script = factory(HashMap::from([("i-leader".into(), vec![Step::Message(reply(&"history ".repeat(16_000)))])]));
    let handle = start(config(&root, script)).await.unwrap();
    handle.input("i-leader", "one reply").await.unwrap();
    wait_event(&handle, "response_imported", 10_000).await;
    wait_instance_phase(&handle, "i-leader", "READY", 10_000).await;
    // Let any already-started idle pass finish before observing subsequent polls.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let count = Arc::new(AtomicUsize::new(0));
    let pointer = Arc::into_raw(count.clone()) as usize;
    handle
        .storage()
        .call(move |control| {
            // SAFETY: the callback only examines SQL and increments its live Arc.
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(
                    control.connection().handle(),
                    rusqlite::ffi::SQLITE_TRACE_STMT,
                    Some(count_context_reads),
                    pointer as *mut std::ffi::c_void,
                )
            }
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let idle_reads = count.load(Ordering::Relaxed);
    handle.input("i-leader", "new input must wake the model").await.unwrap();
    for _ in 0..200 {
        if count.load(Ordering::Relaxed) > idle_reads {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let active_reads = count.load(Ordering::Relaxed) - idle_reads;
    handle
        .storage()
        .call(move |control| {
            // SAFETY: remove the callback on its owning thread before releasing the
            // exact Arc reference transferred at registration.
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(control.connection().handle(), 0, None, std::ptr::null_mut());
                drop(Arc::from_raw(pointer as *const AtomicUsize));
            }
        })
        .await
        .unwrap();
    handle.shutdown().await.unwrap();
    assert_eq!(idle_reads, 0, "unchanged idle polls must not materialize historical message bodies");
    assert!(active_reads > 0, "new input must still load its authoritative context");
}

#[tokio::test]
async fn temporary_storage_saturation_does_not_stop_instance_discovery() {
    let root = root("storage-backpressure");
    std::fs::create_dir_all(root.dir.join("ws")).unwrap();
    let created = Arc::new(Mutex::new(Vec::new()));
    let observed = created.clone();
    let scripted = factory(HashMap::new());
    let mut configuration = config(&root, move |id: &str, profile: &KernelProfile| {
        observed.lock().unwrap().push(id.to_owned());
        scripted(id, profile)
    });
    configuration.storage_queue = 1;
    configuration.poll = Duration::from_secs(1);
    let handle = start(configuration).await.unwrap();
    for _ in 0..200 {
        if !created.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let storage = handle.storage().clone();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, held) = std::sync::mpsc::channel();
    let blocked = tokio::spawn(async move {
        storage
            .call(move |_| {
                let _ = entered.send(());
                let _ = held.recv_timeout(Duration::from_secs(10));
            })
            .await
    });
    started.await.unwrap();
    let storage = handle.storage().clone();
    let queued = tokio::spawn(async move { storage.call(|_| ()).await });
    tokio::task::yield_now().await;
    let full = handle.storage().call(|_| ()).await;
    tokio::time::sleep(Duration::from_millis(2_200)).await;
    release.send(()).unwrap();
    blocked.await.unwrap().unwrap();
    queued.await.unwrap().unwrap();
    let mut admitted = false;
    for _ in 0..200 {
        if handle
            .submit_user(cmd(
                "after-saturation",
                "create_instance",
                json!({
                    "id":"i-after-saturation", "workspace_ref":root.dir.join("ws")
                }),
            ))
            .await
            .is_ok()
        {
            admitted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    for _ in 0..300 {
        if created.lock().unwrap().iter().any(|id| id == "i-after-saturation") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let discovered = created.lock().unwrap().iter().any(|id| id == "i-after-saturation");
    let stopped = handle.shutdown().await;
    assert!(full.unwrap_err().contains("backpressure"), "the fixture must saturate the queue");
    assert!(
        admitted && discovered && stopped.is_ok(),
        "admitted={admitted}, discovered={discovered}, shutdown={stopped:?}"
    );
}
