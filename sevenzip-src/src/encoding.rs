//! Decoding archive names that have no declared encoding.
//!
//! A ZIP name without the UTF-8 flag carries raw bytes. File names are short
//! and often too ambiguous for per-name detection, so the archive is detected
//! once from all of its legacy names. The candidate set is restricted to the
//! encodings actually found in legacy ZIPs: without it the detector answers
//! windows-1250 or ISO-8859-5 for short CJK names.
//!
//! Names that are valid UTF-8 are always decoded as UTF-8; only the remaining
//! bytes take part in detection.

use encoding_rs::{Encoding, BIG5, EUC_JP, EUC_KR, GB18030, SHIFT_JIS, WINDOWS_1252};

const CANDIDATES: [&str; 7] = [
    "gb18030", "big5", "cp932", "euc-kr", "euc-jp", "cp1252", "cp437",
];

/// Detection only needs a sample; huge archives should not concatenate every
/// name just to classify the archive.
const MAX_SAMPLE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameEncoding {
    Utf8,
    Legacy(&'static Encoding),
    Cp437,
}

impl NameEncoding {
    pub(crate) fn detect(names: &[&[u8]]) -> Self {
        let legacy: Vec<&[u8]> = names
            .iter()
            .copied()
            .filter(|name| std::str::from_utf8(name).is_err())
            .collect();
        if legacy.is_empty() {
            return Self::Utf8;
        }

        let mut blob = Vec::new();
        for name in &legacy {
            if blob.len() >= MAX_SAMPLE {
                break;
            }
            if !blob.is_empty() {
                blob.push(b'\n');
            }
            blob.extend_from_slice(name);
        }

        // chardetng is reliable for short Big5/EUC-JP names where chardet-rs
        // prefers GB18030; for short GBK it answers windows-1250, so only its
        // CJK answers are trusted here. chardet-rs then covers GBK and CP437.
        let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
        detector.feed(&blob, true);
        let guessed = detector.guess(None, chardetng::Utf8Detection::Deny);
        if let Some(encoding) = cjk_from_name(guessed.name()) {
            if encoding.decodes_all(&legacy) {
                return encoding;
            }
        }

        let options = chardet_rs::DetectOptions {
            include_encodings: Some(CANDIDATES.iter().map(|name| name.to_string()).collect()),
            ignore_threshold: true,
            ..Default::default()
        };
        let ranked = chardet_rs::detect_all(&blob, &options).unwrap_or_default();
        let mut best = None;
        for candidate in ranked {
            let Some(encoding) = candidate.encoding.as_deref().and_then(from_name) else {
                continue;
            };
            if encoding.decodes_all(&legacy) {
                best = Some(encoding);
                break;
            }
        }
        if let Some(best) = best {
            // CP437 and Windows-1252 share the same byte range and the
            // detector often ranks them equally. DOS names put accented
            // letters where 1252 has currency signs or punctuation, so let
            // readable Latin text break the tie.
            if best == Self::Legacy(WINDOWS_1252) && cp437_reads_better(&legacy) {
                return Self::Cp437;
            }
            return best;
        }
        if Self::Legacy(GB18030).decodes_all(&legacy) {
            Self::Legacy(GB18030)
        } else {
            Self::Cp437
        }
    }

    pub(crate) fn decode(self, raw: &[u8]) -> String {
        if let Ok(text) = std::str::from_utf8(raw) {
            return text.to_string();
        }
        match self {
            Self::Utf8 => String::from_utf8_lossy(raw).into_owned(),
            Self::Legacy(encoding) => encoding.decode(raw).0.into_owned(),
            Self::Cp437 => decode_cp437(raw),
        }
    }

    /// The Windows code page that reproduces this decoding inside the engine
    /// (`-mcp=`). `None` for UTF-8, which the engine already handles.
    #[cfg(windows)]
    pub(crate) fn code_page(self) -> Option<u32> {
        match self {
            Self::Utf8 => None,
            Self::Cp437 => Some(437),
            Self::Legacy(encoding) => match encoding.name() {
                "gb18030" => Some(936),
                "Big5" => Some(950),
                "Shift_JIS" => Some(932),
                "EUC-KR" => Some(949),
                // 51932 makes 7-Zip fail with a bogus out-of-memory error;
                // 20932 is the conversion table it actually supports.
                "EUC-JP" => Some(20932),
                "windows-1252" => Some(1252),
                _ => None,
            },
        }
    }

    fn decodes_all(self, names: &[&[u8]]) -> bool {
        match self {
            Self::Utf8 => names.iter().all(|name| std::str::from_utf8(name).is_ok()),
            Self::Legacy(encoding) => names.iter().all(|name| {
                encoding
                    .decode_without_bom_handling_and_without_replacement(name)
                    .is_some()
            }),
            Self::Cp437 => true,
        }
    }
}

fn cjk_from_name(name: &str) -> Option<NameEncoding> {
    match name {
        "GBK" | "gb18030" => Some(NameEncoding::Legacy(GB18030)),
        "Big5" => Some(NameEncoding::Legacy(BIG5)),
        "Shift_JIS" => Some(NameEncoding::Legacy(SHIFT_JIS)),
        "EUC-KR" => Some(NameEncoding::Legacy(EUC_KR)),
        "EUC-JP" => Some(NameEncoding::Legacy(EUC_JP)),
        _ => None,
    }
}

fn from_name(name: &str) -> Option<NameEncoding> {
    match name.to_ascii_lowercase().as_str() {
        "gb18030" | "gbk" => Some(NameEncoding::Legacy(GB18030)),
        "big5" => Some(NameEncoding::Legacy(BIG5)),
        "cp932" | "shift_jis" | "shift-jis" => Some(NameEncoding::Legacy(SHIFT_JIS)),
        "euc-kr" | "cp949" => Some(NameEncoding::Legacy(EUC_KR)),
        "euc-jp" => Some(NameEncoding::Legacy(EUC_JP)),
        "windows-1252" | "cp1252" => Some(NameEncoding::Legacy(WINDOWS_1252)),
        "cp437" => Some(NameEncoding::Cp437),
        _ => None,
    }
}

fn cp437_reads_better(names: &[&[u8]]) -> bool {
    let mut cp437_score = 0;
    let mut windows_score = 0;
    for name in names {
        cp437_score += latin_score(&decode_cp437(name));
        windows_score += latin_score(&WINDOWS_1252.decode(name).0);
    }
    cp437_score > windows_score
}

/// Scores how much a decoding looks like human-readable Latin text. Letters
/// inside words score best; controls and punctuation splitting a word score
/// worst.
fn latin_score(text: &str) -> i32 {
    let chars: Vec<char> = text.chars().collect();
    let mut score = 0;
    for (index, &c) in chars.iter().enumerate() {
        if c.is_ascii() {
            continue;
        }
        let after_letter = index > 0 && chars[index - 1].is_ascii_alphabetic();
        let before_letter = chars
            .get(index + 1)
            .is_some_and(|next| next.is_ascii_alphabetic());
        let in_word = after_letter || before_letter;
        if c.is_control() || c == '\u{FFFD}' {
            score -= 8;
        } else if is_alien_letter(c) {
            score -= 4;
        } else if c.is_alphabetic() {
            score += if in_word { 3 } else { 1 };
        } else if after_letter && before_letter {
            score -= 3;
        }
    }
    score
}

fn is_alien_letter(c: char) -> bool {
    matches!(c, '\u{0370}'..='\u{03FF}' | '\u{0400}'..='\u{04FF}')
}

fn decode_cp437(raw: &[u8]) -> String {
    raw.iter()
        .map(|&byte| {
            if byte < 0x80 {
                byte as char
            } else {
                const CP437: [char; 128] = [
                    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}', '\u{00E4}', '\u{00E0}',
                    '\u{00E5}', '\u{00E7}', '\u{00EA}', '\u{00EB}', '\u{00E8}', '\u{00EF}',
                    '\u{00EE}', '\u{00EC}', '\u{00C4}', '\u{00C5}', '\u{00C9}', '\u{00E6}',
                    '\u{00C6}', '\u{00F4}', '\u{00F6}', '\u{00F2}', '\u{00FB}', '\u{00F9}',
                    '\u{00FF}', '\u{00D6}', '\u{00DC}', '\u{00A2}', '\u{00A3}', '\u{00A5}',
                    '\u{20A7}', '\u{0192}', '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}',
                    '\u{00F1}', '\u{00D1}', '\u{00AA}', '\u{00BA}', '\u{00BF}', '\u{2310}',
                    '\u{00AC}', '\u{00BD}', '\u{00BC}', '\u{00A1}', '\u{00AB}', '\u{00BB}',
                    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}',
                    '\u{2562}', '\u{2556}', '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}',
                    '\u{255D}', '\u{255C}', '\u{255B}', '\u{2510}', '\u{2514}', '\u{2534}',
                    '\u{252C}', '\u{251C}', '\u{2500}', '\u{253C}', '\u{255E}', '\u{255F}',
                    '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}',
                    '\u{256C}', '\u{2567}', '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}',
                    '\u{2558}', '\u{2552}', '\u{2553}', '\u{256B}', '\u{256A}', '\u{2518}',
                    '\u{250C}', '\u{2588}', '\u{2584}', '\u{258C}', '\u{2590}', '\u{2580}',
                    '\u{03B1}', '\u{00DF}', '\u{0393}', '\u{03C0}', '\u{03A3}', '\u{03C3}',
                    '\u{00B5}', '\u{03C4}', '\u{03A6}', '\u{0398}', '\u{03A9}', '\u{03B4}',
                    '\u{221E}', '\u{03C6}', '\u{03B5}', '\u{2229}', '\u{2261}', '\u{00B1}',
                    '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00F7}', '\u{2248}',
                    '\u{00B0}', '\u{2219}', '\u{00B7}', '\u{221A}', '\u{207F}', '\u{00B2}',
                    '\u{25A0}', '\u{00A0}',
                ];
                CP437[(byte - 0x80) as usize]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(names: &[Vec<u8>]) -> NameEncoding {
        let refs: Vec<&[u8]> = names.iter().map(Vec::as_slice).collect();
        NameEncoding::detect(&refs)
    }

    fn encoded(encoding: &'static Encoding, text: &str) -> Vec<u8> {
        let (bytes, _, had_errors) = encoding.encode(text);
        assert!(!had_errors, "{text:?} cannot be encoded as {}", encoding.name());
        bytes.into_owned()
    }

    fn assert_legacy(encoding: NameEncoding, expected: &'static Encoding) {
        match encoding {
            NameEncoding::Legacy(actual) => assert_eq!(actual.name(), expected.name()),
            other => panic!("expected {}, got {other:?}", expected.name()),
        }
    }

    #[test]
    fn ascii_and_utf8_names_stay_utf8() {
        let names = vec![
            "readme.txt".as_bytes().to_vec(),
            "亮剑/第1集.xml".as_bytes().to_vec(),
        ];
        assert_eq!(detect(&names), NameEncoding::Utf8);
    }

    #[test]
    fn detects_gbk_across_the_whole_archive() {
        let names = vec![
            encoded(encoding_rs::GBK, "亮剑"),
            encoded(encoding_rs::GBK, "亮剑/第1集_6699405806422458894.xml"),
        ];
        let detected = detect(&names);
        assert_legacy(detected, GB18030);
        assert_eq!(
            detected.decode(&names[1]),
            "亮剑/第1集_6699405806422458894.xml"
        );
    }

    #[test]
    fn detects_big5_even_though_each_name_is_short() {
        let names = vec![
            encoded(BIG5, "編碼測試"),
            encoded(BIG5, "測試檔案.txt"),
            encoded(BIG5, "動畫"),
        ];
        let detected = detect(&names);
        assert_legacy(detected, BIG5);
        assert_eq!(detected.decode(&names[1]), "測試檔案.txt");
    }

    #[test]
    fn detects_shift_jis() {
        let names = vec![
            encoded(SHIFT_JIS, "日本語"),
            encoded(SHIFT_JIS, "テスト.txt"),
            encoded(SHIFT_JIS, "第1話"),
        ];
        let detected = detect(&names);
        assert_legacy(detected, SHIFT_JIS);
        assert_eq!(detected.decode(&names[2]), "第1話");
    }

    #[test]
    fn detects_euc_kr() {
        let names = vec![
            encoded(EUC_KR, "한글"),
            encoded(EUC_KR, "테스트.txt"),
            encoded(EUC_KR, "애니메이션"),
        ];
        let detected = detect(&names);
        assert_legacy(detected, EUC_KR);
        assert_eq!(detected.decode(&names[2]), "애니메이션");
    }

    #[test]
    fn detects_euc_jp() {
        let names = vec![
            encoded(EUC_JP, "日本語"),
            encoded(EUC_JP, "テスト.txt"),
            encoded(EUC_JP, "第1話"),
        ];
        let detected = detect(&names);
        assert_legacy(detected, EUC_JP);
        assert_eq!(detected.decode(&names[2]), "第1話");
    }

    #[test]
    fn detects_windows_1252() {
        let names = vec![
            encoded(WINDOWS_1252, "Café"),
            encoded(WINDOWS_1252, "brûlée.txt"),
            encoded(WINDOWS_1252, "Größe"),
        ];
        let detected = detect(&names);
        assert_legacy(detected, WINDOWS_1252);
        assert_eq!(detected.decode(&names[1]), "brûlée.txt");
    }

    #[test]
    fn recognises_cp437() {
        // "Café/Ñandú.txt" in CP437: 0x82 é, 0xA5 Ñ, 0xA3 ú.
        let names = vec![
            b"Caf\x82".to_vec(),
            b"Caf\x82/\xA5and\xA3.txt".to_vec(),
        ];
        let detected = detect(&names);
        assert_eq!(detected, NameEncoding::Cp437);
        assert_eq!(detected.decode(&names[1]), "Café/Ñandú.txt");
    }

    #[test]
    fn detects_single_short_names_where_one_detector_alone_fails() {
        // chardetng calls this GBK name windows-1250; chardet-rs gets it.
        let gbk = vec![encoded(encoding_rs::GBK, "亮剑")];
        assert_legacy(detect(&gbk), GB18030);
        // chardet-rs calls this Big5 name GB18030; chardetng gets it.
        let big5 = vec![encoded(BIG5, "編碼測試")];
        assert_legacy(detect(&big5), BIG5);
    }

    #[test]
    fn mixed_utf8_and_legacy_names_keep_their_own_decoding() {
        let names = vec![
            "亮剑/第1集.xml".as_bytes().to_vec(),
            encoded(encoding_rs::GBK, "亮剑/第2集.xml"),
        ];
        let detected = detect(&names);
        assert_legacy(detected, GB18030);
        assert_eq!(detected.decode(&names[0]), "亮剑/第1集.xml");
        assert_eq!(detected.decode(&names[1]), "亮剑/第2集.xml");
    }
}
