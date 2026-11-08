#![cfg(any(windows, target_os = "android"))]

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64};
use std::time::{Duration, Instant};

/// The patched `TestBreakSignal()` must consult the per-call flag, otherwise
/// compression runs to completion despite the user cancelling.
#[test]
fn a_cancelled_compression_does_not_run_to_completion() {
    let dir = std::env::temp_dir().join(format!("lambda-cancel-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("blob.bin");
    fs::write(&input, vec![0u8; 32 * 1024 * 1024]).unwrap();
    let archive = dir.join("out.7z");
    let slot = Arc::new(AtomicI64::new(-1));
    let cancel = AtomicBool::new(true);

    let started = Instant::now();
    let result = sevenzip_src::create_with_progress(
        &archive,
        &[input],
        None,
        "7z",
        Some(9),
        slot,
        Some(&cancel),
    );
    let elapsed = started.elapsed();

    let message = result.unwrap_err().0;
    assert!(
        message.contains("取消"),
        "cancelled compression reported {message}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "cancel took {elapsed:?}; the engine probably ignored the flag"
    );
    let _ = fs::remove_dir_all(&dir);
}
