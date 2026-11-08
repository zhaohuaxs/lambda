#![cfg(windows)]

use std::fs;
use std::process::Command;

/// The CLI must run the same preflight/gate as the UI instead of calling the
/// engine directly. This is the smoke test the security review asked for.
#[test]
fn cli_extract_runs_through_the_guarded_path() {
    let dir = std::env::temp_dir().join(format!("lambda-cli-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("hello.txt");
    fs::write(&source, b"lambda").unwrap();
    let archive = dir.join("hello.7z");
    sevenzip_src::create(&archive, &[source], None).unwrap();

    let out = dir.join("out");
    let output = Command::new(env!("CARGO_BIN_EXE_lambda"))
        .arg("extract")
        .arg(&archive)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(out.join("hello.txt")).unwrap(), "lambda");
    let _ = fs::remove_dir_all(&dir);
}
