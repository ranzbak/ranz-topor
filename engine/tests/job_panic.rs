//! M4b gate review B-1 (ruling C31): a background job whose closure panics must report `done()` and
//! hand the panic message to `wait()` instead of leaving the caller polling for ever.
use std::time::{Duration, Instant};
use topor_engine::control::{Job, JobError};

fn wait_done(job: &Job, limit: Duration) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < limit {
        if job.done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    job.done()
}

#[test]
fn job_ok_result_is_done_and_idempotent() {
    let job = Job::spawn(|| Ok("{\"x\":1}".to_string()));
    assert!(wait_done(&job, Duration::from_secs(10)));
    assert_eq!(job.wait(), Ok("{\"x\":1}".to_string()));
    assert_eq!(job.wait(), Ok("{\"x\":1}".to_string()));
}

#[test]
fn job_error_result_is_failed_not_panicked() {
    let job = Job::spawn(|| Err("bad".to_string()));
    assert!(wait_done(&job, Duration::from_secs(10)));
    assert_eq!(job.wait(), Err(JobError::Failed("bad".into())));
}

#[test]
fn job_panic_sets_done_and_wait_carries_the_message() {
    let job = Job::spawn(|| panic!("boom at step {}", 7));
    assert!(wait_done(&job, Duration::from_secs(10)), "done() never became true after a panic");
    match job.wait() {
        Err(JobError::Panicked(msg)) => assert!(msg.contains("boom at step 7"), "{msg}"),
        other => panic!("expected a panic error, got {other:?}"),
    }
    // idempotent: the second wait returns the same error
    assert!(matches!(job.wait(), Err(JobError::Panicked(m)) if m.contains("boom")));
}

#[test]
fn job_not_done_while_running() {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let job = Job::spawn(move || {
        rx.recv().ok();
        Ok("late".into())
    });
    std::thread::sleep(Duration::from_millis(50));
    assert!(!job.done());
    tx.send(()).unwrap();
    assert!(wait_done(&job, Duration::from_secs(10)));
    assert_eq!(job.wait(), Ok("late".into()));
}

#[test]
fn job_panic_with_static_str_payload() {
    let job = Job::spawn(|| std::panic::panic_any("plain str payload"));
    assert!(wait_done(&job, Duration::from_secs(10)));
    assert!(matches!(job.wait(), Err(JobError::Panicked(m)) if m.contains("plain str payload")));
}
