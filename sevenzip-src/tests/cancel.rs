#![cfg(any(windows, target_os = "android"))]

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
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

/// Cancel after compression has started, not before the first progress
/// callback. The flag must be observed by a running engine.
#[test]
fn cancelling_a_running_compression_stops_the_engine() {
    let dir = std::env::temp_dir().join(format!("lambda-cancel-run-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("blob.bin");
    // Incompressible data keeps level 9 busy long enough to cancel mid-run.
    let mut data = vec![0u8; 64 * 1024 * 1024];
    let mut state = 0x1234_5678u32;
    for byte in &mut data {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *byte = (state >> 24) as u8;
    }
    fs::write(&input, data).unwrap();
    let archive = dir.join("out.7z");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let started = Instant::now();
    let handle = std::thread::spawn(move || {
        sevenzip_src::create_with_progress(
            &archive,
            &[input],
            None,
            "7z",
            Some(9),
            Arc::new(AtomicI64::new(-1)),
            Some(&flag),
        )
    });

    std::thread::sleep(Duration::from_millis(150));
    cancel.store(true, Ordering::Release);
    let message = handle.join().unwrap().unwrap_err().0;

    assert!(
        message.contains("取消"),
        "cancelled compression reported {message}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "running cancel took {:?}",
        started.elapsed()
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The extraction path uses the same per-call callback as compression. A
/// 1 GiB sparse zero file compresses to a tiny archive but keeps 7-Zip busy
/// writing output long enough to cancel mid-run.
#[test]
fn cancelling_a_running_extraction_stops_the_engine() {
    let dir = std::env::temp_dir().join(format!("lambda-cancel-extract-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("zeros.bin");
    let file = fs::File::create(&input).unwrap();
    file.set_len(1024 * 1024 * 1024).unwrap();
    drop(file);

    let archive = dir.join("zeros.7z");
    let prepared = sevenzip_src::create_with_progress(
        &archive,
        &[input],
        None,
        "7z",
        Some(1),
        Arc::new(AtomicI64::new(-1)),
        None,
    );
    assert!(prepared.is_ok(), "fixture create failed: {prepared:?}");

    let out = dir.join("out");
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let handle = std::thread::spawn(move || {
        sevenzip_src::extract_selected_with_progress(
            &archive,
            None,
            &out,
            &[],
            Arc::new(AtomicI64::new(-1)),
            Some(&flag),
        )
    });

    std::thread::sleep(Duration::from_millis(150));
    let cancel_at = Instant::now();
    cancel.store(true, Ordering::Release);
    let message = handle.join().unwrap().unwrap_err().0;
    println!("extraction cancel latency: {:?}", cancel_at.elapsed());

    assert!(
        message.contains("取消"),
        "cancelled extraction reported {message}"
    );
    assert!(
        cancel_at.elapsed() < Duration::from_secs(5),
        "extraction cancel took {:?}",
        cancel_at.elapsed()
    );
    let _ = fs::remove_dir_all(&dir);
}
