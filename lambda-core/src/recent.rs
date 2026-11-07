//! 最近打开的归档：`data_dir/recent.json`，最多 20 条。
//!
//! 只记「怎么再打开它」：URL、名字、大小、是否加密、上次打开的时间。
//! 不记密码，也不记归档内容——SAF 的持久授权让 URL 在重启后依然有效。

use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::browser;

const LIMIT: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub encrypted: bool,
    pub opened_at: u64,
}

fn store_path() -> std::path::PathBuf {
    crate::locations::data_root().join("recent.json")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

impl Record {
    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "url": self.url,
            "size": self.size,
            "encrypted": self.encrypted,
            "opened_at": self.opened_at,
        })
    }

    fn from_value(value: &serde_json::Value) -> Option<Self> {
        Some(Self {
            id: value.get("id").and_then(|v| v.as_i64())?,
            name: value.get("name")?.as_str()?.to_string(),
            url: value.get("url")?.as_str()?.to_string(),
            size: value.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
            encrypted: value
                .get("encrypted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            opened_at: value.get("opened_at").and_then(|v| v.as_u64()).unwrap_or(0),
        })
    }

    /// 「12.3 MB · 3 天前」。加密标记交给单独的字段，界面自己画锁。
    pub fn detail_text(&self) -> String {
        format!(
            "{} · {}",
            browser::size_text(self.size),
            browser::modified_text(Some(UNIX_EPOCH + Duration::from_secs(self.opened_at)))
        )
    }
}

pub fn load() -> Vec<Record> {
    let value = crate::store::read_json(&store_path()).unwrap_or(serde_json::Value::Null);
    let mut records: Vec<Record> = value
        .as_array()
        .map(|items| items.iter().filter_map(Record::from_value).collect())
        .unwrap_or_default();
    records.sort_by(|a, b| b.opened_at.cmp(&a.opened_at));
    records
}

fn save(records: &[Record]) -> Result<(), String> {
    let array = serde_json::Value::Array(records.iter().map(Record::to_value).collect());
    crate::store::write_json(&store_path(), &array)
        .map_err(|error| format!("写入最近记录失败：{error}"))
}

/// 打开成功后记一笔：同一条 URL 只留一行，排到最前，最多 20 条。
pub fn touch(name: &str, url: &str, size: u64, encrypted: bool) -> Vec<Record> {
    let mut records = load();
    let id = crate::locations::id_of(url);
    records.retain(|record| record.id != id);
    records.insert(
        0,
        Record {
            id,
            name: name.to_string(),
            url: url.to_string(),
            size,
            encrypted,
            opened_at: now_secs(),
        },
    );
    records.truncate(LIMIT);
    let _ = save(&records);
    records
}

pub fn clear() {
    let _ = fs::remove_file(store_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_round_trips_through_json() {
        let record = Record {
            id: 3,
            name: "a.7z".into(),
            url: "content://tree/x/document/a".into(),
            size: 1024,
            encrypted: true,
            opened_at: 7,
        };
        let text = serde_json::to_string(&record.to_value()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(Record::from_value(&value), Some(record));
    }

    #[test]
    fn the_detail_line_reads_like_a_file_row() {
        let record = Record {
            id: 1,
            name: "a.7z".into(),
            url: "u".into(),
            size: 2048,
            encrypted: false,
            opened_at: now_secs(),
        };
        assert!(record.detail_text().contains("2.0 KB"));
        assert!(record.detail_text().contains("刚刚"));
    }
}
