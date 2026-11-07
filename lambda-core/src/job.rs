//! 一次后台任务的运行时状态：工作线程写，`wake` 读。
//!
//! 为什么是 `Mutex` 而不是一堆原子：阶段、计数、文件名要一起读，分开读会
//! 在界面上拼出「读取 3/3 · 压缩中」这种错位。百分比单独用原子，因为
//! 引擎的 watchdog 线程每 120 ms 就要写一次，不值得去抢那把锁。
//!
//! 取消只置一个标志：Android 是进程内调用 `7zz main()`，没有中断钩子，
//! 只能等当前这次调用返回；读取与导出阶段逐文件检查，可以立刻停。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bern::wake::Waker;

/// 阻塞调用期间替工作线程定时唤醒界面。
///
/// 7-Zip 与 `export_path` 都不回调我们，`Job` 里的百分比就算一直在变，
/// 界面也只有收到 `wake` 才会去读。`Waker` 自带合并，所以按固定节拍
/// 敲不会堆积唤醒。
pub struct Ticker {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Ticker {
    pub fn start(waker: Waker, interval: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                // 引擎已经停了，再敲也没有人会读。
                if !waker.wake() {
                    break;
                }
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 任务的结局。工作线程放进 `Job::outcome`，引擎在 `wake` 里取走。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done(String),
    Failed(String),
    Cancelled,
}

/// 工作线程与引擎共享的一份任务状态。
#[derive(Debug)]
pub struct Job {
    /// 「压缩 3 项」
    pub title: String,
    /// 「读取」「压缩」「导出」
    pub phase: String,
    /// 正在处理的那一项
    pub label: String,
    /// 读取阶段是第几项（从 0 起），压缩阶段等于 total。
    pub done: i64,
    pub total: i64,
    /// 引擎侧百分比，`-1` 表示还没报告过。
    pub percent: Arc<AtomicI64>,
    pub cancel: Arc<AtomicBool>,
    pub outcome: Option<Outcome>,
}

pub type Shared = Arc<Mutex<Job>>;

impl Job {
    pub fn new(title: impl Into<String>, total: i64) -> Self {
        Self {
            title: title.into(),
            phase: "准备".into(),
            label: String::new(),
            done: 0,
            total,
            percent: Arc::new(AtomicI64::new(-1)),
            cancel: Arc::new(AtomicBool::new(false)),
            outcome: None,
        }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn percent(&self) -> i64 {
        self.percent.load(Ordering::Relaxed)
    }

    /// 「45%」/「准备中」：引擎还没报告百分比时，说清楚是在准备而不是卡住。
    pub fn percent_text(&self) -> String {
        match self.percent() {
            value if value < 0 => "准备中".into(),
            value => format!("{value}%"),
        }
    }

    /// 换阶段并记下当前项。拿不到锁（被 `wake` 短暂占住）就跳过，
    /// 进度丢一帧比整个任务卡住好。
    pub fn step(shared: &Shared, phase: &str, done: i64, label: &str) {
        if let Ok(mut job) = shared.lock() {
            job.phase = phase.to_string();
            job.done = done;
            job.label = label.to_string();
        }
    }

    /// 只换阶段，保留计数。
    pub fn set_phase(shared: &Shared, phase: &str) {
        if let Ok(mut job) = shared.lock() {
            job.phase = phase.to_string();
        }
    }

    pub fn request_cancel(shared: &Shared) {
        // 先取到那个原子标志再置位：锁本身只保护字段，`wake` 持锁的时间
        // 以微秒计。中毒也要能取消，否则任务会永远停不下来。
        let flag = match shared.lock() {
            Ok(job) => job.cancel.clone(),
            Err(poisoned) => poisoned.into_inner().cancel.clone(),
        };
        flag.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_job_knows_no_percentage_yet() {
        let job = Job::new("压缩 1 项", 1);
        assert_eq!(job.percent(), -1);
        assert_eq!(job.percent_text(), "准备中");
        assert!(!job.cancelled());
    }

    #[test]
    fn the_engine_percentage_reaches_the_text() {
        let job = Job::new("压缩 1 项", 1);
        job.percent.store(42, Ordering::Relaxed);
        assert_eq!(job.percent_text(), "42%");
    }

    #[test]
    fn a_cancel_request_is_visible_to_the_worker() {
        let shared: Shared = Arc::new(Mutex::new(Job::new("压缩 1 项", 3)));
        Job::request_cancel(&shared);
        assert!(shared.lock().unwrap().cancelled());
    }

    #[test]
    fn a_step_moves_phase_and_counter_together() {
        let shared: Shared = Arc::new(Mutex::new(Job::new("压缩 3 项", 3)));
        Job::step(&shared, "读取", 2, "b.txt");
        let job = shared.lock().unwrap();
        assert_eq!(
            (job.phase.as_str(), job.done, job.label.as_str()),
            ("读取", 2, "b.txt")
        );
    }
}
