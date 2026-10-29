use super::stopped_observation;
use crate::jobs::{self, JobSpec, Journal};
use std::os::unix::process::CommandExt;

struct Probe {
    child: std::process::Child,
    dir: std::path::PathBuf,
}
impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn stop_observation_requires_absence_and_preserves_the_unknown_journal() {
    let child = std::process::Command::new("sleep").arg("60").process_group(0).spawn().unwrap();
    let mut probe = Probe { child, dir: std::env::temp_dir().join(format!("ta-work-stop-{}", uuid::Uuid::new_v4())) };
    std::fs::create_dir_all(&probe.dir).unwrap();
    let job = JobSpec {
        job_id: "old-operation".into(),
        program: "/bin/bash".into(),
        args: Vec::new(),
        cwd: "/tmp".into(),
        env: Vec::new(),
        deadline_ms: 0,
        token: "test-token".into(),
    };
    let journal = Journal {
        job_id: job.job_id.clone(),
        state: "OUTCOME_UNKNOWN".into(),
        command_hash: job.command_hash(),
        pid: Some(probe.child.id()),
        start_ticks: Some(jobs::start_ticks(probe.child.id()).unwrap()),
        boot_id: jobs::boot_id().unwrap(),
        exit_code: None,
        signal: None,
        started_ms: Some(jobs::now_ms()),
        finished_ms: None,
        starts: 1,
        cancel_saved: true,
    };
    jobs::atomic_json(&probe.dir.join("job.json"), &job).unwrap();
    jobs::atomic_json(&probe.dir.join("journal.json"), &journal).unwrap();
    let before = std::fs::read(probe.dir.join("journal.json")).unwrap();
    assert!(
        stopped_observation(&probe.dir, &job.job_id).is_none(),
        "unknown outcome and cancellation are not evidence of a stopped process"
    );
    assert!(probe.child.try_wait().unwrap().is_none(), "the observer must not signal the process");
    probe.child.kill().unwrap();
    probe.child.wait().unwrap();
    assert_eq!(stopped_observation(&probe.dir, &job.job_id).unwrap()["reason"], "recorded_group_gone");
    assert_eq!(std::fs::read(probe.dir.join("journal.json")).unwrap(), before);
    let mut no_identity = journal;
    no_identity.pid = None;
    jobs::atomic_json(&probe.dir.join("journal.json"), &no_identity).unwrap();
    assert!(stopped_observation(&probe.dir, &job.job_id).is_none());
    no_identity.state = "READY".into();
    no_identity.boot_id = "previous-boot".into();
    jobs::atomic_json(&probe.dir.join("journal.json"), &no_identity).unwrap();
    assert!(stopped_observation(&probe.dir, &job.job_id).is_none(), "READY can still accept GO even after reboot");
}
