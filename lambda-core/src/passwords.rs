//! 密码簿：`data_dir/passwords.json` 里只放标签，密文全部进系统凭据库
//! （`bern::secrets`：Windows Credential Manager / Android Keystore /
//! macOS Keychain / Linux Secret Service）。
//!
//! 这条分工是产品约束（`Lambda-Product-Plan.md` 5.5）：任务记录与应用文件
//! 都不落密码。标签不是秘密，放 JSON 才能在没有凭据库的测试环境里也把
//! 「有哪些保存项」说清楚；值读不出来时按「没有」处理，绝不回退到明文。

use std::path::PathBuf;

use bern::CommandError;

/// 一条保存项的元信息。`id` 由标签算出：同一标签更新同一条，不会越存越多。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: String,
    pub label: String,
    pub created: u64,
}

pub fn data_root() -> PathBuf {
    crate::locations::data_root()
}

fn store_path() -> PathBuf {
    data_root().join("passwords.json")
}

fn secret_name(id: &str) -> String {
    format!("pw.{id}")
}

fn checked_label(label: &str) -> Result<String, CommandError> {
    let label = label.trim();
    if label.is_empty() {
        return Err(CommandError::new("password", "标签不能为空"));
    }
    Ok(label.chars().take(60).collect())
}

fn id_of(label: &str) -> String {
    format!("{:x}", crate::locations::id_of(label))
}

pub fn load() -> Vec<Record> {
    let value = crate::store::read_json(&store_path()).unwrap_or(serde_json::Value::Null);
    value
        .as_array()
        .map(|items| items.iter().filter_map(Record::from_value).collect())
        .unwrap_or_default()
}

impl Record {
    /// 「保存于 3 天前」。和文件列表用同一种相对时间，避免在 `std` 里手算时区。
    pub fn note(&self) -> String {
        if self.created == 0 {
            return "保存时间未知".into();
        }
        format!(
            "保存于 {}",
            crate::browser::modified_text(Some(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(self.created)
            ))
        )
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "label": self.label,
            "created": self.created,
        })
    }

    fn from_value(value: &serde_json::Value) -> Option<Self> {
        Some(Self {
            id: value.get("id")?.as_str()?.to_string(),
            label: value.get("label")?.as_str()?.to_string(),
            created: value.get("created").and_then(|v| v.as_u64()).unwrap_or(0),
        })
    }
}

fn store(records: &[Record]) -> Result<(), CommandError> {
    let array = serde_json::Value::Array(records.iter().map(Record::to_value).collect());
    crate::store::write_json(&store_path(), &array)
        .map_err(|error| CommandError::new("password", format!("写入密码簿失败：{error}")))
}

/// 存一条密码。密文进系统凭据库；标签进 JSON。写 JSON 失败时把刚写入的
/// 密文删掉，免得留下一条谁也找不到的凭据。
pub fn save(label: &str, password: &str) -> Result<Record, CommandError> {
    let label = checked_label(label)?;
    if password.is_empty() {
        return Err(CommandError::new("password", "密码不能为空"));
    }
    let id = id_of(&label);
    let mut records = load();
    let created = now_secs();
    let record = match records.iter_mut().find(|record| record.id == id) {
        Some(existing) => {
            existing.label = label;
            existing.clone()
        }
        None => {
            let record = Record {
                id: id.clone(),
                label,
                created,
            };
            records.push(record.clone());
            record
        }
    };
    bern::secrets::write(&secret_name(&id), password)?;
    if let Err(error) = store(&records) {
        let _ = bern::secrets::delete(&secret_name(&id));
        return Err(error);
    }
    Ok(record)
}

/// 忘掉一条：先删凭据，再删标签。凭据删不掉时保留标签，用户可以再试。
pub fn forget(id: &str) -> Result<(), CommandError> {
    bern::secrets::delete(&secret_name(id))?;
    let mut records = load();
    records.retain(|record| record.id != id);
    store(&records)
}

/// 某条保存项的密码。条目不存在返回 `None`；凭据库里没有值也返回 `None`
/// ——调用方要把它当成「这条保存项已经不可用」。
pub fn value(id: &str) -> Result<Option<String>, CommandError> {
    if !load().iter().any(|record| record.id == id) {
        return Ok(None);
    }
    bern::secrets::read(&secret_name(id))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_label_maps_to_the_same_id() {
        assert_eq!(id_of("下载站"), id_of("下载站"));
        assert_ne!(id_of("下载站"), id_of("备份"));
        assert!(id_of("下载站").chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn empty_labels_and_passwords_are_refused() {
        assert_eq!(save(" ", "x").unwrap_err().code, "password");
        assert_eq!(save("a", "").unwrap_err().code, "password");
    }

    #[test]
    fn a_record_survives_json() {
        let record = Record {
            id: "abc".into(),
            label: "下载站".into(),
            created: 7,
        };
        assert_eq!(Record::from_value(&record.to_value()), Some(record));
    }
}
