//! 任务记录：`data_dir/jobs/current.json`（产品规划 5.5）。
//!
//! SAF 路线下没有可用于数据类任务的 manifest 前台服务，应用退到后台就可能
//! 被系统回收，任务会无声地断在半路。能做的只有把「我正在做什么」写到盘
//! 上：下次启动读到一份没写完的记录，就说明上次没跑完，界面据此给一次重
//! 试（`Lambda-Product-Plan.md` 5.5 ①②④）。
//!
//! 记的不是进度（那个每 150 ms 都在变），而是**能不能重跑**：哪几个 URL、
//! 什么格式、级别、归档名、落到哪个位置根。密码一律不落盘——明文写进
//! JSON 比明文进 argv 更难解释，重试时让用户在密码框里重新输。
//!
//! 4000 字的记录写三次（开始 + 每个阶段）而不是每次进度都写：闪存的寿命比
//! 「停在哪个阶段」这点精度值钱。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 记录里的一项：`url` 是 SAF document URI，重启后依然有效（持久授权）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedInput {
    pub name: String,
    pub url: String,
    pub folder: bool,
}

impl SavedInput {
    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "url": self.url,
            "folder": self.folder,
        })
    }

    fn from_value(value: &serde_json::Value) -> Option<Self> {
        Some(Self {
            name: value.get("name")?.as_str()?.to_string(),
            url: value.get("url")?.as_str()?.to_string(),
            folder: value
                .get("folder")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }
}

/// 一次压缩任务的参数，够重跑就行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub title: String,
    pub phase: String,
    pub done: i64,
    pub total: i64,
    pub kind: String,
    pub level: u32,
    pub name: String,
    pub root: String,
    /// 原任务带密码。密码本身没存，重试时要重新输入。
    pub locked: bool,
    pub inputs: Vec<SavedInput>,
    pub started: u64,
}

impl Saved {
    pub fn new(
        title: &str,
        kind: &str,
        level: u32,
        name: &str,
        root: &str,
        locked: bool,
        inputs: Vec<SavedInput>,
    ) -> Self {
        Self {
            title: title.to_string(),
            phase: "准备".into(),
            done: 0,
            total: inputs.len() as i64,
            kind: kind.to_string(),
            level,
            name: name.to_string(),
            root: root.to_string(),
            locked,
            inputs,
            started: now_secs(),
        }
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "title": self.title,
            "phase": self.phase,
            "done": self.done,
            "total": self.total,
            "kind": self.kind,
            "level": self.level,
            "name": self.name,
            "root": self.root,
            "locked": self.locked,
            "inputs": self.inputs.iter().map(SavedInput::to_value).collect::<Vec<_>>(),
            "started": self.started,
        })
    }

    /// 没有 `inputs` 的记录等于没法重跑，直接当没有。
    fn from_value(value: &serde_json::Value) -> Option<Self> {
        let inputs: Vec<SavedInput> = value
            .get("inputs")?
            .as_array()?
            .iter()
            .filter_map(SavedInput::from_value)
            .collect();
        if inputs.is_empty() {
            return None;
        }
        Some(Self {
            title: value
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("压缩")
                .to_string(),
            phase: value
                .get("phase")
                .and_then(|v| v.as_str())
                .unwrap_or("读取")
                .to_string(),
            done: value.get("done").and_then(|v| v.as_i64()).unwrap_or(0),
            total: value
                .get("total")
                .and_then(|v| v.as_i64())
                .unwrap_or(inputs.len() as i64),
            kind: value
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("7z")
                .to_string(),
            level: value.get("level").and_then(|v| v.as_u64()).unwrap_or(5) as u32,
            name: value
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("压缩包")
                .to_string(),
            root: value.get("root").and_then(|v| v.as_str())?.to_string(),
            locked: value
                .get("locked")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            inputs,
            started: value.get("started").and_then(|v| v.as_u64()).unwrap_or(0),
        })
    }

    /// 一句话说明上次停在哪，给启动时的提示用。
    pub fn note(&self) -> String {
        let ago = match now_secs().saturating_sub(self.started) {
            seconds if seconds < 90 => "刚刚".to_string(),
            seconds if seconds < 5400 => format!("{} 分钟前", seconds / 60),
            seconds => format!("{} 小时前", seconds / 3600),
        };
        let locked = if self.locked { " · 带密码" } else { "" };
        format!(
            "上次任务未完成：{}（{}{locked}，停在「{}」）· {ago}",
            self.title, self.kind, self.phase
        )
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn path() -> PathBuf {
    crate::locations::data_root()
        .join("jobs")
        .join("current.json")
}

/// 任务开始：先落盘，再起工作线程。顺序反了就没有「进程被杀也留痕」。
pub fn begin(saved: &Saved) {
    let path = path();
    let Some(parent) = path.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let text = serde_json::to_string_pretty(&saved.to_value()).unwrap_or_default();
    let _ = crate::backend::write_atomic(&path, text.as_bytes());
}

/// 换阶段时更新一次；记录被人删掉就什么也不做。
pub fn set_phase(phase: &str, done: i64) {
    let Some(mut saved) = load() else { return };
    if saved.phase == phase && saved.done == done {
        return;
    }
    saved.phase = phase.to_string();
    saved.done = done;
    begin(&saved);
}

/// 任务收尾：无论成功、失败还是取消，记录都该消失，否则下次启动会误报。
pub fn finish() {
    let _ = fs::remove_file(path());
}

/// 启动时读一次。读到什么就说明上次没走到 `finish()`。
pub fn load() -> Option<Saved> {
    let value = crate::store::read_json(&path())?;
    Saved::from_value(&value)
}

/// 用户点了「忽略」。
pub fn discard() {
    finish();
}

/// 清掉上次留下的工作目录（5.5 ④）。只删 Lambda 自己建的 `job-`（任务）
/// 与 `open-`（归档浏览副本），别的没人建过。
pub fn sweep_staging() {
    let root = crate::backend::scratch();
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("job-") || name.starts_with("open-") {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved() -> Saved {
        Saved::new(
            "压缩 2 项",
            "7z",
            5,
            "压缩包",
            "content://tree/x",
            false,
            vec![
                SavedInput {
                    name: "a.txt".into(),
                    url: "content://tree/x/document/a".into(),
                    folder: false,
                },
                SavedInput {
                    name: "l1".into(),
                    url: "content://tree/x/document/l1".into(),
                    folder: true,
                },
            ],
        )
    }

    #[test]
    fn a_record_round_trips_through_json() {
        let original = saved();
        let text = serde_json::to_string(&original.to_value()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(Saved::from_value(&value), Some(original));
    }

    #[test]
    fn a_record_without_inputs_cannot_be_retried() {
        let mut value = saved().to_value();
        value["inputs"] = serde_json::json!([]);
        assert_eq!(Saved::from_value(&value), None);
        // `root` 缺了也没法重跑：产物不知道往哪写。
        let mut value = saved().to_value();
        value.as_object_mut().unwrap().remove("root");
        assert_eq!(Saved::from_value(&value), None);
    }

    #[test]
    fn the_note_says_which_phase_stopped() {
        let mut original = saved();
        original.phase = "压缩".into();
        original.started = now_secs();
        let note = original.note();
        assert!(note.contains("压缩 2 项"), "{note}");
        assert!(note.contains("停在「压缩」"), "{note}");
        assert!(note.contains("刚刚"), "{note}");
    }

    #[test]
    fn a_locked_record_says_so() {
        let mut original = saved();
        original.locked = true;
        original.started = now_secs();
        assert!(original.note().contains("带密码"), "{}", original.note());
    }
}
