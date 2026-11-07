//! 位置表：用户授权过的文件夹，存在应用私有目录的 `locations.json`。
//!
//! M0 已经用真机验证过两件前提（`Lambda-Product-Plan.md` 0.2）：
//! `data_dir()` 可读写、`takePersistableUriPermission` 让授权跨进程有效。
//! 所以这里只是一张普通的表：读、写、按 URI 去重、按 id 删除。
//!
//! `id` 是 URI 的 FNV-1a 哈希，而不是自增序号——它要在 `locations.json`、
//! `Table.selection`、以及将来的任务记录之间保持一致，自增序号在「删一个再加
//! 一个」之后会复用，哈希不会。

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// 一条位置记录。字段保持最少：`uri` 是身份，`label` 只影响显示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: i64,
    pub label: String,
    pub uri: String,
    pub added_at: u64,
}

impl Record {
    pub fn new(uri: &str, label: &str) -> Self {
        Self {
            id: id_of(uri),
            label: label.to_string(),
            uri: uri.to_string(),
            added_at: now_secs(),
        }
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "uri": self.uri,
            "added_at": self.added_at,
        })
    }

    /// 只有一个字段缺失才算坏记录；`id` 可以由 URI 重新算出来，这样 M0 写的
    /// 老文件（`id` 是 `loc-xxxx` 字符串）也能直接读进来。
    fn from_value(value: &serde_json::Value) -> Option<Self> {
        let uri = value.get("uri")?.as_str()?.to_string();
        let label = value
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("目录")
            .to_string();
        let id = value
            .get("id")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| id_of(&uri));
        Some(Self {
            id,
            label,
            uri,
            added_at: value.get("added_at").and_then(|v| v.as_u64()).unwrap_or(0),
        })
    }
}

/// 应用私有目录；位置表住在这里，不需要任何权限。
pub fn data_root() -> PathBuf {
    bern::storage::data_dir()
        .or_else(bern::storage::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
}

pub fn store_path() -> PathBuf {
    data_root().join("locations.json")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 稳定 id：同一 URI 永远得到同一 id。
pub fn id_of(uri: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in uri.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    // `Table.key` 是 i64；清掉最高位，序列化到 Kotlin 的 Long 也不会变负数。
    (hash & 0x7fff_ffff_ffff_ffff) as i64
}

pub fn load() -> Vec<Record> {
    let text = fs::read_to_string(store_path()).unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    value
        .as_array()
        .map(|items| items.iter().filter_map(Record::from_value).collect())
        .unwrap_or_default()
}

pub fn save(items: &[Record]) -> Result<(), String> {
    let array = serde_json::Value::Array(items.iter().map(Record::to_value).collect());
    let text = serde_json::to_string_pretty(&array).map_err(|error| error.to_string())?;
    if let Some(parent) = store_path().parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(store_path(), text).map_err(|error| format!("写入目录表失败：{error}"))
}

/// 添加或更新（按 URI 去重）。已存在时保留原来的 `added_at`，因为「添加时间」
/// 指的是第一次授权的时间。
pub fn upsert(uri: &str, label: &str) -> Result<Vec<Record>, String> {
    let mut table = load();
    match table.iter_mut().find(|item| item.uri == uri) {
        Some(existing) => existing.label = label.to_string(),
        None => table.push(Record::new(uri, label)),
    }
    save(&table)?;
    Ok(table)
}

pub fn remove(id: i64) -> Result<Vec<Record>, String> {
    let mut table = load();
    table.retain(|item| item.id != id);
    save(&table)?;
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_stable_positive_and_distinct() {
        let a = id_of("content://tree/a");
        assert_eq!(a, id_of("content://tree/a"));
        assert_ne!(a, id_of("content://tree/b"));
        assert!(a > 0, "ids stay positive so they survive the trip to a Kotlin Long");
    }

    #[test]
    fn records_round_trip_through_json() {
        let item = Record::new("content://tree/x", "下载");
        let back = Record::from_value(&item.to_value()).unwrap();
        assert_eq!(item, back);
    }

    #[test]
    fn a_record_without_an_id_derives_one_from_its_uri() {
        // M0 写的老文件是 `id: "loc-xxxx"`，读的时候按 URI 重算即可。
        let value = serde_json::json!({
            "id": "loc-0a1b",
            "label": "lambda-m0",
            "uri": "content://tree/x",
            "added_at": 3,
        });
        let record = Record::from_value(&value).unwrap();
        assert_eq!(record.id, id_of("content://tree/x"));
        assert_eq!(record.label, "lambda-m0");
    }

    #[test]
    fn a_record_without_a_uri_is_skipped() {
        assert!(Record::from_value(&serde_json::json!({ "id": 1 })).is_none());
    }
}
