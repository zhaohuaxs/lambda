//! Platform seam between the protocol commands and the 7-Zip engine.
//!
//! Both targets compile the vendored 7-Zip sources and call the console
//! front-end in-process, so listing, preview, extraction and creation share
//! one implementation here. Bern hands the UI's `pick:` results over as
//! `MediaSource` URLs: Windows gives `file://`, Android gives `content://`
//! (a document, or a document tree for a folder). The engine understands
//! neither, so every input is first materialised as an ordinary local path
//! inside the app's private cache, and every result is published back out
//! through the host.

use bern::media::MediaSource;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub files: usize,
    pub bytes: u64,
    pub engine: String,
}

/// One item of an archive listing. `path` keeps the engine's separator; the
/// archive layer normalises it before showing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedEntry {
    pub path: String,
    pub size: u64,
    pub folder: bool,
    pub encrypted: bool,
}

/// A writable directory the app owns on every platform.
pub fn scratch() -> PathBuf {
    bern::storage::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("lambda")
}

fn ensure(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|error| format!("无法创建工作目录 {}：{error}", dir.display()))
}

/// A safe single path component (SAF display names can contain separators).
pub fn file_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_matches([' ', '.']);
    if trimmed.is_empty() {
        "archive".into()
    } else {
        trimmed.chars().take(120).collect()
    }
}

/// Copy the document at `url` into `dir` under its own name, keeping any file
/// already there. This is the M3 collection step: SAF documents have no local
/// path, and `7zz` needs one.
pub fn copy_file_to(url: &str, dir: &Path) -> Result<PathBuf> {
    if url.is_empty() {
        return Err("空的文件地址".into());
    }
    if let Some(path) = bern::files::file_path(url)
        && path.is_file()
    {
        return Ok(path);
    }
    let source = MediaSource::new(url);
    ensure(dir)?;
    let name = file_component(&source.file_name().unwrap_or_else(|| "file".into()));
    let target = unique_path(&dir.join(name));
    let mut reader = source.open().map_err(|error| error.message)?;
    let mut writer = fs::File::create(&target)
        .map_err(|error| format!("无法写入 {}：{error}", target.display()))?;
    io::copy(&mut reader, &mut writer).map_err(|error| format!("读取所选文件失败：{error}"))?;
    Ok(target)
}

/// Import a chosen folder into `dir` (`import_folder` on Android).
///
/// 🔴 不能用 `platform::import_folder`：它拿到任何 document URI 都会回溯到
/// **整棵授权树**（`rootDocument(tree)`），于是「选中 l1」会把 `lambda-m0`
/// 全部导入——2026-11-06 真机上表现成任务卡在「读取 l1」，因为树里有一个
/// 一万个文件的目录。所以这里用 `files::list` 自己递归，只走选中的那一支。
pub fn import_folder_to(url: &str, dir: &Path) -> Result<PathBuf> {
    import_folder_with(url, dir, &|_| true)
}

/// 同上，`visit` 每处理一个条目问一次：它的参数是条目名，返回 `false`
/// 表示取消。界面用这个名字显示「正在读取 xxx」，也让取消立刻生效。
pub fn import_folder_with(
    url: &str,
    dir: &Path,
    visit: &dyn Fn(&str) -> bool,
) -> Result<PathBuf> {
    if url.is_empty() {
        return Err("请先选择文件夹".into());
    }
    if let Some(path) = bern::files::file_path(url)
        && path.is_dir()
    {
        return Ok(path);
    }
    ensure(dir)?;
    let name = file_component(
        &bern::files::name(url).unwrap_or_else(|| "folder".into()),
    );
    let target = unique_path(&dir.join(name));
    let mut budget = MAX_ENTRIES;
    copy_tree(url, &target, 0, visit, &mut budget)?;
    Ok(target)
}

/// 一个文件夹能搬进来多少个条目（文件 + 子目录）。`many` 那种一万个文件
/// 的测试目录要能过，但不能没有上限——递归复制是内存与磁盘都吃的操作。
const MAX_ENTRIES: usize = 200_000;

/// 递归复制一个文档目录。`depth` 挡住环与病态嵌套，`budget` 挡住病态的
/// 大目录。
fn copy_tree(
    url: &str,
    target: &Path,
    depth: usize,
    visit: &dyn Fn(&str) -> bool,
    budget: &mut usize,
) -> Result<()> {
    if depth > 32 {
        return Err("文件夹嵌套超过 32 层".into());
    }
    ensure(target)?;
    let entries = bern::files::list(url).map_err(|error| error.message)?;
    for entry in entries {
        if *budget == 0 {
            return Err(format!("文件夹里的条目超过 {MAX_ENTRIES} 个"));
        }
        *budget -= 1;
        if !visit(&entry.name) {
            return Err("已取消".into());
        }
        let child = target.join(file_component(&entry.name));
        if entry.folder {
            copy_tree(&entry.url, &child, depth + 1, visit, budget)?;
        } else {
            let mut reader = bern::files::open(&entry.url).map_err(|error| error.message)?;
            let mut writer = fs::File::create(&child)
                .map_err(|error| format!("无法写入 {}：{error}", child.display()))?;
            io::copy(&mut reader, &mut writer)
                .map_err(|error| format!("读取 {} 失败：{error}", entry.name))?;
        }
    }
    Ok(())
}

/// Compress `inputs` into `archive`, reporting 0-100 into `slot` and stopping
/// as soon as `cancel` is set. The engine runs in-process on every platform,
/// so cancel arrives through 7-Zip's break counter at the next progress check.
pub fn create_archive_with(
    archive: &Path,
    inputs: &[PathBuf],
    password: Option<&str>,
    kind: &str,
    level: u32,
    slot: std::sync::Arc<std::sync::atomic::AtomicI64>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<u64> {
    #[cfg(any(windows, target_os = "android"))]
    {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = watch_cancel(cancel.clone(), done.clone());
        let outcome = sevenzip_src::create_with_progress(
            archive, inputs, password, kind, Some(level), slot,
        )
        .map(|report| report.bytes)
        .map_err(|error| error.0);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = watcher.join();
        if cancel.load(std::sync::atomic::Ordering::Relaxed) && outcome.is_err() {
            return Err("已取消".into());
        }
        outcome
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (archive, inputs, password, kind, level, slot, cancel);
        Err("本平台没有 7-Zip 引擎".into())
    }
}

/// A fresh staging path for an extraction result.
pub fn extract_staging(archive: &Path) -> Result<PathBuf> {
    let stem = archive
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "lambda-output".into());
    let path = scratch().join("extracted").join(file_component(&stem));
    if path.exists() {
        let _ = fs::remove_dir_all(&path);
    }
    ensure(&path)?;
    Ok(path)
}

/// A fresh staging path for a new archive.
pub fn create_staging(name: &str, fallback: &Path) -> Result<PathBuf> {
    let base = if name.trim().is_empty() {
        fallback
            .file_stem()
            .map(|s| format!("{}.7z", s.to_string_lossy()))
            .unwrap_or_else(|| "lambda.7z".into())
    } else {
        let raw = name.trim();
        if raw.to_ascii_lowercase().ends_with(".7z") {
            raw.to_string()
        } else {
            format!("{raw}.7z")
        }
    };
    let dir = scratch().join("created");
    ensure(&dir)?;
    Ok(unique_path(&dir.join(file_component(&base))))
}

/// Publish a local result into the folder the user picked. Android exports
/// into the document tree; desktop copies into the chosen directory. When
/// no destination was picked, the private path is returned as-is.
pub fn publish(local: &Path, dest: &MediaSource) -> Result<String> {
    if dest.url.is_empty() {
        return Ok(local.display().to_string());
    }

    #[cfg(target_os = "android")]
    {
        if let Some(path) = bern::files::file_path(&dest.url).filter(|p| p.is_dir()) {
            let target = unique_path(&path.join(local.file_name().unwrap_or_default()));
            copy_recursive(local, &target)?;
            return Ok(target.display().to_string());
        }
        bern::platform::export_path(local, &dest.url)
            .map(|uri| format!("已导出到 {uri}"))
            .map_err(|error| error.message)
    }

    #[cfg(not(target_os = "android"))]
    {
        let Some(path) = bern::files::file_path(&dest.url).filter(|p| p.is_dir()) else {
            return Ok(local.display().to_string());
        };
        let target = unique_path(&path.join(local.file_name().unwrap_or_default()));
        if target == local {
            return Ok(local.display().to_string());
        }
        copy_recursive(local, &target)?;
        Ok(target.display().to_string())
    }
}

pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().to_string());
    for index in 1..10_000 {
        let candidate = match &ext {
            Some(ext) => path.with_file_name(format!("{stem}-{index}.{ext}")),
            None => path.with_file_name(format!("{stem}-{index}")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn copy_recursive(source: &Path, target: &Path) -> Result<()> {
    if source.is_dir() {
        ensure(target)?;
        for entry in fs::read_dir(source)
            .map_err(|error| format!("无法读取 {}：{error}", source.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                continue;
            }
            copy_recursive(&entry.path(), &target.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = target.parent() {
            ensure(parent)?;
        }
        fs::copy(source, target)
            .map(|_| ())
            .map_err(|error| format!("无法复制到 {}：{error}", target.display()))
    }
}

// ---------------------------------------------------------------------------
// Engine. Both targets compile the vendored 7-Zip sources into the
// application and drive the console front-end in-process; there is no
// external 7z.exe or 7z.dll anymore.
// ---------------------------------------------------------------------------

pub fn probe_engine() -> Result<String> {
    #[cfg(any(windows, target_os = "android"))]
    {
        Ok(sevenzip_src::engine_name().into())
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        Err("本平台没有 7-Zip 引擎".into())
    }
}

pub fn extract_archive(archive: &Path, password: Option<&str>, out: &Path) -> Result<Report> {
    #[cfg(any(windows, target_os = "android"))]
    {
        let report = sevenzip_src::extract(archive, password, out).map_err(|error| error.0)?;
        Ok(Report {
            files: report.files,
            bytes: report.bytes,
            engine: sevenzip_src::engine_name().into(),
        })
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (archive, password, out);
        Err("本平台没有 7-Zip 引擎".into())
    }
}

pub fn create_archive(archive: &Path, inputs: &[PathBuf], password: Option<&str>) -> Result<Report> {
    #[cfg(any(windows, target_os = "android"))]
    {
        let report = sevenzip_src::create(archive, inputs, password).map_err(|error| error.0)?;
        Ok(Report {
            files: report.files,
            bytes: report.bytes,
            engine: sevenzip_src::engine_name().into(),
        })
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (archive, inputs, password);
        Err("本平台没有 7-Zip 引擎".into())
    }
}

/// Every item inside `archive`, in listing order.
pub fn list_entries(archive: &Path, password: Option<&str>) -> Result<Vec<ListedEntry>> {
    #[cfg(any(windows, target_os = "android"))]
    {
        sevenzip_src::list(archive, password)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| ListedEntry {
                        path: entry.path,
                        size: entry.size,
                        folder: entry.folder,
                        encrypted: entry.encrypted,
                    })
                    .collect()
            })
            .map_err(|error| error.0)
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (archive, password);
        Err("本平台没有 7-Zip 引擎".into())
    }
}

/// Stream one archive entry's bytes through the engine's stdout.
pub fn stream_entry<F>(archive: &Path, password: Option<&str>, entry: &str, sink: F) -> Result<()>
where
    F: FnMut(&[u8]) -> bool,
{
    #[cfg(any(windows, target_os = "android"))]
    {
        sevenzip_src::stream_entry(archive, password, entry, sink).map_err(|error| error.0)
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (archive, password, entry, sink);
        Err("本平台没有 7-Zip 引擎".into())
    }
}

/// Extract the whole archive, or only `paths`, reporting 0-100 into `slot`.
pub fn extract_entries(
    archive: &Path,
    password: Option<&str>,
    out: &Path,
    paths: &[String],
    slot: std::sync::Arc<std::sync::atomic::AtomicI64>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<u64> {
    #[cfg(any(windows, target_os = "android"))]
    {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = watch_cancel(cancel.clone(), done.clone());
        let outcome =
            sevenzip_src::extract_selected_with_progress(archive, password, out, paths, slot)
                .map(|report| report.bytes)
                .map_err(|error| error.0);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = watcher.join();
        if cancel.load(std::sync::atomic::Ordering::Relaxed) && outcome.is_err() {
            return Err("已取消".into());
        }
        outcome
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (archive, password, out, paths, slot, cancel);
        Err("本平台没有 7-Zip 引擎".into())
    }
}

/// Raises the engine's break flag once the user cancels.
#[cfg(any(windows, target_os = "android"))]
fn watch_cancel(
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    use std::sync::atomic::Ordering;
    std::thread::spawn(move || {
        while !done.load(Ordering::Relaxed) {
            if cancel.load(Ordering::Relaxed) {
                sevenzip_src::request_break();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    })
}

/// Create a zip at `archive` from local inputs.
pub fn create_zip(archive: &Path, inputs: &[PathBuf], password: Option<&str>) -> Result<Report> {
    #[cfg(any(windows, target_os = "android"))]
    {
        let report =
            sevenzip_src::create_with(archive, inputs, password, "zip").map_err(|e| e.0)?;
        Ok(Report {
            files: report.files,
            bytes: report.bytes,
            engine: sevenzip_src::engine_name().into(),
        })
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (archive, inputs, password);
        Err("本平台没有 7-Zip 引擎".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_component_rejects_separators_and_empty_names() {
        assert_eq!(file_component("..\\evil/name.7z"), "_evil_name.7z");
        assert_eq!(file_component("..."), "archive");
        assert_eq!(file_component("  a b .7z "), "a b .7z");
    }

    #[test]
    fn create_staging_appends_7z_once() {
        let dir = std::env::temp_dir().join(format!("lambda-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = create_staging("", Path::new("/tmp/photos")).unwrap();
        assert!(path.to_string_lossy().ends_with("photos.7z"));
        let path = create_staging("backup.7z", Path::new("/tmp/photos")).unwrap();
        assert!(path.to_string_lossy().ends_with("backup.7z"));
        assert!(!path.to_string_lossy().ends_with("backup.7z.7z"));
        let _ = fs::remove_dir_all(&dir);
    }
}
