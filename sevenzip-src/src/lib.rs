//! In-process 7-Zip 26.04 backend for Lambda (Windows + Android).
//!
//! Both platforms link the vendored 7zr front-end into the app binary
//! (Windows: `lambda.exe`; Android: `libbern_app.so`) and drive it through
//! `lambda7z_run` with a 7zr argv; there is no external `7z.exe` / `7z.dll`.
//! All paths handed to it are ordinary local files inside the app's private
//! storage: `lambda-core` materialises `content://` documents there first.

#[cfg(any(target_os = "android", windows))]
use std::ffi::{CStr, CString};
use std::fs;
#[cfg(any(target_os = "android", windows))]
use std::os::raw::{c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod encoding;
#[cfg(windows)]
mod zip_names;

/// Entries above this are refused before the archive browser allocates for
/// them. The cap is high enough for a normal photo/dataset archive and low
/// enough that a crafted header cannot exhaust a phone.
pub const MAX_LIST_ENTRIES: usize = 200_000;

/// The listing log may not grow past this. A crafted archive with millions of
/// tiny entries produces a log far larger than the entries themselves.
const MAX_LIST_LOG_BYTES: u64 = 64 * 1024 * 1024;

/// Engine logs live in a private directory, never next to the user's archive.
/// `lambda-core` points this at its own cache before the first engine call;
/// the CLI and host tests fall back to the system temp directory.
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
static LOG_SEQ: AtomicU64 = AtomicU64::new(0);

pub fn set_log_dir(dir: PathBuf) {
    let _ = LOG_DIR.set(dir);
}

fn log_path(tag: &str) -> PathBuf {
    let dir = LOG_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("lambda-logs"));
    let _ = fs::create_dir_all(&dir);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let seq = LOG_SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("{}-{stamp}-{seq}-{tag}.log", std::process::id()))
}

fn remove_log(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_file()
    {
        let _ = fs::remove_file(path);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub files: usize,
    pub bytes: u64,
    /// Tail of 7zr's stdout/stderr, useful for warnings.
    pub log: String,
}

pub fn engine_name() -> &'static str {
    option_env!("LAMBDA_7Z_ENGINE").unwrap_or("7-Zip 26.04 (variant unknown)")
}

/// One entry of `7zz l -slt`, the machine-readable listing that the archive
/// browser (M4) is built on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path inside the archive, `/`-separated.
    pub path: String,
    pub size: u64,
    pub folder: bool,
    pub encrypted: bool,
    /// Symbolic or hard link. Extraction is refused for these: creating a
    /// link inside staging lets a later entry write through it.
    pub link: bool,
}

/// Build the argv for `7zz l -slt`. `-sccUTF-8` keeps non-ASCII names from
/// being transliterated by the console charset conversion.
pub fn list_args(archive: &Path, password: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![exe().into(), "l".into(), "-slt".into(), "-sccUTF-8".into()];
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        args.push(format!("-p{password}"));
    }
    args.push(archive.display().to_string());
    args
}

/// Parse `7zz l -slt` output into entries.
///
/// The output has one property block for the archive itself, a `----------`
/// separator, then one block per item separated by blank lines. Only the
/// blocks after the separator are items.
pub fn parse_slt(text: &str) -> Result<Vec<Entry>, Error> {
    parse_slt_bytes(text.as_bytes())
}

/// List output may contain bytes that are not UTF-8: on a non-Windows build,
/// 7-Zip keeps a ZIP name without the UTF-8 flag as its original bytes, and
/// `-sccUTF-8` writes those bytes back unchanged. Reading the log as a
/// `String` would fail and turn a 30-item GBK archive into an empty listing,
/// so parse at the byte level and decode names separately. The encoding is
/// detected once from every legacy name in the archive: a single file name is
/// too short for reliable detection.
fn parse_slt_bytes(text: &[u8]) -> Result<Vec<Entry>, Error> {
    let mut raw_entries: Vec<RawEntry> = Vec::new();
    let mut in_items = false;
    let mut path: Option<Vec<u8>> = None;
    let mut size = 0u64;
    let mut folder = false;
    let mut encrypted = false;
    let mut link = false;
    let mut too_many = false;

    let flush = |path: &mut Option<Vec<u8>>,
                 size: &mut u64,
                 folder: &mut bool,
                 encrypted: &mut bool,
                 link: &mut bool,
                 entries: &mut Vec<RawEntry>,
                 too_many: &mut bool| {
        if let Some(path) = path.take() {
            if entries.len() >= MAX_LIST_ENTRIES {
                *too_many = true;
            } else {
                entries.push(RawEntry {
                    path,
                    size: *size,
                    folder: *folder,
                    encrypted: *encrypted,
                    link: *link,
                });
            }
        }
        *size = 0;
        *folder = false;
        *encrypted = false;
        *link = false;
    };

    for raw_line in text.split(|&byte| byte == b'\n') {
        if too_many {
            break;
        }
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if line.starts_with(b"----------") {
            in_items = true;
            continue;
        }
        if !in_items {
            continue;
        }
        if line.iter().all(|byte| byte.is_ascii_whitespace()) {
            flush(
                &mut path,
                &mut size,
                &mut folder,
                &mut encrypted,
                &mut link,
                &mut raw_entries,
                &mut too_many,
            );
            continue;
        }
        let Some(eq) = line.windows(3).position(|window| window == b" = ") else {
            continue;
        };
        let key = trim_ascii(&line[..eq]);
        let value = trim_ascii(&line[eq + 3..]);
        match key {
            b"Path" => path = Some(value.to_vec()),
            b"Size" => size = ascii(value).parse().unwrap_or(0),
            b"Folder" => folder = ascii(value) == "+",
            b"Encrypted" => encrypted = ascii(value) == "+",
            b"Symbolic Link" | b"Hard Link" => link = !value.is_empty(),
            _ => {}
        }
    }
    flush(
        &mut path,
        &mut size,
        &mut folder,
        &mut encrypted,
        &mut link,
        &mut raw_entries,
        &mut too_many,
    );
    if too_many {
        return Err(Error(format!(
            "归档条目超过 {MAX_LIST_ENTRIES} 项，暂不打开"
        )));
    }

    let paths: Vec<&[u8]> = raw_entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();
    let encoding = encoding::NameEncoding::detect(&paths);
    Ok(raw_entries
        .into_iter()
        .map(|entry| Entry {
            path: encoding.decode(&entry.path),
            size: entry.size,
            folder: entry.folder,
            encrypted: entry.encrypted,
            link: entry.link,
        })
        .collect())
}

struct RawEntry {
    path: Vec<u8>,
    size: u64,
    folder: bool,
    encrypted: bool,
    link: bool,
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |last| last + 1);
    &bytes[start..end]
}

fn ascii(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or("")
}

/// List `archive` through `7zz l -slt`. The textual output goes to a private
/// log file, which is read back here before it is removed.
pub fn list(archive: &Path, password: Option<&str>) -> Result<Vec<Entry>, Error> {
    if !archive.is_file() {
        return Err(Error(format!("归档不存在：{}", archive.display())));
    }
    let log = log_path("list");
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = LogGuard::start(log.clone(), MAX_LIST_LOG_BYTES, cancel.clone());
    let outcome = run(
        &with_codepage(archive, list_args(archive, password)),
        &log,
        Some(&cancel),
    );
    drop(guard);
    let read = fs::read(&log);
    remove_log(&log);
    if cancel.load(Ordering::Acquire) {
        return Err(Error(format!(
            "归档条目超过 {MAX_LIST_ENTRIES} 项，暂不打开"
        )));
    }
    outcome?;
    let bytes = read.map_err(|error| Error(format!("无法读取引擎日志：{error}")))?;
    if bytes.len() as u64 > MAX_LIST_LOG_BYTES {
        return Err(Error(format!(
            "归档条目超过 {MAX_LIST_ENTRIES} 项，暂不打开"
        )));
    }
    parse_slt_bytes(&bytes)
}

/// Build `7zz x -so` for one entry. File data goes to stdout; console
/// messages and progress are pushed to stderr so stdout stays a clean byte
/// stream. `-spd` keeps wildcard characters in the entry name literal.
pub fn preview_args(archive: &Path, password: Option<&str>, entry: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![
        exe().into(),
        "x".into(),
        "-so".into(),
        "-spd".into(),
        "-y".into(),
        "-bso2".into(),
        "-bse2".into(),
        "-bsp2".into(),
        "-bd".into(),
    ];
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        args.push(format!("-p{password}"));
    }
    args.push(archive.display().to_string());
    // Everything after `--` is an entry name. Without it, an entry called
    // `-p…`, `-o…`, `@list` or `*` is parsed as a switch or a list file.
    args.push("--".into());
    args.push(entry.to_string());
    args
}

/// Stream one entry's data through `7zz x -so`. The sink returns `false` to
/// stop early; the caller decides whether an early stop is a cancellation.
/// `cancel` is checked by 7-Zip itself, so the engine stops at its next
/// progress callback instead of waiting for a pipe write to fail.
pub fn stream_entry<F>(
    archive: &Path,
    password: Option<&str>,
    entry: &str,
    cancel: Option<&AtomicBool>,
    sink: F,
) -> Result<(), Error>
where
    F: FnMut(&[u8]) -> bool,
{
    if !archive.is_file() {
        return Err(Error(format!("归档不存在：{}", archive.display())));
    }
    let log = log_path("preview");
    let outcome = run_stream(
        &with_codepage(archive, preview_args(archive, password, entry)),
        &log,
        cancel,
        sink,
    );
    remove_log(&log);
    outcome
}

/// `argv[0]`: 7-Zip's console front-end reads it to decide its own name, but

/// the variant macro (not this string) decides which formats it offers.
fn exe() -> &'static str {
    option_env!("LAMBDA_7Z_EXE").unwrap_or("7z")
}

/// Windows only: the engine decodes legacy ZIP names through the OEM code
/// page before Rust sees them, so the detected page has to be forced with
/// `-mcp` or Big5/Shift-JIS archives come out as mojibake. On POSIX the
/// engine hands over the original bytes and `parse_slt_bytes` detects them.
fn with_codepage(archive: &Path, args: Vec<String>) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut args = args;
        if let Some(switch) = zip_names::codepage_switch(archive) {
            args.insert(2, switch);
        }
        args
    }
    #[cfg(not(windows))]
    {
        let _ = archive;
        args
    }
}

/// Build the argv for `7zz x`. Kept as a pure function so it can be unit
/// tested on the host, where the C++ engine is not compiled.
pub fn extract_args(archive: &Path, password: Option<&str>, out_dir: &Path) -> Vec<String> {
    let mut args = vec![
        exe().into(),
        "x".into(),
        "-y".into(),
        "-sccUTF-8".into(),
        // Progress goes to the same log the error message is read from; the
        // browser's extraction task needs a percentage for big archives.
        "-bsp1".into(),
        // Names arrive after `--`; keep wildcard characters literal and never
        // materialise archive links (26.04 would otherwise create them).
        "-spd".into(),
        "-snl-".into(),
        "-snh-".into(),
        format!("-o{}", out_dir.display()),
    ];
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        args.push(format!("-p{password}"));
    }
    args.push(archive.display().to_string());
    args.push("--".into());
    args
}

/// Build `7zz x` for a subset of an archive. Paths are archive-relative and
/// may name a directory (7-Zip extracts its whole subtree).
pub fn extract_selected_args(
    archive: &Path,
    password: Option<&str>,
    out_dir: &Path,
    paths: &[String],
) -> Vec<String> {
    let mut args = extract_args(archive, password, out_dir);
    args.extend(paths.iter().cloned());
    args
}

/// Build the argv for `7zz a`. `kind` is 7-Zip's format switch without the
/// dash (`7z`, `zip`). Password handling differs per format: 7z encrypts the
/// header too (`-mhe=on`, filenames hidden), zip has no encrypted header but
/// strong AES (`-mem=AES256`) — `-mhe=on` on zip is a hard error.
pub fn create_args(
    archive: &Path,
    inputs: &[PathBuf],
    password: Option<&str>,
    kind: &str,
    level: Option<u32>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        exe().into(),
        "a".into(),
        format!("-t{kind}"),
        "-y".into(),
        // Progress to stderr, which the bridge redirects into the log file the
        // worker polls. Without this 7-Zip writes progress to a console that
        // does not exist here.
        "-bsp1".into(),
    ];
    if let Some(password) = password.filter(|p| !p.is_empty()) {
        args.push(format!("-p{password}"));
        if kind.eq_ignore_ascii_case("7z") {
            args.push("-mhe=on".into());
        } else if kind.eq_ignore_ascii_case("zip") {
            args.push("-mem=AES256".into());
        }
    }
    if let Some(level) = level {
        args.push(format!("-mx={}", level.min(9)));
    }
    args.push(archive.display().to_string());
    args.extend(inputs.iter().map(|path| path.display().to_string()));
    args
}

/// Extract `archive` into `out_dir`, which is created when missing.
pub fn extract(archive: &Path, password: Option<&str>, out_dir: &Path) -> Result<Report, Error> {
    if !archive.is_file() {
        return Err(Error(format!("归档不存在：{}", archive.display())));
    }
    fs::create_dir_all(out_dir)?;
    let log = log_path("extract");
    let outcome = run(
        &with_codepage(archive, extract_args(archive, password, out_dir)),
        &log,
        None,
    );
    remove_log(&log);
    let output = outcome?;
    let (files, bytes) = count_tree(out_dir);
    Ok(Report {
        files,
        bytes,
        log: output,
    })
}

/// Extract the whole archive, or only `paths`, reporting progress into
/// `slot`. An empty path list means the whole archive.
pub fn extract_selected_with_progress(
    archive: &Path,
    password: Option<&str>,
    out_dir: &Path,
    paths: &[String],
    slot: Arc<AtomicI64>,
    cancel: Option<&AtomicBool>,
) -> Result<Report, Error> {
    if !archive.is_file() {
        return Err(Error(format!("归档不存在：{}", archive.display())));
    }
    fs::create_dir_all(out_dir)?;
    slot.store(-1, Ordering::Relaxed);
    let log = log_path("extract");
    let watcher = Watch::start(log.clone(), slot);
    let outcome = run(
        &with_codepage(
            archive,
            extract_selected_args(archive, password, out_dir, paths),
        ),
        &log,
        cancel,
    );
    drop(watcher);
    remove_log(&log);
    let output = outcome?;
    let (files, bytes) = count_tree(out_dir);
    Ok(Report {
        files,
        bytes,
        log: output,
    })
}

/// Create a fresh `.7z` at `archive` from local files / directories.
pub fn create(archive: &Path, inputs: &[PathBuf], password: Option<&str>) -> Result<Report, Error> {
    create_with(archive, inputs, password, "7z")
}

/// Create an archive of `kind` (`7z`, `zip`, …) at `archive`.
pub fn create_with(
    archive: &Path,
    inputs: &[PathBuf],
    password: Option<&str>,
    kind: &str,
) -> Result<Report, Error> {
    create_with_level(archive, inputs, password, kind, None)
}

/// Create with an explicit compression level (0 = store … 9 = ultra). A level
/// is what the M5 timing bench needs: the default `mx=5` on a 32 MB blob takes
/// tens of seconds on a phone and says nothing about the extraction path.
pub fn create_with_level(
    archive: &Path,
    inputs: &[PathBuf],
    password: Option<&str>,
    kind: &str,
    level: Option<u32>,
) -> Result<Report, Error> {
    create_inner(archive, inputs, password, kind, level, None, None)
}

/// Create an archive and report 7-Zip's progress into `slot` (0…100, `-1`
/// until the first line arrives).
///
/// The percentage comes from the `-bsp1` lines that the bridge redirects into
/// the same log file the error message is read from, so a watcher thread only
/// has to re-read the tail of that file. `slot` is shared with the caller's
/// job so a UI refresh can pick it up without a channel. It only ever grows:
/// 7-Zip restarts its own percentage when it writes the archive trailer
/// (`… 98% 87%` in a real run), and a bar that goes backwards is worse than a
/// slow one.
pub fn create_with_progress(
    archive: &Path,
    inputs: &[PathBuf],
    password: Option<&str>,
    kind: &str,
    level: Option<u32>,
    slot: Arc<AtomicI64>,
    cancel: Option<&AtomicBool>,
) -> Result<Report, Error> {
    slot.store(-1, Ordering::Relaxed);
    create_inner(archive, inputs, password, kind, level, Some(slot), cancel)
}

fn create_inner(
    archive: &Path,
    inputs: &[PathBuf],
    password: Option<&str>,
    kind: &str,
    level: Option<u32>,
    progress: Option<Arc<AtomicI64>>,
    cancel: Option<&AtomicBool>,
) -> Result<Report, Error> {
    if inputs.is_empty() {
        return Err(Error("没有要压缩的文件".into()));
    }
    for input in inputs {
        if !input.exists() {
            return Err(Error(format!("文件不存在：{}", input.display())));
        }
    }
    if let Some(parent) = archive.parent() {
        fs::create_dir_all(parent)?;
    }
    // `a` updates an existing archive; start from a clean one instead.
    let _ = fs::remove_file(archive);
    let log = log_path("create");
    let watcher = progress.map(|slot| Watch::start(log.clone(), slot));
    let outcome = run(
        &create_args(archive, inputs, password, kind, level),
        &log,
        cancel,
    );
    drop(watcher);
    remove_log(&log);
    let output = outcome?;
    let bytes = fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
    Ok(Report {
        files: 1,
        bytes,
        log: output,
    })
}

/// A thread that re-reads the tail of 7-Zip's log while it runs and keeps the
/// last percentage it saw in `slot`. Dropping it stops and joins the thread.
struct Watch {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Watch {
    fn start(log: PathBuf, slot: Arc<AtomicI64>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            // The log only grows; 120 ms keeps the bar responsive without
            // waking the phone's scheduler more than a few times a second.
            while !flag.load(Ordering::Relaxed) {
                if let Ok(tail) = read_tail(&log, 4096)
                    && let Some(percent) = last_percent(&tail)
                {
                    advance(&slot, percent);
                }
                std::thread::sleep(Duration::from_millis(120));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Stops a listing whose `-slt` log grows past `limit`. The 7-Zip header of a
/// crafted archive can declare millions of entries while the archive itself
/// stays a few megabytes; the log is where that turns into memory pressure.
struct LogGuard {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl LogGuard {
    fn start(log: PathBuf, limit: u64, cancel: Arc<AtomicBool>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                if let Ok(meta) = fs::metadata(&log)
                    && meta.len() > limit
                {
                    cancel.store(true, Ordering::Release);
                    break;
                }
                std::thread::sleep(Duration::from_millis(120));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Keep `slot` at the highest percentage seen.
///
/// 7-Zip restarts its own counter when it writes the archive trailer, so a real
/// run ends `… 98% 87%`. A bar that walks backwards is harder to explain than
/// one that waits, so the slot only ever grows.
pub fn advance(slot: &AtomicI64, percent: u8) {
    let _ = slot.fetch_max(percent as i64, Ordering::Relaxed);
}

/// The last 4 KB of a file, or what there is of it.
fn read_tail(path: &Path, limit: u64) -> Result<String, std::io::Error> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(limit)))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The last `NN%` in 7-Zip's progress output, if there is one.
///
/// The stream carries `\b` backspaces between refreshes and lines such as
/// ` 45% 12 - a.txt`, so the last match wins. Pure so the host can test it
/// without the engine.
pub fn last_percent(text: &str) -> Option<u8> {
    let bytes = text.as_bytes();
    let mut found = None;
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let mut start = index;
        while start > 0 && bytes[start - 1].is_ascii_digit() && index - start < 3 {
            start -= 1;
        }
        if start == index {
            continue;
        }
        if let Ok(value) = text[start..index].parse::<u8>()
            && value <= 100
        {
            found = Some(value);
        }
    }
    found
}

fn count_tree(dir: &Path) -> (usize, u64) {
    fn walk(path: &Path, files: &mut usize, bytes: &mut u64) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                walk(&entry.path(), files, bytes);
            } else if let Ok(meta) = entry.metadata() {
                *files += 1;
                *bytes += meta.len();
            }
        }
    }
    let mut files = 0;
    let mut bytes = 0;
    walk(dir, &mut files, &mut bytes);
    (files, bytes)
}

#[cfg(any(target_os = "android", windows))]
unsafe extern "C" {
    fn lambda7z_run(
        argc: c_int,
        argv: *const *const c_char,
        log_path: *const c_char,
        cancel: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
        cancel_user: *mut c_void,
        message: *mut c_char,
        message_cap: usize,
    ) -> c_int;
    fn lambda7z_run_stream(
        argc: c_int,
        argv: *const *const c_char,
        log_path: *const c_char,
        cancel: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
        cancel_user: *mut c_void,
        sink: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int,
        user: *mut c_void,
        message: *mut c_char,
        message_cap: usize,
    ) -> c_int;
}

/// Called by 7-Zip's `TestBreakSignal()` on the engine thread. Reads the same
/// flag the user's cancel command sets; no polling thread and no shared
/// counter, so a cancel cannot be lost or leak into the next call.
#[cfg(any(target_os = "android", windows))]
unsafe extern "C" fn cancel_flag(user: *mut c_void) -> c_int {
    if user.is_null() {
        return 0;
    }
    let flag = unsafe { &*(user as *const AtomicBool) };
    flag.load(Ordering::Acquire) as c_int
}

/// Ask Windows consoles to print UTF-8, so the Chinese CLI lines are readable.
pub fn enable_utf8_console() {
    #[cfg(windows)]
    unsafe {
        SetConsoleOutputCP(65001);
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleOutputCP(codepage: u32) -> i32;
}

#[cfg(any(target_os = "android", windows))]
unsafe extern "C" fn stream_sink(user: *mut c_void, data: *const u8, len: usize) -> c_int {
    // SAFETY: `run_stream` keeps the sink reference alive for the whole call
    // and the bridge calls this on one reader thread at a time.
    let sink = unsafe { &mut *(user as *mut &mut dyn FnMut(&[u8]) -> bool) };
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    if sink(bytes) { 1 } else { 0 }
}

#[cfg(any(target_os = "android", windows))]
fn run(args: &[String], log: &Path, cancel: Option<&AtomicBool>) -> Result<String, Error> {
    let owned: Vec<CString> = args
        .iter()
        .map(|arg| CString::new(arg.as_str()).map_err(|_| Error("参数含 NUL 字节".into())))
        .collect::<Result<_, _>>()?;
    let pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
    let log_path = CString::new(log.to_string_lossy().as_ref())
        .map_err(|_| Error("日志路径含 NUL 字节".into()))?;
    let mut message = vec![0 as c_char; 2048];
    let cancel_fn = cancel.map(|_| cancel_flag as unsafe extern "C" fn(*mut c_void) -> c_int);
    let cancel_user = cancel.map_or(std::ptr::null_mut(), |flag| {
        flag as *const AtomicBool as *mut c_void
    });
    let had_password = args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg.starts_with("-p") && arg.len() > 2);

    // SAFETY: all pointers stay alive for the call; the bridge only reads
    // argv and writes at most `message.len()` bytes into `message`.
    let code = unsafe {
        lambda7z_run(
            pointers.len() as c_int,
            pointers.as_ptr(),
            log_path.as_ptr(),
            cancel_fn,
            cancel_user,
            message.as_mut_ptr(),
            message.len(),
        )
    };
    let text = unsafe { CStr::from_ptr(message.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string();
    if code == 9 {
        return Err(Error("无法创建引擎日志：检查应用缓存目录权限".into()));
    }
    // 0 = success, 1 = warning (7-Zip's NExitCode).
    if code == 0 || code == 1 {
        Ok(text)
    } else {
        // 直接把整段日志丢给用户会看到版本横幅与版权声明；只留原因那一行。
        Err(Error(
            failure_message_ctx(&text, had_password)
                .unwrap_or_else(|| format!("7-Zip 退出码 {code}")),
        ))
    }
}

#[cfg(not(any(target_os = "android", windows)))]
fn run(_args: &[String], _log: &Path, _cancel: Option<&AtomicBool>) -> Result<String, Error> {
    Err(Error("本平台没有 7-Zip 引擎".into()))
}

#[cfg(any(target_os = "android", windows))]
fn run_stream<F>(
    args: &[String],
    log: &Path,
    cancel: Option<&AtomicBool>,
    mut sink: F,
) -> Result<(), Error>
where
    F: FnMut(&[u8]) -> bool,
{
    let owned: Vec<CString> = args
        .iter()
        .map(|arg| CString::new(arg.as_str()).map_err(|_| Error("参数含 NUL 字节".into())))
        .collect::<Result<_, _>>()?;
    let pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
    let log_path = CString::new(log.to_string_lossy().as_ref())
        .map_err(|_| Error("日志路径含 NUL 字节".into()))?;
    let mut message = vec![0 as c_char; 2048];
    let mut callback: &mut dyn FnMut(&[u8]) -> bool = &mut sink;
    let user = (&mut callback as *mut &mut dyn FnMut(&[u8]) -> bool).cast::<c_void>();
    let cancel_fn = cancel.map(|_| cancel_flag as unsafe extern "C" fn(*mut c_void) -> c_int);
    let cancel_user = cancel.map_or(std::ptr::null_mut(), |flag| {
        flag as *const AtomicBool as *mut c_void
    });
    let had_password = args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg.starts_with("-p") && arg.len() > 2);

    // SAFETY: argv, log_path, message and the sink reference stay alive for
    // the call; the bridge only reads argv and the sink until it returns.
    let code = unsafe {
        lambda7z_run_stream(
            pointers.len() as c_int,
            pointers.as_ptr(),
            log_path.as_ptr(),
            cancel_fn,
            cancel_user,
            stream_sink,
            user,
            message.as_mut_ptr(),
            message.len(),
        )
    };
    if code == 9 {
        return Err(Error("无法创建引擎日志：检查应用缓存目录权限".into()));
    }
    if code == 0 || code == 1 {
        return Ok(());
    }
    let text = unsafe { CStr::from_ptr(message.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string();
    Err(Error(
        failure_message_ctx(&text, had_password).unwrap_or_else(|| format!("7-Zip 退出码 {code}")),
    ))
}

#[cfg(not(any(target_os = "android", windows)))]
fn run_stream<F>(
    _args: &[String],
    _log: &Path,
    _cancel: Option<&AtomicBool>,
    _sink: F,
) -> Result<(), Error>
where
    F: FnMut(&[u8]) -> bool,
{
    Err(Error("本平台没有 7-Zip 引擎".into()))
}

/// 7-Zip writes a version banner before the actual reason. Keep the reason.
///
/// The strings below are the ones the真机补测 actually produced (M5 report,
/// 2026-11-06) — including the two ugly cases: a `path : reason` prefix, and
/// `Break signaled`, which is what 7-Zip says after it asked for a password on
/// a console that has none.
pub fn failure_message(output: &str) -> Option<String> {
    failure_message_ctx(output, false)
}

fn failure_message_ctx(output: &str, had_password: bool) -> Option<String> {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.iter().any(|line| line.contains("Enter password")) {
        return Some("需要密码：这个归档已加密".into());
    }
    let interesting = lines.iter().rev().find(|line| {
        line.starts_with("ERROR:")
            || line.contains("Cannot")
            || line.contains("Can not")
            || line.contains("Wrong password")
            || line.contains("Unsupported")
            || line.contains("not supported")
            || line.contains("Data Error")
            || line.contains("Headers Error")
            || line.contains("CRC Failed")
    });
    let chosen = interesting.copied().or_else(|| {
        // 没有可识别的措辞时，退回到最后一行——只要它不是版本横幅。
        lines
            .last()
            .copied()
            .filter(|line| !line.starts_with("7-Zip (") && !line.contains("Copyright"))
    })?;
    if chosen.contains("Break signaled") {
        // 7-Zip 在等密码时被 EOF 打断，对用户等于「缺密码」；已经给过密码
        // 还被打断，只可能是用户取消或引擎主动停止。
        return Some(if had_password {
            "已取消：引擎已停止".into()
        } else {
            "需要密码：这个归档已加密".into()
        });
    }
    // `<path> : reason` —— 路径对用户没有信息量，界面自己知道是哪个归档。
    let cleaned = match chosen.split_once(" : ") {
        Some((head, tail)) if head.starts_with('/') || head.contains(":\\") => tail.trim(),
        _ => chosen,
    };
    let cleaned = cleaned
        .trim_start_matches("Open ")
        .trim_start_matches("ERROR:")
        .trim();
    let cleaned = redact_private_paths(cleaned);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Strip private storage paths from an engine message before it reaches the
/// status bar. A path is not useful there and leaks the app sandbox layout.
fn redact_private_paths(text: &str) -> String {
    let mut out = text.to_string();
    for marker in ["/data/", "content://"] {
        if let Some(start) = out.find(marker) {
            let end = out[start..]
                .find(char::is_whitespace)
                .map(|offset| start + offset)
                .unwrap_or(out.len());
            out.replace_range(start..end, "<path>");
        }
    }
    if let Some(colon) = out.find(":\\") {
        let start = out[..colon]
            .rfind(char::is_whitespace)
            .map(|index| index + 1)
            .unwrap_or(0);
        let end = out[colon..]
            .find(char::is_whitespace)
            .map(|offset| colon + offset)
            .unwrap_or(out.len());
        if start < end {
            out.replace_range(start..end, "<path>");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_message_skips_the_version_banner() {
        let output = "7-Zip (z) 23.01 (arm64) : Copyright (c) 1999-2023 Igor Pavlov : 2023-06-20\n 64-bit arm64 thread\n\nOpen ERROR: Cannot open the file as [7z] archive\n";
        assert_eq!(
            failure_message(output).as_deref(),
            Some("Cannot open the file as [7z] archive")
        );
    }

    #[test]
    fn failure_message_falls_back_to_the_last_line() {
        assert_eq!(
            failure_message("7-Zip (z) 23.01\nsomething odd happened\n").as_deref(),
            Some("something odd happened")
        );
        assert_eq!(
            failure_message("7-Zip (z) 23.01 : Copyright (c) 1999\n"),
            None
        );
    }

    // 下面三条的字面值来自 2026-11-06 的真机 M5 补测报告。
    #[test]
    fn failure_message_drops_the_path_prefix() {
        let output = "/data/user/0/dev.bern.lambda/cache/lambda/bench/zero.7z : Cannot open the file as archive\n";
        assert_eq!(
            failure_message(output).as_deref(),
            Some("Cannot open the file as archive")
        );
        let windows = r"C:\tmp\a.zip : Cannot open the file as [zip] archive";
        assert_eq!(
            failure_message(windows).as_deref(),
            Some("Cannot open the file as [zip] archive")
        );
    }

    #[test]
    fn a_console_password_prompt_becomes_a_message_about_the_password() {
        // 真机上「无密码」列目录得到的正是 `Break signaled`。
        assert_eq!(
            failure_message("Break signaled").as_deref(),
            Some("需要密码：这个归档已加密")
        );
        assert_eq!(
            failure_message("Enter password (will not be echoed):\n").as_deref(),
            Some("需要密码：这个归档已加密")
        );
    }

    #[test]
    fn failure_message_keeps_the_wrong_password_reason() {
        let output = "/data/user/0/dev.bern.lambda/cache/bench/secure.7z : Cannot open encrypted archive. Wrong password?\n";
        assert_eq!(
            failure_message(output).as_deref(),
            Some("Cannot open encrypted archive. Wrong password?")
        );
    }

    #[test]
    fn extract_args_keeps_password_and_output_switch() {
        let args = extract_args(
            Path::new("/cache/in.7z"),
            Some("password123"),
            Path::new("/cache/out"),
        );
        assert_eq!(
            args[1..],
            [
                "x",
                "-y",
                "-sccUTF-8",
                "-bsp1",
                "-spd",
                "-snl-",
                "-snh-",
                "-o/cache/out",
                "-ppassword123",
                "/cache/in.7z",
                "--"
            ]
        );
    }

    #[test]
    fn selected_extraction_appends_the_archive_paths_after_the_sentinel() {
        let args = extract_selected_args(
            Path::new("a.7z"),
            None,
            Path::new("out"),
            &["sub".to_string(), "readme.txt".to_string()],
        );
        let sentinel = args.iter().position(|arg| arg == "--").expect("-- present");
        assert_eq!(sentinel, args.len() - 3, "archive name then -- then names");
        assert_eq!(args[args.len() - 2], "sub");
        assert_eq!(args.last().unwrap(), "readme.txt");
    }

    #[test]
    fn hostile_names_stay_after_the_sentinel() {
        let names = [
            "-pwrong".to_string(),
            "-o/tmp/x".to_string(),
            "-so".to_string(),
            "@/data/user/0/dev.bern.lambda/files/locations.json".to_string(),
            "*.txt".to_string(),
            "../escape".to_string(),
        ];
        let args = extract_selected_args(Path::new("a.7z"), None, Path::new("out"), &names);
        let sentinel = args.iter().position(|arg| arg == "--").expect("-- present");
        for name in &names {
            let index = args.iter().position(|arg| arg == name).expect(name);
            assert!(index > sentinel, "{name} must be a name, not a switch");
        }
    }

    #[test]
    fn empty_password_is_not_passed_to_the_engine() {
        let args = extract_args(Path::new("a.7z"), Some(""), Path::new("out"));
        assert!(!args.iter().any(|a| a.starts_with("-p")));
    }

    #[test]
    fn create_args_encrypts_the_header_only_for_7z() {
        let plain = create_args(Path::new("a.7z"), &[PathBuf::from("dir")], None, "7z", None);
        assert!(!plain.iter().any(|a| a == "-mhe=on" || a.starts_with("-p")));
        let secure = create_args(
            Path::new("a.7z"),
            &[PathBuf::from("dir")],
            Some("password123"),
            "7z",
            None,
        );
        assert!(secure.contains(&"-ppassword123".to_string()));
        assert!(secure.contains(&"-mhe=on".to_string()));
        assert_eq!(secure.last().unwrap(), "dir");
    }

    #[test]
    fn zip_uses_aes_and_never_the_header_switch() {
        let args = create_args(
            Path::new("a.zip"),
            &[PathBuf::from("dir")],
            Some("pw"),
            "zip",
            None,
        );
        assert!(args.contains(&"-tzip".to_string()));
        assert!(args.contains(&"-mem=AES256".to_string()));
        assert!(
            !args.contains(&"-mhe=on".to_string()),
            "7-Zip rejects -mhe on zip"
        );
    }

    #[test]
    fn level_is_clamped_and_optional() {
        let none = create_args(Path::new("a.7z"), &[PathBuf::from("dir")], None, "7z", None);
        assert!(!none.iter().any(|a| a.starts_with("-mx")));
        let fast = create_args(
            Path::new("a.7z"),
            &[PathBuf::from("dir")],
            None,
            "7z",
            Some(1),
        );
        assert!(fast.contains(&"-mx=1".to_string()));
        let absurd = create_args(
            Path::new("a.7z"),
            &[PathBuf::from("dir")],
            None,
            "7z",
            Some(42),
        );
        assert!(
            absurd.contains(&"-mx=9".to_string()),
            "levels above 9 are clamped"
        );
    }

    #[test]
    fn parse_slt_marks_symbolic_and_hard_links() {
        let text = "----------\nPath = link\nSize = 0\nFolder = -\nSymbolic Link = ../../etc/passwd\n\nPath = hard\nSize = 0\nHard Link = readme.txt\n\nPath = b.txt\nSize = 1\n\n";
        let entries = parse_slt(text).unwrap();
        assert!(entries[0].link);
        assert!(entries[1].link);
        assert!(!entries[2].link);
    }

    #[test]
    fn a_cancel_after_a_password_is_not_reported_as_a_missing_password() {
        assert_eq!(
            failure_message_ctx("Break signaled", true).as_deref(),
            Some("已取消：引擎已停止")
        );
        assert_eq!(
            failure_message_ctx("Break signaled", false).as_deref(),
            Some("需要密码：这个归档已加密")
        );
    }

    #[test]
    fn private_paths_are_redacted_from_engine_messages() {
        let message =
            failure_message("ERROR: Can not open /data/user/0/dev.bern.lambda/cache/x.7z").unwrap();
        assert!(!message.contains("/data/"));
        assert!(message.contains("<path>"));
    }

    #[test]
    fn parse_slt_skips_the_archive_block() {
        let text = "7-Zip (z) 23.01 : Copyright\n\nListing archive: t.7z\n\n--\nPath = t.7z\nType = 7z\nPhysical Size = 222\n\n----------\nPath = single.txt\nSize = 6\nFolder = -\nEncrypted = +\n\nPath = sub\nSize = 0\nFolder = +\nAttributes = D\n\nPath = sub/b.txt\nSize = 5\n\n";
        let entries = parse_slt(text).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path, "single.txt");
        assert_eq!(entries[0].size, 6);
        assert!(entries[0].encrypted);
        assert!(entries[1].folder);
        assert_eq!(entries[2].path, "sub/b.txt");
        assert!(!entries[2].folder);
    }

    #[test]
    fn parse_slt_handles_crlf_and_a_last_block_without_a_blank_line() {
        let text = "----------\r\nPath = a\\b.txt\r\nSize = 1\r\n";
        let entries = parse_slt(text).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].size, 1);
    }

    #[test]
    fn parse_slt_decodes_a_gbk_name_kept_as_raw_bytes() {
        // 亮剑/第1集_….xml in GBK: no UTF-8 flag, so 7-Zip on Android writes
        // the original bytes into the -slt log.
        let mut text = b"7-Zip (z) 23.01\n----------\nPath = ".to_vec();
        text.extend_from_slice(&[
            0xC1, 0xC1, 0xBD, 0xA3, b'/', 0xB5, 0xDA, b'1', 0xBC, 0xAF, b'_', b'6', b'6', b'9',
            b'9', b'4', b'0', b'5', b'8', b'0', b'6', b'4', b'2', b'2', b'4', b'5', b'8', b'8',
            b'9', b'4', b'.', b'x', b'm', b'l',
        ]);
        text.extend_from_slice(b"\nSize = 702666\nFolder = -\n\n");
        let entries = parse_slt_bytes(&text).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "亮剑/第1集_6699405806422458894.xml");
        assert_eq!(entries[0].size, 702666);
        assert!(!entries[0].folder);
    }

    #[test]
    fn parse_slt_keeps_utf8_names_untouched() {
        let text = "----------\nPath = 亮剑/第1集.xml\nSize = 1\n\n";
        let entries = parse_slt(text).unwrap();
        assert_eq!(entries[0].path, "亮剑/第1集.xml");
    }

    #[test]
    fn parse_slt_decodes_the_archive_with_one_detected_encoding() {
        let mut text = b"7-Zip (z) 23.01\n----------\n".to_vec();
        for name in ["編碼測試", "編碼測試/測試檔案.txt"] {
            let (bytes, _, had_errors) = encoding_rs::BIG5.encode(name);
            assert!(!had_errors);
            text.extend_from_slice(b"Path = ");
            text.extend_from_slice(&bytes);
            text.extend_from_slice(b"\nSize = 1\nFolder = -\n\n");
        }
        let entries = parse_slt_bytes(&text).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "編碼測試");
        assert_eq!(entries[1].path, "編碼測試/測試檔案.txt");
    }

    #[test]
    fn preview_args_stream_one_entry_to_clean_stdout() {
        let args = preview_args(Path::new("a.zip"), Some("pw"), "sub/a.txt");
        assert!(args.contains(&"-so".to_string()));
        assert!(args.contains(&"-spd".to_string()));
        assert!(args.contains(&"-bso2".to_string()));
        assert!(args.contains(&"-bse2".to_string()));
        assert!(args.contains(&"-bsp2".to_string()));
        assert!(args.contains(&"-bd".to_string()));
        assert!(args.contains(&"-ppw".to_string()));
        assert!(
            !args.iter().any(|arg| arg.starts_with("-o")),
            "no directory output"
        );
        assert_eq!(args.last().unwrap(), "sub/a.txt");
    }

    #[test]
    fn preview_args_omit_an_empty_password() {
        let args = preview_args(Path::new("a.7z"), None, "a.txt");
        assert!(!args.iter().any(|arg| arg.starts_with("-p")));
    }

    #[test]
    fn the_last_percentage_wins() {
        assert_eq!(last_percent(" 12% 1 - a.txt\n 45% 2 - b.txt"), Some(45));
        assert_eq!(last_percent("\u{8}\u{8}\u{8}  7%"), Some(7));
        assert_eq!(last_percent("100%"), Some(100));
        assert_eq!(last_percent("no progress here"), None);
        assert_eq!(
            last_percent("200%"),
            None,
            "percentages above 100 are noise"
        );
        // A stray `%` with no digits before it is not progress.
        assert_eq!(last_percent("100 % done"), None);
    }

    #[test]
    fn the_percentage_never_walks_backwards() {
        let slot = AtomicI64::new(-1);
        advance(&slot, 0);
        advance(&slot, 98);
        // 7-Zip's own trailer pass reports a smaller number; the bar stays put.
        advance(&slot, 87);
        assert_eq!(slot.load(Ordering::Relaxed), 98);
        advance(&slot, 100);
        assert_eq!(slot.load(Ordering::Relaxed), 100);
    }

    #[test]
    fn the_tail_is_the_end_of_the_file() {
        let dir = std::env::temp_dir().join("lambda-tail-test");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("t.log");
        fs::write(&path, "old\n 88%").unwrap();
        let tail = read_tail(&path, 4).unwrap();
        assert_eq!(tail, " 88%", "four bytes from the end");
        assert_eq!(read_tail(&path, 4096).unwrap(), "old\n 88%");
    }

    #[test]
    fn host_engine_matches_the_platform() {
        let log = std::env::temp_dir().join("lambda-engine-run-test.log");
        let _ = fs::remove_file(&log);
        let result = run(&["7zz".into()], &log, None);
        let _ = fs::remove_file(&log);
        #[cfg(any(windows, target_os = "android"))]
        assert!(result.is_ok());
        #[cfg(not(any(windows, target_os = "android")))]
        assert!(result.is_err());
    }
}
