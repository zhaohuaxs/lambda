#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lambda-cli-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_cli(archive: &Path, out: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lambda"))
        .arg("extract")
        .arg(archive)
        .arg("-o")
        .arg(out)
        .output()
        .unwrap()
}

/// The CLI must run the same preflight/gate as the UI instead of calling the
/// engine directly. This is the smoke test the security review asked for.
#[test]
fn cli_extract_runs_through_the_guarded_path() {
    let dir = temp("guarded");
    let source = dir.join("hello.txt");
    fs::write(&source, b"lambda").unwrap();
    let archive = dir.join("hello.7z");
    sevenzip_src::create(&archive, &[source], None).unwrap();

    let out = dir.join("out");
    let output = run_cli(&archive, &out);
    assert!(
        output.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(out.join("hello.txt")).unwrap(), "lambda");
    let _ = fs::remove_dir_all(&dir);
}

/// `/WHOLEARCHIVE` must keep the registration objects of every compiled
/// format: a ZIP (not just 7z) has to be extractable through the CLI.
#[test]
fn cli_extract_handles_zip_codecs() {
    let dir = temp("zip");
    let source = dir.join("hello.txt");
    fs::write(&source, b"zip-lambda").unwrap();
    let archive = dir.join("hello.zip");
    sevenzip_src::create_with(&archive, &[source], None, "zip").unwrap();

    let out = dir.join("out");
    let output = run_cli(&archive, &out);
    assert!(
        output.status.success(),
        "cli failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(out.join("hello.txt")).unwrap(),
        "zip-lambda"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A small ZIP whose headers claim a 3 GiB output must be refused by the
/// preflight, before any file is written.
#[test]
fn cli_refuses_a_zip_that_declares_a_huge_output() {
    let dir = temp("bomb");
    let source = dir.join("hello.txt");
    fs::write(&source, b"bomb").unwrap();
    let archive = dir.join("bomb.zip");
    sevenzip_src::create_with(&archive, &[source], None, "zip").unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    patch_declared_size(&mut bytes, 3 * 1024 * 1024 * 1024);
    fs::write(&archive, bytes).unwrap();

    let out = dir.join("out");
    let output = run_cli(&archive, &out);
    assert!(
        !output.status.success(),
        "the gate accepted a fake 3 GiB zip"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("疑似解压炸弹"),
        "unexpected error: {stderr}"
    );
    assert!(
        !out.exists(),
        "the preflight must run before creating output"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Patch the uncompressed-size field in every local and central header. The
/// file is a normal one-entry ZIP, so the EOCD points at one central record.
fn patch_declared_size(bytes: &mut [u8], size: u32) {
    let eocd = (0..=bytes.len() - 22)
        .rev()
        .find(|&index| &bytes[index..index + 4] == b"PK\x05\x06")
        .expect("end of central directory");
    let count = u16::from_le_bytes([bytes[eocd + 10], bytes[eocd + 11]]) as usize;
    let mut central = u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    for _ in 0..count {
        assert_eq!(&bytes[central..central + 4], b"PK\x01\x02");
        bytes[central + 24..central + 28].copy_from_slice(&size.to_le_bytes());
        let local =
            u32::from_le_bytes(bytes[central + 42..central + 46].try_into().unwrap()) as usize;
        assert_eq!(&bytes[local..local + 4], b"PK\x03\x04");
        bytes[local + 22..local + 26].copy_from_slice(&size.to_le_bytes());
        let name = u16::from_le_bytes([bytes[central + 28], bytes[central + 29]]) as usize;
        let extra = u16::from_le_bytes([bytes[central + 30], bytes[central + 31]]) as usize;
        let comment = u16::from_le_bytes([bytes[central + 32], bytes[central + 33]]) as usize;
        central += 46 + name + extra + comment;
    }
}
