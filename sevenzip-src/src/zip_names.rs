//! Raw ZIP name probe for the Windows engine.
//!
//! On Windows 7-Zip converts a name without the UTF-8 flag through the OEM
//! code page before Rust can see it, so the byte-level detection that works
//! on Android has no input there. The central directory still carries the
//! original bytes, so this probe reads them, runs the same `NameEncoding`
//! detection, and returns the `-mcp=` switch that reproduces the Android
//! result inside the engine.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::encoding::NameEncoding;

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD64_SIG: u32 = 0x0606_4b50;
const EOCD64_LOCATOR_SIG: u32 = 0x0706_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
/// EOCD plus the longest comment a ZIP can carry.
const MAX_EOCD_SEARCH: u64 = 22 + 65_535;
const MAX_SAMPLE: usize = 64 * 1024;

/// `-mcp=<code page>` for a ZIP whose legacy names need a non-OEM decoding,
/// or `None` when the archive is not a ZIP or needs no switch.
pub(crate) fn codepage_switch(archive: &Path) -> Option<String> {
    let names = legacy_names(archive)?;
    if names.is_empty() {
        return None;
    }
    let refs: Vec<&[u8]> = names.iter().map(Vec::as_slice).collect();
    NameEncoding::detect(&refs)
        .code_page()
        .map(|code_page| format!("-mcp={code_page}"))
}

fn legacy_names(path: &Path) -> Option<Vec<Vec<u8>>> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < 22 {
        return None;
    }

    let tail_len = len.min(MAX_EOCD_SEARCH) as usize;
    let mut tail = vec![0u8; tail_len];
    file.seek(SeekFrom::End(-(tail_len as i64))).ok()?;
    file.read_exact(&mut tail).ok()?;
    let eocd = find_last(&tail, EOCD_SIG)?;

    let mut entries = read_u16(&tail, eocd + 10)? as u64;
    let mut directory = read_u32(&tail, eocd + 16)? as u64;

    if entries == 0xFFFF || directory == 0xFFFF_FFFF {
        // ZIP64: the locator sits immediately before the EOCD.
        let locator = eocd.checked_sub(20)?;
        if read_u32(&tail, locator)? == EOCD64_LOCATOR_SIG {
            let eocd64_offset = read_u64(&tail, locator + 8)?;
            let mut record = [0u8; 56];
            file.seek(SeekFrom::Start(eocd64_offset)).ok()?;
            file.read_exact(&mut record).ok()?;
            if read_u32(&record, 0)? == EOCD64_SIG {
                entries = read_u64(&record, 32)?;
                directory = read_u64(&record, 48)?;
            }
        }
    }

    if directory >= len || entries == 0 {
        return None;
    }

    file.seek(SeekFrom::Start(directory)).ok()?;
    let mut names = Vec::new();
    let mut sampled = 0usize;
    for _ in 0..entries {
        let mut head = [0u8; 46];
        if file.read_exact(&mut head).is_err() {
            break;
        }
        if read_u32(&head, 0)? != CENTRAL_SIG {
            break;
        }
        let flags = read_u16(&head, 8)?;
        let name_len = read_u16(&head, 28)? as usize;
        let extra_len = read_u16(&head, 30)? as usize;
        let comment_len = read_u16(&head, 32)? as usize;
        let mut name = vec![0u8; name_len];
        if file.read_exact(&mut name).is_err() {
            break;
        }
        file.seek(SeekFrom::Current((extra_len + comment_len) as i64))
            .ok()?;
        if flags & 0x0800 == 0 {
            sampled += name_len + 1;
            if sampled > MAX_SAMPLE {
                break;
            }
            names.push(name);
        }
    }
    (!names.is_empty()).then_some(names)
}

fn find_last(haystack: &[u8], signature: u32) -> Option<usize> {
    let needle = signature.to_le_bytes();
    haystack.windows(4).rposition(|window| window == needle)
}

fn read_u16(buffer: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*buffer.get(at)?, *buffer.get(at + 1)?]))
}

fn read_u32(buffer: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *buffer.get(at)?,
        *buffer.get(at + 1)?,
        *buffer.get(at + 2)?,
        *buffer.get(at + 3)?,
    ]))
}

fn read_u64(buffer: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes([
        *buffer.get(at)?,
        *buffer.get(at + 1)?,
        *buffer.get(at + 2)?,
        *buffer.get(at + 3)?,
        *buffer.get(at + 4)?,
        *buffer.get(at + 5)?,
        *buffer.get(at + 6)?,
        *buffer.get(at + 7)?,
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the central directory + EOCD a real archive would carry; the
    /// probe never looks at local headers.
    fn zip_with_names(names: &[&[u8]]) -> Vec<u8> {
        let mut directory = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let mut head = [0u8; 46];
            head[0..4].copy_from_slice(&CENTRAL_SIG.to_le_bytes());
            head[4..6].copy_from_slice(&20u16.to_le_bytes());
            head[6..8].copy_from_slice(&20u16.to_le_bytes());
            // General-purpose flag stays 0: no UTF-8 declaration.
            head[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
            head[42..46].copy_from_slice(&(index as u32).to_le_bytes());
            directory.extend_from_slice(&head);
            directory.extend_from_slice(name);
        }
        let offset = 0u32;
        let size = directory.len() as u32;
        let mut eocd = [0u8; 22];
        eocd[0..4].copy_from_slice(&EOCD_SIG.to_le_bytes());
        eocd[8..10].copy_from_slice(&(names.len() as u16).to_le_bytes());
        eocd[10..12].copy_from_slice(&(names.len() as u16).to_le_bytes());
        eocd[12..16].copy_from_slice(&size.to_le_bytes());
        eocd[16..20].copy_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(&eocd);
        directory
    }

    fn switch_for(name: &str, bytes: &[u8]) -> Option<String> {
        let dir = std::env::temp_dir().join("lambda-zip-names-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let result = codepage_switch(&path);
        let _ = std::fs::remove_file(&path);
        result
    }

    #[test]
    fn gbk_names_ask_for_code_page_936() {
        // 中文.txt / 测试文件.txt in GBK, no UTF-8 flag.
        let names: [&[u8]; 2] = [
            &[0xD6, 0xD0, 0xCE, 0xC4, 0x2E, 0x74, 0x78, 0x74],
            &[
                0xB2, 0xE2, 0xCA, 0xD4, 0xCE, 0xC4, 0xBC, 0xFE, 0x2E, 0x74, 0x78, 0x74,
            ],
        ];
        let archive = zip_with_names(&names);
        assert_eq!(switch_for("gbk.zip", &archive).as_deref(), Some("-mcp=936"));
    }

    #[test]
    fn big5_names_ask_for_code_page_950() {
        // 測試檔案.txt / 報表.xlsx / 圖片資料夾 / 發票 in Big5. Several of
        // them are invalid EUC-JP, which is what makes the call unambiguous.
        let names: [&[u8]; 4] = [
            &[
                0xB4, 0xFA, 0xB8, 0xD5, 0xC0, 0xC9, 0xAE, 0xD7, 0x2E, 0x74, 0x78, 0x74,
            ],
            &[0xB3, 0xF8, 0xAA, 0xED, 0x2E, 0x78, 0x6C, 0x73, 0x78],
            &[0xB9, 0xCF, 0xA4, 0xF9, 0xB8, 0xEA, 0xAE, 0xC6, 0xA7, 0xA8],
            &[0xB5, 0x6F, 0xB2, 0xBC],
        ];
        let archive = zip_with_names(&names);
        assert_eq!(
            switch_for("big5.zip", &archive).as_deref(),
            Some("-mcp=950")
        );
    }

    #[test]
    fn plain_ascii_names_need_no_switch() {
        let names: [&[u8]; 1] = [b"readme.txt"];
        let archive = zip_with_names(&names);
        assert_eq!(switch_for("ascii.zip", &archive), None);
    }

    #[test]
    fn a_non_zip_is_left_alone() {
        assert_eq!(switch_for("not-a-zip.bin", b"this is not an archive"), None);
    }
}
