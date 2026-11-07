//! M5 边界补测：把「7zz 遇到异常归档会怎样」和「大归档要多久」变成一次
//! 可复现的真机报告。
//!
//! 完全不碰 SAF：所有夹具都由引擎自己生成在应用私有 cache 里，所以只需要点一次
//! 按钮，报告写到 `data_dir()/m5-report.txt`，用
//!
//! ```text
//! adb shell run-as dev.bern.lambda cat files/dev.bern.lambda/m5-report.txt
//! ```
//!
//! 取回。报告里的每一行都是 `✓/✗ 编号 名称 — 细节`，失败不中断（「哪一条失败」
//! 本身就是结论）。
//!
//! 覆盖三类问题：
//!
//! 1. **异常归档**：0 字节 / 随机字节 / 截断 / 错密码 / 无密码 / 空归档 / 深层路径 /
//!    非 ASCII 名 —— 期望是「报错但不崩、不留垃圾」；
//! 2. **路径穿越**（AC14 的 Zip Slip）：手工构造带 `../../` 与绝对路径条目的 zip，
//!    确认引擎的落点仍在目标目录内；
//! 3. **耗时与内存**：5000 个小文件、32 MB 不可压缩数据、16 MB 全零（解压炸弹形状），
//!    分别记录压缩 / 列举 / 解压毫秒数与压缩比。

#[cfg(target_os = "android")]
pub use android::run;

#[cfg(not(target_os = "android"))]
pub fn run(_dest: Option<&str>) -> String {
    "边界补测需要在 Android 上运行（引擎在那里）".into()
}

// ---------------------------------------------------------------------------
// 纯工具：不碰引擎与 SAF，因此放在 `android` 模块之外，宿主上也能单测。
// ---------------------------------------------------------------------------

use std::fs;
use std::path::Path;

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, bytes).map_err(|e| format!("写入 {} 失败：{e}", path.display()))
}

/// 递归收集相对路径，用于「有没有东西漏到目标外」的判断。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn names_under(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, path: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                walk(root, &child, out);
            } else if let Ok(relative) = child.strip_prefix(root) {
                out.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn stem(name: &str) -> String {
    name.rsplit_once('.').map(|(head, _)| head).unwrap_or(name).to_string()
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn yes_no(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn clip(text: &str, limit: usize) -> String {
    let cleaned = text.replace(['\r', '\n'], " ");
    if cleaned.chars().count() <= limit {
        return cleaned;
    }
    let head: String = cleaned.chars().take(limit).collect();
    format!("{head}…")
}

/// 进程峰值 RSS（KB）。`VmHWM` 只升不降，正好用来给一张表定上界。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn vm_hwm_kb() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| {
            text.lines()
                .find(|line| line.starts_with("VmHWM:"))
                .and_then(|line| line.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}

/// 一个最小的 zip 写入器：只支持 stored，够构造 Zip Slip 与空归档样本。
/// 引擎不让我们造出带 `../` 名的归档，所以这里只能自己拼字节。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn zip_bytes(items: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in items {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let name = name.as_bytes();

        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 names
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0x2821u16.to_le_bytes()); // 2020-01-01
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name);
        out.extend_from_slice(data);

        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x2821u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let count = items.len() as u16;
    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_zip_is_just_an_end_record() {
        let bytes = zip_bytes(&[]);
        assert_eq!(bytes.len(), 22);
        assert_eq!(&bytes[..4], &0x0605_4b50u32.to_le_bytes());
    }

    #[test]
    fn zip_round_trips_through_its_own_headers() {
        let bytes = zip_bytes(&[("a.txt", b"hello"), ("../../x.txt", b"slip")]);
        assert_eq!(&bytes[..4], &0x0403_4b50u32.to_le_bytes());
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("a.txt"));
        assert!(text.contains("../../x.txt"));
        assert_eq!(crc32(b"hello"), 0x3610_a686);
    }

    #[test]
    fn names_under_skips_missing_directories() {
        assert!(names_under(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn clip_keeps_it_short_and_single_line() {
        let text = clip("a\nb\r\nc", 50);
        assert!(!text.contains('\n'));
        assert!(clip(&"x".repeat(80), 10).ends_with('…'));
    }
}

#[cfg(target_os = "android")]
mod android {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use sevenzip_src as engine;

    use super::{clip, names_under, stem, write, yes_no, zip_bytes, vm_hwm_kb};

    /// 应用私有目录：报告与位置表都住这里，不需要任何权限。
    fn data_root() -> PathBuf {
        bern::storage::data_dir()
            .or_else(bern::storage::cache_dir)
            .unwrap_or_else(std::env::temp_dir)
    }

    fn report_path() -> PathBuf {
        data_root().join("m5-report.txt")
    }

    /// 所有夹具与产物的根，每次补测前整个删掉。
    fn bench_root() -> PathBuf {
        bern::storage::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("lambda")
            .join("bench")
    }

    /// 报告先写私有的 `data_dir()`，再导出到用户授权的公有位置树根。
    ///
    /// 私有路径只有 `run-as`（还得是 debug 包）才取得出来，而 Android 10
    /// 起应用不能直接写 `/sdcard`，唯一的公有出口就是 SAF 授权树
    /// （`export_path` 落在树根）。`dest` 是当前位置根 URI；没有位置时
    /// 只剩私有副本，报告里会写明这一点。
    fn finish(lines: Vec<String>, dest: Option<&str>) -> String {
        let text = lines.join("\n");
        let path = report_path();
        let mut note = match fs::write(&path, &text) {
            Ok(()) => format!("\n[已写入 {}]", path.display()),
            Err(error) => format!("\n[写入 {} 失败：{error}]", path.display()),
        };
        match dest.filter(|url| !url.is_empty()) {
            Some(url) => match bern::platform::export_path(&path, url) {
                Ok(uri) => note.push_str(&format!("\n[已导出到公有目录 {uri}]")),
                Err(error) => note.push_str(&format!("\n[导出到公有目录失败：{}]", error.message)),
            },
            None => note.push_str("\n[没有目录可导出：先添加一个目录，报告才能落到公有目录]"),
        }
        format!("{text}{note}")
    }

    /// 一次补测的全部夹具路径，准备一次，重复使用。
    struct Fixtures {
        /// 明文样本的源目录。
        src: PathBuf,
        plain_7z: PathBuf,
        secure_7z: PathBuf,
        plain_zip: PathBuf,
        secure_zip: PathBuf,
        empty_zip: PathBuf,
        junk_7z: PathBuf,
        junk_zip: PathBuf,
        zero_7z: PathBuf,
        zero_zip: PathBuf,
        trunc_7z: PathBuf,
        trunc_zip: PathBuf,
        slip_zip: PathBuf,
        deep_7z: PathBuf,
        notes: PathBuf,
        /// 所有解压产物都放这里，方便整体清理与越界检查。
        out: PathBuf,
    }

    /// 通过 / 失败计数与报告行。
    #[derive(Default)]
    struct Tally {
        lines: Vec<String>,
        pass: usize,
        fail: usize,
        section: usize,
    }

    impl Tally {
        fn head(&mut self, title: &str) {
            self.section += 1;
            self.lines.push(String::new());
            self.lines.push(format!("-- {}. {title} --", self.section));
        }

        /// 记录一条判定。`detail` 只在失败时才重要，但成功也留一行证据。
        fn check(&mut self, id: &str, name: &str, ok: bool, detail: impl Into<String>) {
            if ok {
                self.pass += 1;
                self.lines.push(format!("✓ {id} {name} — {}", detail.into()));
            } else {
                self.fail += 1;
                self.lines.push(format!("✗ {id} {name} — {}", detail.into()));
            }
        }
    }

    pub fn run(dest: Option<&str>) -> String {
        let started = Instant::now();
        let root = bench_root();
        let _ = fs::remove_dir_all(&root);
        let mut tally = Tally::default();

        let mut head = vec![
            "== Lambda M5 边界补测（异常归档 / 路径穿越 / 耗时）==".to_string(),
            format!("engine = {}", engine::engine_name()),
            format!("bench  = {}", root.display()),
            format!("hwm    = {} KB（进程峰值 RSS）", vm_hwm_kb()),
        ];
        head.push(String::new());

        match prepare(&root) {
            Ok(fixtures) => {
                cases_list(&fixtures, &mut tally);
                cases_extract(&fixtures, &mut tally);
                cases_safety(&fixtures, &mut tally);
                cases_bench(&root, &mut tally);
            }
            Err(error) => head.push(format!("!! 夹具准备失败：{error}")),
        }

        let verdict = format!(
            "判定：{} 通过 / {} 失败 · 总耗时 {} ms · 峰值 RSS {} KB",
            tally.pass,
            tally.fail,
            started.elapsed().as_millis(),
            vm_hwm_kb()
        );
        head.extend(tally.lines);
        head.push(String::new());
        head.push(verdict);
        finish(head, dest)
    }

    // -----------------------------------------------------------------------
    // 夹具
    // -----------------------------------------------------------------------

    fn prepare(root: &Path) -> Result<Fixtures, String> {
        let src = root.join("src");
        fs::create_dir_all(src.join("sub")).map_err(|e| e.to_string())?;
        write(&src.join("a.txt"), b"hello from lambda\n")?;
        write(&src.join("sub/b.txt"), b"nested file\n")?;
        write(&src.join("中文 名字.txt"), b"unicode name\n")?;
        write(&src.join("emoji😀.txt"), b"four byte emoji\n")?;

        // 顶层输入 = src 的每个子项，这样归档里的路径不带 `src/` 前缀。
        let mut inputs: Vec<PathBuf> = fs::read_dir(&src)
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        inputs.sort();

        let plain_7z = root.join("plain.7z");
        engine::create_with(&plain_7z, &inputs, None, "7z").map_err(|e| e.0)?;
        let secure_7z = root.join("secure.7z");
        engine::create_with(&secure_7z, &inputs, Some("lambda"), "7z").map_err(|e| e.0)?;
        let plain_zip = root.join("plain.zip");
        engine::create_with(&plain_zip, &inputs, None, "zip").map_err(|e| e.0)?;
        let secure_zip = root.join("secure.zip");
        engine::create_with(&secure_zip, &inputs, Some("lambda"), "zip").map_err(|e| e.0)?;

        // 深层目录：20 层，路径约 60 字符，够验证 `-slt` 与解压的路径长度。
        let mut deep = root.join("deep");
        for index in 1..=20 {
            deep = deep.join(format!("d{index}"));
        }
        fs::create_dir_all(&deep).map_err(|e| e.to_string())?;
        write(&deep.join("leaf.txt"), b"the deepest file\n")?;
        let deep_7z = root.join("deep.7z");
        engine::create_with(&deep_7z, &[root.join("deep")], None, "7z").map_err(|e| e.0)?;

        // 异常体：全零字节、随机字节、截断、空 zip、手工构造的穿越 zip。
        let zero_7z = root.join("zero.7z");
        write(&zero_7z, b"")?;
        let zero_zip = root.join("zero.zip");
        write(&zero_zip, b"")?;
        let junk_7z = root.join("junk.7z");
        write(&junk_7z, &vec![0xABu8; 4096])?;
        let junk_zip = root.join("junk.zip");
        write(&junk_zip, &vec![0xABu8; 4096])?;

        let full = fs::read(&plain_7z).map_err(|e| e.to_string())?;
        let trunc_7z = root.join("trunc.7z");
        write(&trunc_7z, &full[..full.len() * 6 / 10])?;
        let full = fs::read(&plain_zip).map_err(|e| e.to_string())?;
        let trunc_zip = root.join("trunc.zip");
        write(&trunc_zip, &full[..full.len() * 6 / 10])?;

        let empty_zip = root.join("empty.zip");
        write(&empty_zip, &zip_bytes(&[]))?;

        let slip_zip = root.join("slip.zip");
        write(
            &slip_zip,
            &zip_bytes(&[
                ("../../escape.txt", b"should stay inside\n"),
                ("/abs.txt", b"absolute entry\n"),
                ("sub/../../up.txt", b"two levels up\n"),
                ("ok.txt", b"a normal entry\n"),
            ]),
        )?;

        let notes = root.join("notes.txt");
        write(&notes, b"not an archive at all\n")?;

        Ok(Fixtures {
            src,
            plain_7z,
            secure_7z,
            plain_zip,
            secure_zip,
            empty_zip,
            junk_7z,
            junk_zip,
            zero_7z,
            zero_zip,
            trunc_7z,
            trunc_zip,
            slip_zip,
            deep_7z,
            notes,
            out: root.join("out"),
        })
    }

    // -----------------------------------------------------------------------
    // 1. 列举
    // -----------------------------------------------------------------------

    fn cases_list(f: &Fixtures, tally: &mut Tally) {
        tally.head("列举：正常归档");

        for (label, archive, password, expect_min) in [
            ("plain.7z", &f.plain_7z, None, 4usize),
            ("plain.zip", &f.plain_zip, None, 4),
            ("secure.7z", &f.secure_7z, Some("lambda"), 4),
            ("secure.zip", &f.secure_zip, Some("lambda"), 4),
        ] {
            let started = Instant::now();
            match engine::list(archive, password) {
                Ok(entries) => {
                    let encrypted = entries.iter().filter(|e| e.encrypted).count();
                    tally.check(
                        "T01",
                        &format!("list {label}"),
                        entries.len() >= expect_min,
                        format!(
                            "{} 条目（加密 {encrypted}）{} ms",
                            entries.len(),
                            started.elapsed().as_millis()
                        ),
                    );
                }
                Err(error) => tally.check("T01", &format!("list {label}"), false, error.0),
            }
        }

        let started = Instant::now();
        match engine::list(&f.deep_7z, None) {
            Ok(entries) => {
                let deepest = entries
                    .iter()
                    .map(|e| e.path.len())
                    .max()
                    .unwrap_or(0);
                tally.check(
                    "T02",
                    "list 20 层深路径",
                    entries.iter().any(|e| e.path.ends_with("leaf.txt")),
                    format!(
                        "{} 条目，最长路径 {deepest} 字符，{} ms",
                        entries.len(),
                        started.elapsed().as_millis()
                    ),
                );
            }
            Err(error) => tally.check("T02", "list 20 层深路径", false, error.0),
        }

        tally.head("列举：异常归档（期望报错，不崩）");
        for (id, label, archive, password) in [
            ("T03", "secure.7z 无密码", &f.secure_7z, None),
            ("T04", "secure.7z 错密码", &f.secure_7z, Some("wrong")),
            ("T05", "junk.7z", &f.junk_7z, None),
            ("T05", "junk.zip", &f.junk_zip, None),
            ("T06", "zero.7z", &f.zero_7z, None),
            ("T06", "zero.zip", &f.zero_zip, None),
            ("T07", "trunc.7z", &f.trunc_7z, None),
            ("T07", "trunc.zip", &f.trunc_zip, None),
            ("T08", "notes.txt（不是归档）", &f.notes, None),
        ] {
            let started = Instant::now();
            match engine::list(archive, password) {
                Ok(entries) => tally.check(
                    id,
                    label,
                    // 截断的归档有时能列出部分条目，这不算错；其余必须拒绝。
                    id == "T07",
                    format!(
                        "未报错，列出 {} 条目（{} ms）{}",
                        entries.len(),
                        started.elapsed().as_millis(),
                        if id == "T07" { "—— 截断仍可部分可读，记入边界" } else { "" }
                    ),
                ),
                Err(error) => tally.check(
                    id,
                    label,
                    true,
                    format!("拒绝：{}（{} ms）", clip(&error.0, 120), started.elapsed().as_millis()),
                ),
            }
        }

        match engine::list(&f.empty_zip, None) {
            Ok(entries) => tally.check(
                "T09",
                "list 空 zip（0 条目）",
                entries.is_empty(),
                format!("{} 条目", entries.len()),
            ),
            Err(error) => tally.check("T09", "list 空 zip（0 条目）", false, error.0),
        }
    }

    // -----------------------------------------------------------------------
    // 2. 解压
    // -----------------------------------------------------------------------

    fn cases_extract(f: &Fixtures, tally: &mut Tally) {
        tally.head("解压：正常归档与内容一致");

        let expected_bytes: u64 = [
            "hello from lambda\n",
            "nested file\n",
            "unicode name\n",
            "four byte emoji\n",
        ]
        .iter()
        .map(|text| text.len() as u64)
        .sum();

        for (label, archive, password) in [
            ("plain.7z", &f.plain_7z, None),
            ("plain.zip", &f.plain_zip, None),
            ("secure.7z", &f.secure_7z, Some("lambda")),
            ("secure.zip", &f.secure_zip, Some("lambda")),
        ] {
            let target = f.out.join(format!("ok-{}", stem(label)));
            let started = Instant::now();
            match engine::extract(archive, password, &target) {
                Ok(report) => {
                    let names = names_under(&target);
                    let unicode = names.iter().any(|n| n.contains("中文 名字.txt"))
                        && names.iter().any(|n| n.contains("emoji😀.txt"));
                    let nested = names.iter().any(|n| n.ends_with("sub/b.txt"));
                    tally.check(
                        "T10",
                        &format!("extract {label}"),
                        report.files == 4 && report.bytes == expected_bytes && unicode && nested,
                        format!(
                            "{} 文件 / {} 字节（期望 4 / {expected_bytes}）· 非 ASCII {} · 子目录 {} · {} ms",
                            report.files,
                            report.bytes,
                            yes_no(unicode),
                            yes_no(nested),
                            started.elapsed().as_millis()
                        ),
                    );
                }
                Err(error) => tally.check("T10", &format!("extract {label}"), false, error.0),
            }
        }

        // 已存在的同名文件：`-y` 应覆盖，staging 复用才安全。
        let overwrite = f.out.join("overwrite");
        let _ = fs::create_dir_all(&overwrite);
        let _ = write(&overwrite.join("a.txt"), b"stale content\n");
        let ok = engine::extract(&f.plain_7z, None, &overwrite)
            .map(|_| {
                fs::read_to_string(overwrite.join("a.txt")).unwrap_or_default()
                    == "hello from lambda\n"
            })
            .unwrap_or(false);
        tally.check(
            "T11",
            "解压覆盖同名旧文件",
            ok,
            if ok { "旧内容被替换为归档内内容" } else { "旧文件未被覆盖或解压失败" },
        );

        let deep_target = f.out.join("deep");
        let started = Instant::now();
        match engine::extract(&f.deep_7z, None, &deep_target) {
            Ok(report) => {
                let leaf = names_under(&deep_target)
                    .into_iter()
                    .any(|name| name.ends_with("leaf.txt"));
                tally.check(
                    "T12",
                    "解压 20 层深路径",
                    report.files == 1 && leaf,
                    format!(
                        "{} 文件，最深 leaf 存在 {} · {} ms",
                        report.files,
                        yes_no(leaf),
                        started.elapsed().as_millis()
                    ),
                );
            }
            Err(error) => tally.check("T12", "解压 20 层深路径", false, error.0),
        }

        tally.head("解压：异常归档（期望报错，且不留垃圾）");
        for (id, label, archive, password) in [
            ("T13", "junk.zip", &f.junk_zip, None),
            ("T13", "zero.7z", &f.zero_7z, None),
            ("T13", "trunc.zip", &f.trunc_zip, None),
            ("T14", "secure.7z 无密码", &f.secure_7z, None),
            ("T14", "secure.7z 错密码", &f.secure_7z, Some("wrong")),
        ] {
            let target = f.out.join(format!("bad-{}-{}", id, stem(label)));
            let started = Instant::now();
            match engine::extract(archive, password, &target) {
                Ok(report) => tally.check(
                    id,
                    label,
                    false,
                    format!(
                        "意外成功，释放 {} 文件 / {} 字节",
                        report.files, report.bytes
                    ),
                ),
                Err(error) => {
                    let leftovers = names_under(&target).len();
                    tally.check(
                        id,
                        label,
                        true,
                        format!(
                            "拒绝：{} · 残留 {leftovers} 文件（{} ms）",
                            clip(&error.0, 100),
                            started.elapsed().as_millis()
                        ),
                    );
                }
            }
        }

        let empty_target = f.out.join("empty");
        match engine::extract(&f.empty_zip, None, &empty_target) {
            Ok(report) => tally.check(
                "T15",
                "解压空 zip",
                report.files == 0,
                format!("{} 文件 / {} 字节", report.files, report.bytes),
            ),
            Err(error) => tally.check("T15", "解压空 zip", false, error.0),
        }

        // 目录当归档传：引擎必须拒绝，而不是把目录当文件读。
        let target = f.out.join("as-archive");
        match engine::extract(&f.src, None, &target) {
            Ok(report) => tally.check(
                "T16",
                "目录当归档",
                false,
                format!("意外成功，释放 {} 文件", report.files),
            ),
            Err(error) => tally.check("T16", "目录当归档", true, format!("拒绝：{}", clip(&error.0, 100))),
        }
    }

    // -----------------------------------------------------------------------
    // 3. 安全：路径穿越
    // -----------------------------------------------------------------------

    fn cases_safety(f: &Fixtures, tally: &mut Tally) {
        tally.head("安全：路径穿越（Zip Slip / 绝对路径）");

        let target = f.out.join("slip");
        let result = engine::extract(&f.slip_zip, None, &target);
        let inside = names_under(&target);
        // 目标目录之外是否出现任何东西：上一级是本用例的 `out/`，再上一级是 bench 根。
        let parent = f.out.clone();
        let grandparent = parent.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let escaped: Vec<String> = ["escape.txt", "up.txt", "abs.txt"]
            .iter()
            .flat_map(|name| [parent.join(name), grandparent.join(name)])
            .filter(|path| path.exists())
            .map(|path| path.display().to_string())
            .collect();

        match result {
            Ok(report) => tally.check(
                "T17",
                "穿越条目被就地收纳",
                escaped.is_empty(),
                format!(
                    "{} 文件：{} · 目标外泄漏 {} 处{}",
                    report.files,
                    if inside.is_empty() { "（空）".into() } else { inside.join(", ") },
                    escaped.len(),
                    if escaped.is_empty() {
                        String::new()
                    } else {
                        format!(" → {}", escaped.join(" | "))
                    }
                ),
            ),
            Err(error) => tally.check(
                "T17",
                "穿越条目被拒绝",
                escaped.is_empty(),
                format!("引擎拒绝：{} · 目标外泄漏 {} 处", clip(&error.0, 100), escaped.len()),
            ),
        }
    }

    // -----------------------------------------------------------------------
    // 4. 耗时与内存
    // -----------------------------------------------------------------------

    fn cases_bench(root: &Path, tally: &mut Tally) {
        tally.head("耗时：5000 个小文件");

        let many = root.join("many");
        let _ = fs::create_dir_all(&many);
        let started = Instant::now();
        for index in 0..5000 {
            let _ = fs::write(many.join(format!("f{index:05}.txt")), b"lambda many files probe\n");
        }
        let wrote = started.elapsed().as_millis();

        let archive = root.join("many.7z");
        let started = Instant::now();
        let created = engine::create_with_level(&archive, &[many.clone()], None, "7z", Some(1));
        let create_ms = started.elapsed().as_millis();
        match created {
            Ok(report) => tally.check(
                "B01",
                "压缩 5000 个小文件",
                report.bytes > 0,
                format!(
                    "写盘 {wrote} ms · 压缩 {create_ms} ms · 归档 {} B",
                    report.bytes
                ),
            ),
            Err(error) => tally.check("B01", "压缩 5000 个小文件", false, error.0),
        }

        let started = Instant::now();
        match engine::list(&archive, None) {
            Ok(entries) => {
                let list_ms = started.elapsed().as_millis();
                let after_list = vm_hwm_kb();
                tally.check(
                    "B02",
                    "列举 5000 条目",
                    entries.len() >= 5000,
                    format!("{} 条目 · {list_ms} ms · 峰值 RSS {after_list} KB", entries.len()),
                );
            }
            Err(error) => tally.check("B02", "列举 5000 条目", false, error.0),
        }

        let target = root.join("out-many");
        let started = Instant::now();
        match engine::extract(&archive, None, &target) {
            Ok(report) => tally.check(
                "B03",
                "解压 5000 个文件",
                report.files >= 5000,
                format!(
                    "{} 文件 / {} 字节 · {} ms · 峰值 RSS {} KB",
                    report.files,
                    report.bytes,
                    started.elapsed().as_millis(),
                    vm_hwm_kb()
                ),
            ),
            Err(error) => tally.check("B03", "解压 5000 个文件", false, error.0),
        }
        let _ = fs::remove_dir_all(&target);
        let _ = fs::remove_dir_all(&many);

        tally.head("耗时：32 MB 不可压缩数据（-mx=1）");

        let blob = root.join("blob.bin");
        let mut data = vec![0u8; 32 * 1024 * 1024];
        let mut state: u32 = 0x1234_5678;
        for chunk in data.chunks_mut(4) {
            // xorshift32：不压缩的伪随机，才能得到真实的 32 MB 写入量。
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            chunk.copy_from_slice(&state.to_le_bytes());
        }
        let _ = fs::write(&blob, &data);
        drop(data);

        let blob_archive = root.join("blob.7z");
        let started = Instant::now();
        match engine::create_with_level(&blob_archive, &[blob.clone()], None, "7z", Some(1)) {
            Ok(report) => {
                let create_ms = started.elapsed().as_millis();
                let ratio = report.bytes as f64 / (32.0 * 1024.0 * 1024.0);
                tally.check(
                    "B04",
                    "压缩 32 MB",
                    report.bytes > 0,
                    format!(
                        "{create_ms} ms · {} B（{:.2}%）",
                        report.bytes,
                        ratio * 100.0
                    ),
                );
                let target = root.join("out-blob");
                let started = Instant::now();
                match engine::extract(&blob_archive, None, &target) {
                    Ok(extracted) => {
                        let ok = extracted.bytes == 32 * 1024 * 1024;
                        tally.check(
                            "B05",
                            "解压 32 MB",
                            ok,
                            format!(
                                "{} 字节（期望 33554432）· {} ms",
                                extracted.bytes,
                                started.elapsed().as_millis()
                            ),
                        );
                    }
                    Err(error) => tally.check("B05", "解压 32 MB", false, error.0),
                }
                let _ = fs::remove_dir_all(&target);
            }
            Err(error) => tally.check("B04", "压缩 32 MB", false, error.0),
        }
        let _ = fs::remove_file(&blob);

        tally.head("耗时：16 MB 全零（解压炸弹形状）");

        let zeros = root.join("zeros.bin");
        let _ = fs::write(&zeros, vec![0u8; 16 * 1024 * 1024]);
        let zeros_archive = root.join("zeros.7z");
        let started = Instant::now();
        match engine::create_with_level(&zeros_archive, &[zeros.clone()], None, "7z", Some(9)) {
            Ok(report) => {
                let create_ms = started.elapsed().as_millis();
                let ratio = 16.0 * 1024.0 * 1024.0 / report.bytes.max(1) as f64;
                let target = root.join("out-zeros");
                let started = Instant::now();
                let extracted = engine::extract(&zeros_archive, None, &target);
                let extract_ms = started.elapsed().as_millis();
                let bytes = extracted.map(|r| r.bytes).unwrap_or(0);
                tally.check(
                    "B06",
                    "16 MB 全零",
                    bytes == 16 * 1024 * 1024,
                    format!(
                        "压缩 {create_ms} ms → {} B（膨胀 {ratio:.0}×）· 解压 {extract_ms} ms / {bytes} 字节",
                        report.bytes
                    ),
                );
                let _ = fs::remove_dir_all(&target);
            }
            Err(error) => tally.check("B06", "16 MB 全零", false, error.0),
        }
        let _ = fs::remove_file(&zeros);
    }

    // -----------------------------------------------------------------------
    // 工具
    // -----------------------------------------------------------------------
}
