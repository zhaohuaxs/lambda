#![cfg(any(windows, target_os = "android"))]

use std::fs;

use sevenzip_src::{list, set_log_dir};

/// A log directory that cannot be created used to make listing run with its
/// output on NUL and return an empty archive. It must fail loudly instead.
#[test]
fn a_log_that_cannot_be_created_is_an_error_not_an_empty_archive() {
    let dir = std::env::temp_dir().join(format!("lambda-log-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let blocker = dir.join("not-a-directory");
    fs::write(&blocker, b"x").unwrap();
    set_log_dir(blocker);

    let archive = dir.join("fake.7z");
    fs::write(&archive, b"not an archive").unwrap();
    let error = list(&archive, None).unwrap_err().0;
    assert!(error.contains("引擎日志"), "unexpected message: {error}");

    let _ = fs::remove_dir_all(&dir);
}
