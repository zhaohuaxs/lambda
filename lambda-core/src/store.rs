//! JSON state files with two guarantees: a write never leaves a half file
//! (temp + fsync + rename), and a file that no longer parses is moved aside
//! instead of being silently overwritten by the next save.

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

pub fn read_json(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(_) => {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or(0);
            let backup = path.with_extension(format!("corrupt-{stamp}"));
            let _ = fs::rename(path, &backup);
            None
        }
    }
}

pub fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    crate::backend::write_atomic(path, text.as_bytes())
}
