//! 归档浏览层：把 `7z l -slt` 的平铺路径变成「当前这一层」的文件夹/文件，
//! 并把界面用的 id、大小、加密标记算好。
//!
//! 归档里没有真正的目录项也常见（有些 zip 只写 `sub/a.txt`），所以一层里的
//! 文件夹是**推导**出来的：路径里还有 `/` 就说明它是文件夹。所有路径在这里
//! 统一成 `/` 分隔、不带前导斜杠；带 `..` 的条目直接丢掉，界面永远走不出
//! 归档根（7-Zip 自己也会剥掉 `..`，这里只是不把它展示出来）。

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::backend::ListedEntry;

/// 归档里的一行（当前层）。`path` 是归档内的完整路径，引擎用它解压；
/// 界面只拿 `id`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub folder: bool,
    pub size: u64,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Name,
    Size,
}

/// 「sub\a.txt」→「sub/a.txt」；去掉 `./`、前导与结尾的斜杠。带 `..` 或
/// 盘符的路径返回 `None`：它们不该出现在浏览树里。
pub fn normalize(path: &str) -> Option<String> {
    let unified = path.replace('\\', "/");
    if unified.starts_with('/') || unified.contains(':') {
        return None;
    }
    let mut parts = Vec::new();
    for part in unified.split('/') {
        match part {
            "" | "." => continue,
            ".." => return None,
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// 归档里是否有加密条目。7z 的头部加密在列目录时就会失败（没有条目可看），
/// 引擎会把那次失败也当作「加密」。
pub fn is_encrypted(entries: &[ListedEntry]) -> bool {
    entries.iter().any(|entry| entry.encrypted)
}

/// 错误消息是不是「缺密码 / 密码不对」。
pub fn password_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("password")
        || message.contains("密码")
        || lower.contains("encrypted")
        || lower.contains("break signaled")
        || lower.contains("headers error")
}

/// 当前层的直接子项。`prefix` 形如 `""` 或 `"sub/"`。
///
/// 文件夹的 `size` 是后代文件的总和，只用于排序/说明；界面显示「—」。
pub fn layer(entries: &[ListedEntry], prefix: &str) -> Vec<Item> {
    let mut map: BTreeMap<String, Item> = BTreeMap::new();
    for entry in entries {
        let Some(path) = normalize(&entry.path) else {
            continue;
        };
        let Some(rest) = path.strip_prefix(prefix) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }
        let (name, deeper) = match rest.split_once('/') {
            Some((name, _)) => (name, true),
            None => (rest, false),
        };
        if name.is_empty() {
            continue;
        }
        let full = format!("{prefix}{name}");
        let item = map.entry(full.clone()).or_insert_with(|| Item {
            id: crate::locations::id_of(&full),
            path: full,
            name: name.to_string(),
            folder: false,
            size: 0,
            encrypted: false,
        });
        if deeper || entry.folder {
            item.folder = true;
        }
        if !entry.folder {
            item.size = item.size.saturating_add(entry.size);
        }
        if entry.encrypted {
            item.encrypted = true;
        }
    }
    map.into_values().collect()
}

/// 目录优先，再按字段。和文件列表同一套约定：降序也不把文件夹甩到后面。
pub fn sort(items: &mut [Item], by: Sort, descending: bool) {
    items.sort_by(|a, b| {
        let folders = b.folder.cmp(&a.folder);
        if folders != Ordering::Equal {
            return folders;
        }
        let order = match by {
            Sort::Name => crate::browser::natural(&a.name, &b.name),
            Sort::Size => a.size.cmp(&b.size),
        };
        if descending { order.reverse() } else { order }
    });
}

/// 能当文本预览的文件名。只认常见后缀，避免把二进制交给解码器；
/// 没有后缀的 README / LICENSE 这类也放进来。
pub fn is_text(name: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        "txt", "text", "md", "markdown", "rst", "log", "csv", "tsv", "json", "jsonl", "xml",
        "html", "htm", "xhtml", "svg", "css", "js", "mjs", "cjs", "ts", "tsx", "jsx", "yml",
        "yaml", "toml", "ini", "cfg", "conf", "properties", "sh", "bash", "zsh", "bat", "cmd",
        "ps1", "sql", "rs", "c", "h", "cc", "cpp", "hpp", "py", "java", "kt", "kts", "gradle",
        "srt", "vtt", "tex", "diff", "patch",
    ];
    const NAMES: &[&str] = &[
        "readme",
        "license",
        "changelog",
        "notice",
        "makefile",
        "dockerfile",
        ".gitignore",
        ".gitattributes",
        ".editorconfig",
    ];
    let lower = name.to_ascii_lowercase();
    if NAMES.contains(&lower.as_str()) {
        return true;
    }
    match lower.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => EXTENSIONS.contains(&extension),
        _ => false,
    }
}

/// 能当图片预览的文件名。只收双端解码器都稳的常见格式：Android 用
/// BitmapFactory，Windows 用 WIC；TIFF/HEIC/AVIF 这类一边缺解码器的不进。
pub fn is_image(name: &str) -> bool {
    const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "jfif", "gif", "webp", "bmp", "apng"];
    let lower = name.to_ascii_lowercase();
    match lower.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => EXTENSIONS.contains(&extension),
        _ => false,
    }
}

/// 从归档路径取最后一段，给「智能解压」的目录名用。
pub fn stem_of(name: &str) -> String {
    let stem = std::path::Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or(name);
    crate::backend::file_component(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64, folder: bool) -> ListedEntry {
        ListedEntry {
            path: path.into(),
            size,
            folder,
            encrypted: false,
        }
    }

    #[test]
    fn paths_become_forward_slashed_and_relative() {
        assert_eq!(normalize("sub\\a.txt").as_deref(), Some("sub/a.txt"));
        assert_eq!(normalize("./sub//a.txt/").as_deref(), Some("sub/a.txt"));
        assert_eq!(normalize("../evil"), None);
        assert_eq!(normalize("/etc/passwd"), None);
        assert_eq!(normalize("C:\\a"), None);
    }

    #[test]
    fn a_layer_is_derived_even_without_directory_entries() {
        let entries = vec![
            entry("readme.txt", 6, false),
            entry("sub/a.txt", 10, false),
            entry("sub/deep/b.txt", 20, false),
        ];
        let items = layer(&entries, "");
        assert_eq!(items.len(), 2);
        let sub = items.iter().find(|item| item.name == "sub").unwrap();
        assert!(sub.folder);
        assert_eq!(sub.size, 30);
        let readme = items.iter().find(|item| item.name == "readme.txt").unwrap();
        assert!(!readme.folder);
        assert_eq!(readme.size, 6);

        let inside = layer(&entries, "sub/");
        let names: Vec<&str> = inside.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "deep"]);
    }

    #[test]
    fn folders_come_first_in_both_directions() {
        let mut items = vec![
            Item {
                id: 1,
                path: "a".into(),
                name: "a".into(),
                folder: false,
                size: 9,
                encrypted: false,
            },
            Item {
                id: 2,
                path: "d".into(),
                name: "d".into(),
                folder: true,
                size: 0,
                encrypted: false,
            },
        ];
        sort(&mut items, Sort::Size, true);
        assert_eq!(items[0].name, "d");
    }

    #[test]
    fn only_known_text_names_are_previewable() {
        assert!(is_text("readme.txt"));
        assert!(is_text("config.XML"));
        assert!(is_text("README"));
        assert!(is_text(".gitignore"));
        assert!(!is_text("archive.tar.gz"));
        assert!(!is_text("photo.jpg"));
        assert!(!is_text("setup.exe"));
        assert!(!is_text("no-extension"));
        assert!(!is_text(".txt"));
    }

    #[test]
    fn pictures_are_recognised_by_their_common_extensions() {
        assert!(is_image("photo.jpg"));
        assert!(is_image("IMG_0001.JPEG"));
        assert!(is_image("logo.png"));
        assert!(is_image("anim.webp"));
        assert!(!is_image("photo.heic"));
        assert!(!is_image("notes.txt"));
        assert!(!is_image("no-extension"));
        assert!(!is_image(".png"));
    }

    #[test]
    fn a_password_prompt_reads_as_a_password_problem() {
        assert!(password_error("Cannot open encrypted archive. Wrong password?"));
        assert!(password_error("需要密码：这个归档已加密"));
        assert!(password_error("Headers Error"));
        assert!(!password_error("Cannot open the file as [7z] archive"));
    }
}
