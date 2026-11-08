//! 解压任务的工作线程：把归档里选中/当前层的条目解到私有 staging，再按
//! 策略导出到 SAF 树根（`Lambda-Product-Plan.md` 8.2）。
//!
//! 四个策略只差「导出时套不套一层目录」：
//!
//! - 当前文件夹：选中的顶层项直接进位置根；只在位置根可用（SAF 只能写树根）。
//! - 智能解压：只选了一个文件夹就展开它的内容，否则套归档名。
//! - 归档名文件夹：总是套一层归档名。
//! - 其他位置：和「当前文件夹」一样直接导出，只是目标由用户另选。
//!
//! 冲突由导出端编号（`name (1).ext`），这里不删、不改名、不覆盖用户文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use bern::media::MediaSource;
use bern::wake::Waker;

use crate::backend;
use crate::job::{Job, Outcome, Shared, Ticker};

const WAKE_INTERVAL: Duration = Duration::from_millis(150);

/// 当前层里被选中的一项。`path` 是归档内的完整路径。
pub struct Output {
    pub path: String,
}

pub struct Plan {
    /// 归档的本地副本。
    pub archive: PathBuf,
    /// 空串表示不加密。
    pub password: String,
    /// 传给 7-Zip 的路径；空表示整个归档。
    pub paths: Vec<String>,
    /// 导出时要落到目标根的顶层项。
    pub outputs: Vec<Output>,
    /// 被选条目声明的总字节数（含文件夹的后代）。
    pub declared_bytes: u64,
    /// 被选中文件的条目数（含文件夹的后代）。
    pub entry_count: usize,
    /// 目标 SAF 树根 / 桌面目录。
    pub root: String,
    /// 「下载」「lambda-m0」这类目标名，写进结果消息。
    pub target_label: String,
    /// 0 当前 / 1 智能 / 2 归档名文件夹 / 3 其他位置。
    pub policy: i64,
    /// 归档名（不含扩展名），套目录时用。
    pub stem: String,
}

impl Drop for Plan {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}

pub fn start(job: Shared, waker: Waker, plan: Plan) {
    std::thread::spawn(move || {
        let outcome = run(&job, &waker, plan);
        let mut guard = job.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.outcome = Some(outcome);
        waker.wake();
    });
}

fn run(job: &Shared, waker: &Waker, plan: Plan) -> Outcome {
    let staging = match backend::fresh_dir("job") {
        Ok(staging) => staging,
        Err(message) => return Outcome::Failed(message),
    };
    let out = staging.join("extracted");
    let outcome = match gate(&plan, &staging) {
        Ok(()) => match extract(job, waker, &plan, &out) {
            Ok(bytes) => export(job, waker, &plan, &staging, &out, bytes),
            Err(Failure::Cancelled) => Outcome::Cancelled,
            Err(Failure::Message(message)) => Outcome::Failed(message),
        },
        Err(Failure::Cancelled) => Outcome::Cancelled,
        Err(Failure::Message(message)) => Outcome::Failed(message),
    };
    let _ = fs::remove_dir_all(&staging);
    outcome
}

fn gate(plan: &Plan, staging: &Path) -> Result<(), Failure> {
    let target = bern::files::file_path(&plan.root);
    let mut targets = vec![staging];
    if let Some(target) = target.as_deref() {
        // Staging is on the private cache volume; export lands on the
        // user-selected volume, which may be a different disk/card.
        targets.push(target);
    }
    sevenzip_src::check_extract_limits(
        &plan.archive,
        plan.declared_bytes,
        plan.entry_count,
        &targets,
    )
    .map_err(|error| Failure::Message(error.0))
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

fn extract(job: &Shared, waker: &Waker, plan: &Plan, out: &Path) -> Result<u64, Failure> {
    Job::set_phase(job, "解压");
    Job::step(job, "解压", 0, &plan.stem);
    waker.wake();
    let (slot, cancel) = match job.lock() {
        Ok(guard) => (guard.percent.clone(), guard.cancel.clone()),
        Err(_) => return Err(Failure::Message("任务状态已损坏".into())),
    };
    // Android 的引擎在进程内跑，阻塞期间只能靠定时器把百分比搬给界面；
    // Windows 会真的杀掉子进程，这里的取消标志两种平台都传下去。
    let _ticker = Ticker::start(waker.clone(), WAKE_INTERVAL);
    let password = (!plan.password.is_empty()).then_some(plan.password.as_str());
    let space = sevenzip_src::SpaceGuard::start(out, cancel.clone());
    let outcome = backend::extract_entries(&plan.archive, password, out, &plan.paths, slot, cancel);
    let bytes = match outcome {
        Ok(bytes) => bytes,
        Err(message) => {
            // The guard sets the same cancel flag the user does; check it
            // before translating the engine's generic "已取消".
            if space.exceeded() {
                return Err(Failure::Message(
                    "空间不足：解压过程中可用空间低于安全余量，已停止".into(),
                ));
            }
            return Err(if message == "已取消" {
                Failure::Cancelled
            } else {
                Failure::Message(message)
            });
        }
    };
    let space_exceeded = space.exceeded();
    drop(space);
    if !space_exceeded && job.lock().map(|guard| guard.cancelled()).unwrap_or(false) {
        return Err(Failure::Cancelled);
    }
    Ok(bytes)
}

fn export(
    job: &Shared,
    waker: &Waker,
    plan: &Plan,
    staging: &Path,
    out: &Path,
    bytes: u64,
) -> Outcome {
    Job::set_phase(job, "导出");
    job.lock()
        .map(|guard| guard.percent.store(100, Ordering::Relaxed))
        .ok();
    waker.wake();

    let sources = match sources(plan, out) {
        Ok(sources) => sources,
        Err(message) => return Outcome::Failed(message),
    };
    let (sources, wrapped) = match plan.policy {
        // 智能解压：单一项不再套一层——单个文件夹展开它的内容，单个文件
        // 直接落进目标根；选中多项时才用归档名包起来。
        1 if sources.len() == 1 => {
            if sources[0].is_dir() {
                match children_of(&sources[0]) {
                    Ok(children) if !children.is_empty() => (children, false),
                    Ok(_) => (sources, false),
                    Err(message) => return Outcome::Failed(message),
                }
            } else {
                (sources, false)
            }
        }
        1 | 2 => match wrap(staging, &plan.stem, &sources) {
            Ok(wrapper) => (vec![wrapper], true),
            Err(message) => return Outcome::Failed(message),
        },
        _ => (sources, false),
    };

    let mut exported = 0usize;
    for source in &sources {
        if job.lock().map(|guard| guard.cancelled()).unwrap_or(false) {
            return Outcome::Cancelled;
        }
        waker.wake();
        match backend::publish(source, &MediaSource::new(&plan.root)) {
            Ok(_) => exported += 1,
            Err(message) => {
                return Outcome::Failed(format!(
                    "解压完成但导出失败（已导出 {exported} 项）：{message}"
                ));
            }
        }
    }
    let what = if wrapped {
        format!(
            "{} 项到「{}」/{}/",
            plan.outputs.len(),
            plan.target_label,
            plan.stem
        )
    } else {
        format!("{} 项到「{}」", plan.outputs.len(), plan.target_label)
    };
    Outcome::Done(format!(
        "已解压 {what}（{}）",
        crate::browser::size_text(bytes)
    ))
}

/// 每个顶层项在 staging 里的本地路径。文件夹即使没有显式目录项，7-Zip 也会
/// 建出来；真没有就是解压失败，绝不能伪造一个空目录然后报「已解压」。
fn sources(plan: &Plan, out: &Path) -> Result<Vec<PathBuf>, String> {
    let root = out
        .canonicalize()
        .map_err(|error| format!("无法定位解压目录 {}：{error}", out.display()))?;
    let mut sources = Vec::new();
    for output in &plan.outputs {
        let local = out.join(output.path.replace('\\', "/"));
        if !local.exists() {
            return Err(format!("归档里没有这一项：{}", output.path));
        }
        let local = local
            .canonicalize()
            .map_err(|error| format!("无法定位 {}：{error}", local.display()))?;
        if !local.starts_with(&root) {
            return Err(format!("归档条目落在解压目录之外：{}", output.path));
        }
        sources.push(local);
    }
    if sources.is_empty() {
        return Err("没有要解压的项".into());
    }
    Ok(sources)
}

fn children_of(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        fs::read_dir(dir).map_err(|error| format!("无法读取 {}：{error}", dir.display()))?;
    Ok(entries
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>())
}

/// 在 staging 里建一个以归档名命名的目录，把顶层项搬进去。目标与源在同一
/// 个 staging 卷上，`rename` 就够；万一跨卷再退回复制。
fn wrap(staging: &Path, stem: &str, sources: &[PathBuf]) -> Result<PathBuf, String> {
    let wrapper = staging.join("wrap").join(backend::file_component(stem));
    fs::create_dir_all(&wrapper).map_err(|error| format!("无法创建导出目录：{error}"))?;
    for source in sources {
        let name = source
            .file_name()
            .map(|name| name.to_os_string())
            .unwrap_or_default();
        let target = backend::unique_path(&wrapper.join(name))?;
        if fs::rename(source, &target).is_err() {
            copy(source, &target, 0)?;
        }
    }
    Ok(wrapper)
}

fn copy(source: &Path, target: &Path, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err(format!("目录嵌套超过 64 层：{}", source.display()));
    }
    if source.is_dir() {
        fs::create_dir_all(target).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(source)
            .map_err(|error| format!("无法读取 {}：{error}", source.display()))?
            .flatten()
        {
            copy(&entry.path(), &target.join(entry.file_name()), depth + 1)?;
        }
        Ok(())
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(source, target)
            .map(|_| ())
            .map_err(|error| format!("无法复制到 {}：{error}", target.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(archive: &Path, declared_bytes: u64, entry_count: usize) -> Plan {
        Plan {
            archive: archive.to_path_buf(),
            password: String::new(),
            paths: Vec::new(),
            outputs: Vec::new(),
            declared_bytes,
            entry_count,
            root: String::new(),
            target_label: String::new(),
            policy: 0,
            stem: "a".into(),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lambda-extract-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_gate_refuses_too_many_entries() {
        let dir = temp("count");
        let archive = dir.join("a.7z");
        fs::write(&archive, b"x").unwrap();
        let plan = plan(&archive, 0, sevenzip_src::MAX_LIST_ENTRIES + 1);
        assert!(matches!(gate(&plan, &dir), Err(Failure::Message(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_gate_refuses_an_absurd_declared_ratio() {
        let dir = temp("bomb");
        let archive = dir.join("a.7z");
        fs::write(&archive, vec![0u8; 1024]).unwrap();
        let plan = plan(&archive, 64 * 1024 * 1024 * 1024, 1);
        assert!(matches!(gate(&plan, &dir), Err(Failure::Message(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_gate_accepts_a_normal_archive() {
        let dir = temp("ok");
        let archive = dir.join("a.7z");
        fs::write(&archive, vec![0u8; 1024]).unwrap();
        let plan = plan(&archive, 4096, 2);
        assert!(gate(&plan, &dir).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sources_refuse_missing_and_escaping_paths() {
        let dir = temp("sources");
        let out = dir.join("out");
        fs::create_dir_all(&out).unwrap();
        fs::write(dir.join("secret"), b"x").unwrap();
        let mut plan = plan(&dir.join("a.7z"), 0, 0);

        plan.outputs = vec![Output {
            path: "missing.txt".into(),
        }];
        assert!(sources(&plan, &out).is_err());

        plan.outputs = vec![Output {
            path: "../secret".into(),
        }];
        assert!(sources(&plan, &out).is_err());

        fs::write(out.join("ok.txt"), b"x").unwrap();
        plan.outputs = vec![Output {
            path: "ok.txt".into(),
        }];
        assert!(sources(&plan, &out).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }
}
