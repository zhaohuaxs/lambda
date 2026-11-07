//! Streaming text preview.
//!
//! The worker pulls one archive entry through `7zz x -so`, decodes it
//! incrementally and publishes pages into a small ring. The UI is the
//! consumer: when it has read ahead far enough the worker blocks, so memory
//! stays bounded no matter how large the entry is.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use bern::wake::Waker;
use encoding_rs::{
    BIG5, EUC_JP, EUC_KR, Encoding, GB18030, SHIFT_JIS, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252,
};

use crate::backend;

pub const PAGE_CHARS: usize = 12_000;
const SAMPLE_BYTES: usize = 64 * 1024;
const AHEAD: usize = 2;
const KEEP_BACK: usize = 16;

#[derive(Debug, Clone)]
pub struct Page {
    /// 1-based page number; stays stable after older pages are dropped.
    pub number: i64,
    pub text: String,
}

#[derive(Debug)]
pub struct Data {
    pub pages: VecDeque<Page>,
    /// Index into `pages`, not the page number.
    pub current: usize,
    /// The UI asked for the next page before the worker produced it.
    pub want_next: bool,
    pub next_page: i64,
    pub done: bool,
    pub error: Option<String>,
    pub loaded_bytes: u64,
    pub encoding: String,
    pub closed: bool,
}

type Shared = (Mutex<Data>, Condvar);

pub struct Session {
    pub title: String,
    pub total_bytes: u64,
    shared: Arc<Shared>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

/// What the engine copies into protocol state; one page at a time.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub text: String,
    pub page: i64,
    pub pages: i64,
    pub has_prev: bool,
    pub has_next: bool,
    pub loading: bool,
    pub done: bool,
    pub error: Option<String>,
    pub loaded_bytes: u64,
    pub encoding: String,
}

impl Session {
    /// Stops the worker and waits for it to release the engine mutex before
    /// the caller deletes the staging copy. Without the join the worker could
    /// still be inside 7-Zip, and Windows then refuses to remove the folder.
    pub fn stop(&mut self) {
        {
            let (lock, ready) = &*self.shared;
            let mut data = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            data.closed = true;
            ready.notify_all();
        }
        self.cancel.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let (lock, _) = &*self.shared;
        let data = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let page = data.pages.get(data.current);
        Snapshot {
            text: page.map(|page| page.text.clone()).unwrap_or_default(),
            page: page.map(|page| page.number).unwrap_or(0),
            pages: data.next_page - 1,
            has_prev: data.current > 0,
            has_next: !data.closed
                && data.error.is_none()
                && (data.current + 1 < data.pages.len() || !data.done),
            loading: !data.done && data.error.is_none(),
            done: data.done,
            error: data.error.clone(),
            loaded_bytes: data.loaded_bytes,
            encoding: data.encoding.clone(),
        }
    }

    /// `true` when the cursor moved (or is now waiting for the next page).
    pub fn turn(&self, delta: i64) -> bool {
        if delta == 0 {
            return false;
        }
        let (lock, ready) = &*self.shared;
        let mut data = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if delta > 0 {
            let target = data.current as i64 + delta;
            if target >= 0 && (target as usize) < data.pages.len() {
                data.current = target as usize;
                data.want_next = false;
            } else if !data.done && data.error.is_none() {
                // 页环边界：先记住「要下一页」，worker 推入时自动前进。
                data.want_next = true;
            } else {
                return false;
            }
        } else {
            let target = data.current as i64 + delta;
            if target < 0 {
                return false;
            }
            data.current = target as usize;
            data.want_next = false;
        }
        while data.current > KEEP_BACK {
            data.pages.pop_front();
            data.current -= 1;
        }
        ready.notify_all();
        true
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start(
    archive: PathBuf,
    mut password: Option<String>,
    entry: String,
    title: String,
    total_bytes: u64,
    wake: Waker,
) -> Session {
    let shared: Arc<Shared> = Arc::new((
        Mutex::new(Data {
            pages: VecDeque::new(),
            current: 0,
            want_next: false,
            next_page: 1,
            done: false,
            error: None,
            loaded_bytes: 0,
            encoding: String::new(),
            closed: false,
        }),
        Condvar::new(),
    ));
    let worker_shared = shared.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let handle = std::thread::spawn(move || {
        let mut decoder = StreamDecoder::new(worker_shared, Some(wake));
        match backend::stream_entry(
            &archive,
            password.as_deref(),
            &entry,
            Some(&worker_cancel),
            |chunk| decoder.feed(chunk),
        ) {
            Ok(()) => decoder.finish(),
            Err(error) => decoder.fail(error),
        }
        if let Some(password) = password.as_mut() {
            use zeroize::Zeroize;
            password.zeroize();
        }
    });
    Session {
        title,
        total_bytes,
        shared,
        cancel,
        handle: Some(handle),
    }
}

struct StreamDecoder {
    shared: Arc<Shared>,
    wake: Option<Waker>,
    sample: Vec<u8>,
    decoder: Option<encoding_rs::Decoder>,
    encoding: &'static Encoding,
    encoding_label: String,
    pending: String,
    loaded: u64,
    produced: bool,
}

impl StreamDecoder {
    fn new(shared: Arc<Shared>, wake: Option<Waker>) -> Self {
        Self {
            shared,
            wake,
            sample: Vec::new(),
            decoder: None,
            encoding: UTF_8,
            encoding_label: "UTF-8".into(),
            pending: String::new(),
            loaded: 0,
            produced: false,
        }
    }

    fn feed(&mut self, chunk: &[u8]) -> bool {
        self.loaded += chunk.len() as u64;
        if self.decoder.is_none() {
            self.sample.extend_from_slice(chunk);
            if self.sample.len() < SAMPLE_BYTES {
                return !closed(&self.shared);
            }
            self.begin();
        } else {
            self.decode(chunk, false);
        }
        !closed(&self.shared)
    }

    fn begin(&mut self) {
        let (encoding, bom) = detect(&self.sample);
        self.encoding = encoding;
        self.encoding_label = label(encoding).into();
        self.decoder = Some(encoding.new_decoder_without_bom_handling());
        let sample = std::mem::take(&mut self.sample);
        self.decode(&sample[bom..], false);
    }

    fn decode(&mut self, bytes: &[u8], last: bool) {
        if self.decoder.is_none() {
            return;
        }
        let mut offset = 0;
        // `decode_to_string` appends only into the `String`'s spare capacity;
        // it does not grow it. Start with room for the worst case and retry
        // with more when the decoder reports a full output buffer.
        let mut capacity = bytes.len().saturating_mul(3).saturating_add(64);
        loop {
            let mut text = String::with_capacity(capacity);
            let (result, read, _) = self
                .decoder
                .as_mut()
                .expect("decoder is present")
                .decode_to_string(&bytes[offset..], &mut text, last);
            self.pending.push_str(&text);
            self.drain();
            offset += read;
            match result {
                encoding_rs::CoderResult::InputEmpty => break,
                encoding_rs::CoderResult::OutputFull => {
                    if read == 0 {
                        capacity = capacity.saturating_mul(2);
                    }
                }
            }
        }
    }

    fn drain(&mut self) {
        while let Some(page) = take_page(&mut self.pending) {
            if !self.push(page) {
                return;
            }
        }
    }

    fn finish(&mut self) {
        if self.decoder.is_none() {
            self.begin();
        }
        if closed(&self.shared) {
            return;
        }
        self.decode(&[], true);
        while let Some(page) = take_page(&mut self.pending) {
            if !self.push(page) {
                return;
            }
        }
        if !self.pending.is_empty() || !self.produced {
            let text = std::mem::take(&mut self.pending);
            if !self.push(text) {
                return;
            }
        }
        let (lock, _) = &*self.shared;
        let mut data = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if data.closed {
            return;
        }
        data.encoding = self.encoding_label.clone();
        data.loaded_bytes = self.loaded;
        data.done = true;
        data.want_next = false;
        drop(data);
        self.notify();
    }

    fn fail(&mut self, error: String) {
        let (lock, _) = &*self.shared;
        let mut data = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if data.closed {
            return;
        }
        data.error = Some(error);
        data.loaded_bytes = self.loaded;
        data.encoding = self.encoding_label.clone();
        data.done = true;
        data.want_next = false;
        drop(data);
        self.notify();
    }

    fn notify(&self) {
        if let Some(wake) = &self.wake {
            wake.wake();
        }
    }

    fn push(&mut self, text: String) -> bool {
        let (lock, ready) = &*self.shared;
        let mut data = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if data.closed {
            return false;
        }
        let page = Page {
            number: data.next_page,
            text: sanitize(&text),
        };
        data.next_page += 1;
        data.pages.push_back(page);
        if data.want_next && data.current + 1 < data.pages.len() {
            data.current += 1;
            data.want_next = false;
        }
        data.encoding = self.encoding_label.clone();
        data.loaded_bytes = self.loaded;
        self.produced = true;
        drop(data);
        self.notify();

        let mut data = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        while !data.closed && data.pages.len() > data.current + AHEAD + 1 {
            data = ready
                .wait(data)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        !data.closed
    }
}

fn closed(shared: &Shared) -> bool {
    let (lock, _) = shared;
    lock.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .closed
}

fn take_page(pending: &mut String) -> Option<String> {
    if pending.chars().count() < PAGE_CHARS {
        return None;
    }
    let mut end = pending
        .char_indices()
        .nth(PAGE_CHARS)
        .map(|(index, _)| index)
        .unwrap_or(pending.len());
    // Prefer a line break in the second half so a page does not end in the
    // middle of a line when a nearby newline exists.
    if let Some(newline) = pending[..end].rfind('\n')
        && newline >= end / 2
    {
        end = newline + 1;
    }
    Some(pending.drain(..end).collect())
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\t') {
                c
            } else if c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect()
}

fn detect(sample: &[u8]) -> (&'static Encoding, usize) {
    if sample.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (UTF_8, 3);
    }
    if sample.starts_with(&[0xFF, 0xFE]) {
        return (UTF_16LE, 2);
    }
    if sample.starts_with(&[0xFE, 0xFF]) {
        return (UTF_16BE, 2);
    }
    if utf8_complete(sample) {
        return (UTF_8, 0);
    }
    let options = chardet_rs::DetectOptions {
        include_encodings: Some(
            ["gb18030", "big5", "cp932", "euc-kr", "euc-jp", "cp1252"]
                .iter()
                .map(|name| name.to_string())
                .collect(),
        ),
        ignore_threshold: true,
        ..Default::default()
    };
    if let Ok(ranked) = chardet_rs::detect_all(sample, &options) {
        for candidate in ranked {
            if let Some(encoding) = candidate.encoding.as_deref().and_then(from_name) {
                return (encoding, 0);
            }
        }
    }
    if GB18030
        .decode_without_bom_handling_and_without_replacement(sample)
        .is_some()
    {
        (GB18030, 0)
    } else {
        (WINDOWS_1252, 0)
    }
}

fn utf8_complete(bytes: &[u8]) -> bool {
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        // A sample cut through the last character is still valid UTF-8.
        Err(error) => error.error_len().is_none(),
    }
}

fn from_name(name: &str) -> Option<&'static Encoding> {
    match name.to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" => Some(UTF_8),
        "gb18030" | "gbk" => Some(GB18030),
        "big5" => Some(BIG5),
        "cp932" | "shift_jis" | "shift-jis" => Some(SHIFT_JIS),
        "euc-kr" | "cp949" => Some(EUC_KR),
        "euc-jp" => Some(EUC_JP),
        "windows-1252" | "cp1252" => Some(WINDOWS_1252),
        _ => None,
    }
}

fn label(encoding: &'static Encoding) -> &'static str {
    match encoding.name() {
        "UTF-8" => "UTF-8",
        "gb18030" | "GB18030" | "GBK" => "GB18030",
        "Big5" => "Big5",
        "Shift_JIS" => "Shift_JIS",
        "EUC-KR" => "EUC-KR",
        "EUC-JP" => "EUC-JP",
        "windows-1252" => "Windows-1252",
        name => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(pages: &[&str]) -> Session {
        let mut queue = VecDeque::new();
        for (index, text) in pages.iter().enumerate() {
            queue.push_back(Page {
                number: index as i64 + 1,
                text: (*text).into(),
            });
        }
        Session {
            title: "a.txt".into(),
            total_bytes: 10,
            shared: Arc::new((
                Mutex::new(Data {
                    pages: queue,
                    current: 0,
                    want_next: false,
                    next_page: pages.len() as i64 + 1,
                    done: true,
                    error: None,
                    loaded_bytes: 10,
                    encoding: "UTF-8".into(),
                    closed: false,
                }),
                Condvar::new(),
            )),
            cancel: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    #[test]
    fn turns_move_between_loaded_pages_and_stop_at_the_edges() {
        let session = session(&["one", "two"]);
        assert_eq!(session.snapshot().page, 1);
        assert!(session.snapshot().has_next);
        assert!(!session.snapshot().has_prev);
        assert!(session.turn(1));
        assert_eq!(session.snapshot().text, "two");
        assert!(session.snapshot().has_prev);
        assert!(!session.turn(1));
        assert!(session.turn(-1));
        assert_eq!(session.snapshot().page, 1);
        assert!(!session.turn(-1));
    }

    #[test]
    fn next_before_the_worker_produces_it_waits_instead_of_failing() {
        let session = session(&["one"]);
        {
            let (lock, _) = &*session.shared;
            let mut data = lock.lock().unwrap();
            data.done = false;
            data.next_page = 2;
        }
        assert!(session.turn(1));
        let snapshot = session.snapshot();
        assert_eq!(snapshot.page, 1);
        assert!(snapshot.loading);
        assert!(snapshot.has_next);
        {
            let (lock, ready) = &*session.shared;
            let mut data = lock.lock().unwrap();
            data.pages.push_back(Page {
                number: 2,
                text: "two".into(),
            });
            data.next_page = 3;
            if data.want_next && data.current + 1 < data.pages.len() {
                data.current += 1;
                data.want_next = false;
            }
            ready.notify_all();
        }
        assert_eq!(session.snapshot().page, 2);
        assert_eq!(session.snapshot().text, "two");
    }

    #[test]
    fn a_small_file_is_decoded_when_the_stream_ends() {
        let shared: Arc<Shared> = Arc::new((
            Mutex::new(Data {
                pages: VecDeque::new(),
                current: 0,
                want_next: false,
                next_page: 1,
                done: false,
                error: None,
                loaded_bytes: 0,
                encoding: String::new(),
                closed: false,
            }),
            Condvar::new(),
        ));
        let mut decoder = StreamDecoder::new(shared.clone(), None);
        let text = "Lambda 文本预览\n第二行：UTF-8。\n";
        assert!(decoder.feed(text.as_bytes()));
        decoder.finish();
        let (lock, _) = &*shared;
        let data = lock.lock().unwrap();
        assert!(data.done);
        assert_eq!(data.pages.len(), 1);
        assert_eq!(data.pages[0].text, text);
        assert_eq!(data.encoding, "UTF-8");
    }

    #[test]
    fn pages_split_on_character_boundaries() {
        let text = "测".repeat(PAGE_CHARS * 2 + 7);
        let mut pending = text.clone();
        let mut pages = Vec::new();
        while let Some(page) = take_page(&mut pending) {
            assert!(page.chars().count() <= PAGE_CHARS);
            pages.push(page);
        }
        pages.push(pending);
        assert_eq!(pages.concat(), text);
    }

    #[test]
    fn pages_prefer_a_nearby_newline() {
        let text = format!("{}\nnext line", "a".repeat(PAGE_CHARS - 10));
        let mut pending = text.clone();
        let page = take_page(&mut pending).expect("one full page");
        assert!(page.ends_with('\n'));
        assert!(page.chars().count() <= PAGE_CHARS);
    }

    #[test]
    fn a_sample_cut_through_a_utf8_character_is_still_utf8() {
        let mut bytes = "测试".as_bytes().to_vec();
        bytes.pop();
        assert!(utf8_complete(&bytes));
        assert_eq!(detect(&bytes).0.name(), "UTF-8");
    }

    #[test]
    fn detects_gbk_text() {
        let text = "测试文件".repeat(100);
        let (bytes, _, had_errors) = GB18030.encode(&text);
        assert!(!had_errors);
        let (encoding, bom) = detect(&bytes);
        assert_eq!(label(encoding), "GB18030");
        assert_eq!(bom, 0);
    }

    #[test]
    fn detects_utf16_le_bom() {
        let (bytes, _, had_errors) = UTF_16LE.encode("测试");
        assert!(!had_errors);
        let mut sample = vec![0xFF, 0xFE];
        sample.extend_from_slice(&bytes);
        let (encoding, bom) = detect(&sample);
        assert_eq!(encoding.name(), "UTF-16LE");
        assert_eq!(bom, 2);
    }

    #[test]
    fn control_characters_do_not_reach_the_ui() {
        assert_eq!(sanitize("a\u{0}b\nc\t"), "a b\nc\t");
    }
}
