//! 浏览层：把一个目录的 `files::list` 结果变成界面能直接吃的一行。
//!
//! 三件事都在这里做完，UI 不参与：
//!
//! - **排序**：目录优先，再按所选字段；大小与时间未提供的排在最后。
//! - **格式化**：大小（B/KB/MB/GB）、相对时间（「3 天前」）、类型文案。
//!   相对时间是为了绕开时区：`SystemTime` 是 UTC，而 `std` 拿不到本地偏移。
//! - **URL 与 id 的映射**：URL 很长，只留在引擎这边；界面拿 `id`。

use std::time::SystemTime;

use bern::files;

/// 目录里的一项。`url` 不进 state，只在引擎里用来进目录 / 读文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub folder: bool,
    pub archive: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Name,
    Size,
    Time,
}

/// 能被 Lambda 打开浏览的归档。RAR 只读（`DOC/unRarLicense.txt`），
/// 但仍然要认出来——M4 的归档浏览器就是靠这个标记进门的。
const ARCHIVE_EXTENSIONS: &[&str] = &[
    "7z", "zip", "rar", "tar", "gz", "tgz", "xz", "bz2", "tbz2", "zst", "cab", "iso", "jar", "apk",
];

/// 一个目录的内容。`Err` 原样带着 Bern 的错误码（`not_a_folder` 等），
/// 界面据此区分「授权失效」与「这不是目录」。
pub fn list(url: &str) -> Result<Vec<Item>, bern::CommandError> {
    let entries = files::list(url)?;
    Ok(entries
        .into_iter()
        .map(|entry| Item {
            id: super::locations::id_of(&entry.url),
            archive: !entry.folder && is_archive(&entry.name),
            name: entry.name,
            url: entry.url,
            folder: entry.folder,
            size: entry.size,
            modified: entry.modified,
        })
        .collect())
}

/// 目录优先，然后按字段。升序时按自然顺序，降序时整列反转（但不把目录
/// 甩到后面——文件管理器里目录永远在上面）。
pub fn sort(items: &mut [Item], by: Sort, descending: bool) {
    items.sort_by(|a, b| {
        let folders = b.folder.cmp(&a.folder);
        if folders != std::cmp::Ordering::Equal {
            return folders;
        }
        let order = match by {
            Sort::Name => natural(&a.name, &b.name),
            Sort::Size => a.size.cmp(&b.size),
            Sort::Time => a.modified.cmp(&b.modified),
        };
        if descending { order.reverse() } else { order }
    });
}

/// 名称比较：`file2` 排在 `file10` 前面，中文按码位。数字段按数值比，其余
/// 不区分大小写——手机上一个目录里常见 `IMG_2` 与 `IMG_10`。
pub(crate) fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut left_number = String::new();
                while let Some(c) = left.peek().copied().filter(char::is_ascii_digit) {
                    left_number.push(c);
                    left.next();
                }
                let mut right_number = String::new();
                while let Some(c) = right.peek().copied().filter(char::is_ascii_digit) {
                    right_number.push(c);
                    right.next();
                }
                // 去掉前导零再按长度、然后按字典序比：`007` == `7`，`10` > `9`。
                let left_trimmed = left_number.trim_start_matches('0');
                let right_trimmed = right_number.trim_start_matches('0');
                let order = left_trimmed
                    .len()
                    .cmp(&right_trimmed.len())
                    .then_with(|| left_trimmed.cmp(right_trimmed));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                left.next();
                right.next();
            }
        }
    }
}

pub fn is_archive(name: &str) -> bool {
    let lower = name.to_lowercase();
    // `tar.gz` / `tar.bz2` 这类双扩展名：取最后一段就够了，`gz` 已在表里。
    match lower.rsplit_once('.') {
        Some((_, extension)) => ARCHIVE_EXTENSIONS.contains(&extension),
        None => false,
    }
}

/// 「1.2 MB」。1024 进制，保留一位小数；1 KB 以下按字节。
pub fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 「3 天前」。`None` 给「—」——SAF 的某些 provider 不提供修改时间。
pub fn modified_text(when: Option<SystemTime>) -> String {
    let Some(when) = when else {
        return "—".into();
    };
    let Ok(elapsed) = SystemTime::now().duration_since(when) else {
        // 未来的时间（设备时间被改过）：不要说「-3 天前」。
        return "刚刚".into();
    };
    let seconds = elapsed.as_secs();
    match seconds {
        0..=59 => "刚刚".into(),
        60..=3599 => format!("{} 分钟前", seconds / 60),
        3600..=86_399 => format!("{} 小时前", seconds / 3600),
        86_400..=2_591_999 => format!("{} 天前", seconds / 86_400),
        2_592_000..=31_535_999 => format!("{} 个月前", seconds / 2_592_000),
        _ => format!("{} 年前", seconds / 31_536_000),
    }
}

pub fn kind_text(item: &Item) -> &'static str {
    if item.folder {
        "文件夹"
    } else if item.archive {
        "压缩包"
    } else {
        "文件"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn item(name: &str, folder: bool, size: u64) -> Item {
        Item {
            id: super::super::locations::id_of(name),
            name: name.into(),
            url: format!("content://x/{name}"),
            folder,
            archive: is_archive(name),
            size,
            modified: Some(UNIX_EPOCH + Duration::from_secs(size)),
        }
    }

    #[test]
    fn folders_come_first_in_both_directions() {
        let mut items = vec![
            item("a.txt", false, 3),
            item("dir", true, 0),
            item("b.txt", false, 9),
        ];
        sort(&mut items, Sort::Size, false);
        assert_eq!(items[0].name, "dir");
        assert_eq!(items[1].name, "a.txt");
        sort(&mut items, Sort::Size, true);
        assert_eq!(
            items[0].name, "dir",
            "a descending size still lists folders first"
        );
        assert_eq!(items[1].name, "b.txt");
    }

    #[test]
    fn names_with_numbers_sort_naturally() {
        let mut items = vec![
            item("IMG_10.jpg", false, 10),
            item("IMG_2.jpg", false, 2),
            item("img_1.jpg", false, 1),
        ];
        sort(&mut items, Sort::Name, false);
        let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["img_1.jpg", "IMG_2.jpg", "IMG_10.jpg"]);
    }

    #[test]
    fn archive_names_are_recognised_whatever_the_case() {
        assert!(is_archive("a.7z"));
        assert!(is_archive("A.ZIP"));
        assert!(is_archive("backup.tar.gz"));
        assert!(!is_archive("notes.txt"));
        assert!(!is_archive("no-extension"));
        assert!(!is_archive("7z"));
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(1023), "1023 B");
        assert_eq!(size_text(1024), "1.0 KB");
        assert_eq!(size_text(1536), "1.5 KB");
        assert_eq!(size_text(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(size_text(1024 * 1024 * 200), "200 MB");
    }

    #[test]
    fn relative_time_covers_every_bucket() {
        let now = SystemTime::now();
        assert_eq!(modified_text(None), "—");
        assert_eq!(modified_text(Some(now)), "刚刚");
        assert_eq!(
            modified_text(Some(now - Duration::from_secs(90))),
            "1 分钟前"
        );
        assert_eq!(
            modified_text(Some(now - Duration::from_secs(7200))),
            "2 小时前"
        );
        assert_eq!(
            modified_text(Some(now - Duration::from_secs(86_400 * 3))),
            "3 天前"
        );
        assert_eq!(
            modified_text(Some(now - Duration::from_secs(2_592_000 * 2))),
            "2 个月前"
        );
        assert_eq!(
            modified_text(Some(now - Duration::from_secs(31_536_000 * 3))),
            "3 年前"
        );
    }

    #[test]
    fn a_future_timestamp_is_not_negative() {
        let future = SystemTime::now() + Duration::from_secs(600);
        assert_eq!(modified_text(Some(future)), "刚刚");
    }
}
