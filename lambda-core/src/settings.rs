//! 界面偏好：外观与强调色。写在应用私有目录，和任务记录分开。
//! 这里不存任何用户内容，更没有密码。

use std::fs;

use lambda_protocol::lambda::{Accent, Appearance};

use crate::locations;

fn path() -> std::path::PathBuf {
    locations::data_root().join("settings.json")
}

/// 上次选的外观与强调色；没有记录或读不懂时跟随系统、用棕色。
pub fn load() -> (Appearance, Accent) {
    let saved = fs::read_to_string(path())
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
    let appearance = match saved
        .as_ref()
        .and_then(|value| value.get("appearance"))
        .and_then(|value| value.as_str())
    {
        Some("Light") => Appearance::Light,
        Some("Dark") => Appearance::Dark,
        _ => Appearance::System,
    };
    let accent = match saved
        .as_ref()
        .and_then(|value| value.get("accent"))
        .and_then(|value| value.as_str())
    {
        Some("Blue") => Accent::Blue,
        Some("Red") => Accent::Red,
        _ => Accent::Brown,
    };
    (appearance, accent)
}

pub fn save(appearance: &Appearance, accent: &Accent) {
    let appearance = match appearance {
        Appearance::Light => "Light",
        Appearance::Dark => "Dark",
        Appearance::System => "System",
    };
    let accent = match accent {
        Accent::Brown => "Brown",
        Accent::Blue => "Blue",
        Accent::Red => "Red",
    };
    let _ = fs::create_dir_all(locations::data_root());
    let _ = fs::write(
        path(),
        serde_json::json!({ "appearance": appearance, "accent": accent }).to_string(),
    );
}
