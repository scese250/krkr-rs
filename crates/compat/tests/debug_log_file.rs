//! The `Debug` log file: a file that cannot be opened or written is
//! recorded, not dropped.

mod common;

use std::sync::{Mutex, OnceLock};

use common::vm_lock;
use compat::CompatHost;
use tjs::Tjs2Engine;

/// The records the capturing sink collected.
static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A `log` sink that keeps every record the test reads.
struct CaptureSink;

impl log::Log for CaptureSink {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        RECORDS
            .lock()
            .expect("the record lock")
            .push(record.args().to_string());
    }

    fn flush(&self) {}
}

/// Install the capturing sink and empty the records it collected.
///
/// The sink is process-global and can only be installed once, so the second
/// test reuses the one the first installed.
fn capture_records() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        log::set_boxed_logger(Box::new(CaptureSink)).expect("install the capturing sink");
        log::set_max_level(log::LevelFilter::Info);
    });
    RECORDS.lock().expect("the record lock").clear();
}

/// The records the `compat.debug` subsystem wrote.
fn debug_records() -> Vec<String> {
    let records = RECORDS.lock().expect("the record lock").clone();
    records
        .iter()
        .filter(|record| record.starts_with("compat.debug: "))
        .cloned()
        .collect()
}

/// Build a VM with the full compat surface installed and run `f` against it.
fn with_vm(f: impl FnOnce(&Tjs2Engine)) {
    let engine = Tjs2Engine::new().expect("create engine");
    let mut host = CompatHost::new();
    host.install(&engine).expect("install compat surface");
    f(&engine);
}

/// Reference: the log stream is opened with `fopen`, and a file that cannot
/// be opened marks the holder failed
/// (`reference/cpp/core/utils/DebugIntf.cpp:255-257`); a failed holder is
/// never opened again (`reference/cpp/core/utils/DebugIntf.cpp:242-243`).
/// `TVPStartLogToFile` calls the holder without checking anything
/// (`reference/cpp/core/utils/DebugIntf.cpp:483`), so the script call
/// succeeds and the failure never reaches the script.
#[test]
fn an_unopenable_log_file_is_recorded_once_and_never_retried() {
    let _lock = vm_lock();
    capture_records();
    let dir = common::unique_dir("krkr-debug-open-fail");
    std::fs::create_dir_all(&dir).expect("create the log directory");
    // The log file's own path is a directory, so it cannot be opened for
    // writing.
    std::fs::create_dir_all(dir.join("krkr.console.log")).expect("make the log file a directory");
    let literal = dir.to_string_lossy().replace('\\', "/");

    with_vm(|e| {
        // Every call succeeds: the failure is recorded, not raised.
        e.exec_script(
            &format!(
                "Debug.logLocation = '{literal}'; Debug.startLogToFile(); \
                 Debug.message('one'); Debug.message('two');"
            ),
            "t",
        )
        .expect("the log file failure must not reach the script");
    });

    let records = debug_records();
    assert_eq!(
        records.len(),
        1,
        "the failed open is recorded once and the file is not retried: {records:?}"
    );
    assert!(
        records[0].contains("failed to open"),
        "the record names the failure: {records:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reference: a write that does not complete closes the stream and marks the
/// holder failed (`reference/cpp/core/utils/DebugIntf.cpp:310-315`), and a
/// failed holder is never opened again
/// (`reference/cpp/core/utils/DebugIntf.cpp:242-243`). `TVPAddLog` calls the
/// holder without checking anything
/// (`reference/cpp/core/utils/DebugIntf.cpp:397`), so the script call
/// succeeds and the failure never reaches the script.
#[test]
fn an_unwritable_log_file_is_recorded_once_and_never_retried() {
    let _lock = vm_lock();
    capture_records();
    assert!(
        std::path::Path::new("/dev/full").exists(),
        "the write failure case needs the /dev/full device"
    );
    let dir = common::unique_dir("krkr-debug-write-fail");
    std::fs::create_dir_all(&dir).expect("create the log directory");
    // `/dev/full` fails every write with ENOSPC: the log file opens, and the
    // write that follows it does not.
    std::os::unix::fs::symlink("/dev/full", dir.join("krkr.console.log"))
        .expect("link the log file to /dev/full");
    let literal = dir.to_string_lossy().replace('\\', "/");

    with_vm(|e| {
        // Every call succeeds: the failure is recorded, not raised.
        e.exec_script(
            &format!(
                "Debug.logLocation = '{literal}'; Debug.startLogToFile(); \
                 Debug.message('one'); Debug.message('two');"
            ),
            "t",
        )
        .expect("the log file failure must not reach the script");
    });

    let records = debug_records();
    assert_eq!(
        records.len(),
        1,
        "the failed write is recorded once and the stream is closed: {records:?}"
    );
    assert!(
        records[0].contains("failed to write"),
        "the record names the failure: {records:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
