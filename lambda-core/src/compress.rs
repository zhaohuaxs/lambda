//! 压缩任务的工作线程：读取 → 压缩 → 导出，每步都往 `Job` 里写进度。
//!
//! 三个阶段的精度不一样，界面必须照实说（`Lambda-Product-Plan.md` 5.3）：
//!
//! | 阶段 | 进度 | 精度 |
//! |---|---|---|
//! | 读取 | 已复制条目 / 总数 | 精确 |
//! | 压缩 | `7zz` 的 `-bsp1` 百分比 | 粗，失败时 `-1` |
//! | 导出 | 一整步（`export_path` 不报进度） | 无 |
//!
//! 取消：读取与导出逐项检查，压缩阶段只能等 `7zz` 返回——Android 是进程内
//! 调用 `main()`，没有中断钩子（5.4）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bern::media::MediaSource;
use bern::wake::Waker;

use crate::backend;
use crate::browser;
use crate::job::{Job, Outcome, Shared, Ticker};
use crate::record;

/// 递归读取时多久唤醒一次界面。见 `collect` 里的说明。
const WAKE_INTERVAL: Duration = Duration::from_millis(150);

/// 一次压缩要做的事。
pub struct Plan {
    /// `7z` 或 `zip`。
    pub kind: &'static str,
    /// 0–9，传给 `-mx`。
    pub level: u32,
    /// 空串表示不加密。
    pub password: String,
    /// 导出目标：位置的根 URI（SAF 树根 / 桌面目录）。
    pub root: String,
    /// 归档名，不含扩展名。
    pub name: String,
}

/// 工作线程里要压的一项（引擎侧的项目，比协议里的 `Entry` 多一个 URL）。
pub struct Input {
    pub name: String,
    pub url: String,
    pub folder: bool,
}

/// 起一个工作线程。`wake` 会把进度搬进状态，调用方立刻返回。
pub fn start(job: Shared, waker: Waker, inputs: Vec<Input>, plan: Plan) {
    std::thread::spawn(move || {
        let outcome = run(&job, &waker, inputs, plan);
        if let Ok(mut guard) = job.lock() {
            guard.outcome = Some(outcome);
        }
        waker.wake();
    });
}

fn run(job: &Shared, waker: &Waker, inputs: Vec<Input>, plan: Plan) -> Outcome {
    let staging = backend::scratch().join(format!("job-{}", stamp()));
    let total = inputs.len() as i64;
    let outcome = match collect(job, waker, &inputs, &staging) {
        Ok(locals) => compress_and_export(job, waker, &locals, &staging, total, &plan),
        Err(Failure::Cancelled) => Outcome::Cancelled,
        Err(Failure::Message(message)) => Outcome::Failed(message),
    };
    // 成功时产物已经导出，工作目录整个删掉；失败时也删，别把半个 staging
    // 留在 cache 里（M1 的日志残留就是这么被当成「解压出来的文件」的）。
    let _ = fs::remove_dir_all(&staging);
    outcome
}

enum Failure {
    Cancelled,
    Message(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

/// 递归复制里的「已取消」是一句普通错误消息，这里把它认回来。
fn cancel_or_message(message: String) -> Failure {
    if message == "已取消" {
        Failure::Cancelled
    } else {
        Failure::Message(message)
    }
}

/// 把选中的项复制进私有 staging。SAF 的文档不能直接交给 7zz，它要普通路径。
fn collect(
    job: &Shared,
    waker: &Waker,
    inputs: &[Input],
    staging: &Path,
) -> Result<Vec<PathBuf>, Failure> {
    Job::set_phase(job, "读取");
    // 盘上的记录只跟「阶段」走：万一进程在压缩中途被杀，下次启动能说清
    // 停在哪一步（`record.rs`）。
    record::set_phase("读取", 0);
    let mut locals = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        if cancelled(job) {
            return Err(Failure::Cancelled);
        }
        Job::step(job, "读取", index as i64, &input.name);
        waker.wake();
        // 文件夹用 `files::list` 自己递归（`import_folder` 会把整棵授权树
        // 都搬进来）；递归里每次取消都变成 `Failure::Cancelled`，而不是
        // 一句看不出原因的错误消息。回调同时把正在复制的名字写进任务，
        // 否则选一个文件夹时进度区会一直停在「读取 0/1」。
        let local = if input.folder {
            let last = std::cell::Cell::new(
                Instant::now()
                    .checked_sub(WAKE_INTERVAL)
                    .unwrap_or_else(Instant::now),
            );
            backend::import_folder_with(&input.url, staging, &|name| {
                if cancelled(job) {
                    return false;
                }
                Job::step(job, "读取", index as i64, name);
                // 一万个文件的目录里每个文件都唤醒一次太吵：150 ms 一次
                // 足够界面显示，`Waker` 的合并也省不下这份开销。
                if last.get().elapsed() >= WAKE_INTERVAL {
                    last.set(Instant::now());
                    waker.wake();
                }
                true
            })
            .map_err(cancel_or_message)?
        } else {
            backend::copy_file_to(&input.url, staging)?
        };
        locals.push(local);
    }
    Job::step(job, "读取", inputs.len() as i64, "");
    Ok(locals)
}

fn compress_and_export(
    job: &Shared,
    waker: &Waker,
    locals: &[PathBuf],
    staging: &Path,
    total: i64,
    plan: &Plan,
) -> Outcome {
    let archive = staging.join(format!("{}.{}", plan.name, extension_of(plan.kind)));
    let password = if plan.password.is_empty() {
        None
    } else {
        Some(plan.password.as_str())
    };
    Job::step(job, "压缩", total, &plan.name);
    record::set_phase("压缩", 0);
    waker.wake();
    let (slot, cancel) = match job.lock() {
        Ok(guard) => (guard.percent.clone(), guard.cancel.clone()),
        Err(_) => return Outcome::Failed("任务状态已损坏".into()),
    };
    // 压缩与导出都是阻塞调用，`7zz` 只会把百分比写进 `slot`，它自己不会
    // 唤醒任何人。没有这个定时器，界面在整个压缩期间一次都不刷新，进度
    // 区会一直停在「准备中」（2026-11-06 真机 384 MB 复现）。
    let _ticker = Ticker::start(waker.clone(), WAKE_INTERVAL);
    let created = backend::create_archive_with(
        &archive,
        locals,
        password,
        plan.kind,
        plan.level.min(9),
        slot,
        cancel,
    );
    // Windows 的取消是真的杀掉子进程，回来是一句「已取消」；Android 只能
    // 等 `7zz` 返回，所以在这里再看一次标志，两种取消都归成同一种结局。
    if cancelled(job) {
        return Outcome::Cancelled;
    }
    match created {
        Ok(bytes) => {
            // `export_path` 没有进度可报，一步到底：状态栏说清楚在干什么
            // 就够了，给一个假的分段进度反而更糟。
            Job::set_phase(job, "导出");
            record::set_phase("导出", 0);
            job.lock()
                .map(|guard| guard.percent.store(100, Ordering::Relaxed))
                .ok();
            waker.wake();
            match backend::publish(&archive, &MediaSource::new(&plan.root)) {
                Ok(target) => Outcome::Done(format!(
                    "已压缩 {} 项 → {}（{}）",
                    total,
                    display(&target),
                    browser::size_text(bytes)
                )),
                Err(message) => Outcome::Failed(format!("压缩完成但导出失败：{message}")),
            }
        }
        Err(message) => Outcome::Failed(message),
    }
}

fn cancelled(job: &Shared) -> bool {
    job.lock().map(|guard| guard.cancelled()).unwrap_or(false)
}

/// 导出结果可能是一句「已导出到 content://…」。那个 URI 的最后一段是
/// 整串百分号编码的文档 id（`primary%3ADownload%2F…%2F压缩包.7z`），直接
/// 显示给用户没法看（2026-11-06 真机）。让宿主把名字解出来，解不出再退回
/// 最后一段路径。
fn display(target: &str) -> String {
    let uri = target.strip_prefix("已导出到 ").unwrap_or(target);
    if let Some(name) = bern::files::name(uri) {
        return name;
    }
    uri.rsplit(['/', '\\']).next().unwrap_or(uri).to_string()
}

/// 工作目录用的唯一后缀：毫秒时间戳，任务串行，够用。
fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_archive_name_follows_the_selection() {
        assert_eq!(super::extension_of("7z"), "7z");
        assert_eq!(super::extension_of("zip"), "zip");
    }

    #[test]
    fn a_content_uri_is_shortened_to_its_last_part() {
        assert_eq!(
            display("已导出到 content://tree/primary%3ADownload/document/abc"),
            "abc"
        );
        assert_eq!(display("/storage/emulated/0/a/backup.7z"), "backup.7z");
    }
}

/// 归档扩展名就是格式名。留成函数是为了让「格式名」这件事只有一个来源。
fn extension_of(kind: &str) -> &str {
    if kind.eq_ignore_ascii_case("zip") {
        "zip"
    } else {
        "7z"
    }
}
