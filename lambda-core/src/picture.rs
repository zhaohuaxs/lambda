//! Archive picture preview.
//!
//! Bern 0.1.9 lets an engine serve a scheme to `Image(source:)`: the host
//! reads the stream on a thread of its own and decodes the bytes at the
//! size the picture is drawn. One preview token maps to one archive entry;
//! the stream runs `7z x -so`, so nothing is written to disk. The host needs
//! the whole file before decoding, so this module caps both the bytes and
//! the pixels rather than trusting the archive.

use std::collections::HashMap;
use std::io::{self, Cursor, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};

use bern::media::MediaSource;
use bern::wake::Waker;

use crate::backend;
use crate::browser;

/// The host holds the whole encoded picture, then a decoded copy at screen
/// size; 64 MB keeps both within a phone's budget.
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// A decompression bomb still has to be turned into sampling options; 24 MP
/// keeps a decoded 4-byte-per-pixel copy near 96 MB, which a phone can take.
pub const MAX_PIXELS: u64 = 24_000_000;

const SCHEME: &str = "lambda-picture";

#[derive(Clone)]
struct Status {
    loading: bool,
    detail: String,
    error: Option<String>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            loading: true,
            detail: "正在读取图片…".into(),
            error: None,
        }
    }
}

#[derive(Clone)]
struct Served {
    archive: PathBuf,
    password: Option<String>,
    entry: String,
    size: u64,
    status: Arc<Mutex<Status>>,
    closed: Arc<AtomicBool>,
    busy: Arc<Mutex<()>>,
    wake: Option<Waker>,
}

impl Drop for Served {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(password) = self.password.as_mut() {
            password.zeroize();
        }
    }
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub loading: bool,
    pub detail: String,
    pub error: Option<String>,
}

pub struct Session {
    token: u64,
    url: MediaSource,
    title: String,
    status: Arc<Mutex<Status>>,
    closed: Arc<AtomicBool>,
    busy: Arc<Mutex<()>>,
}

static SERVED: OnceLock<Mutex<HashMap<u64, Served>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
static TOKEN_SEED: OnceLock<u64> = OnceLock::new();
static REGISTER: Once = Once::new();

/// Not cryptographic, but not enumerable from 1 either: a per-process seed
/// mixed with a counter. The scheme is only reachable from the host's
/// in-process `Image` control.
fn next_token() -> u64 {
    let seed = *TOKEN_SEED.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0);
        nanos ^ (u64::from(std::process::id()) << 32)
    });
    seed.wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed)) | 1
}

fn served() -> &'static Mutex<HashMap<u64, Served>> {
    SERVED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Register the picture scheme once per process. The closure finds the
/// preview token in the map and clones what it needs, so the map lock is
/// never held while 7-Zip runs.
fn register() {
    REGISTER.call_once(|| {
        bern::files::serve(SCHEME, |url| {
            let token = url
                .strip_prefix(SCHEME)
                .and_then(|rest| rest.strip_prefix(':'))
                .and_then(|rest| rest.parse::<u64>().ok())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "无效的图片预览地址"))?;
            let served = lock(served())
                .get(&token)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "图片预览已关闭"))?;
            read(&served)
        });
    });
}

pub fn start(
    archive: PathBuf,
    password: Option<String>,
    entry: String,
    title: String,
    size: u64,
    wake: Waker,
) -> Session {
    register();
    let token = next_token();
    let status = Arc::new(Mutex::new(Status::default()));
    let closed = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(Mutex::new(()));
    lock(served()).insert(
        token,
        Served {
            archive,
            password,
            entry,
            size,
            status: status.clone(),
            closed: closed.clone(),
            busy: busy.clone(),
            wake: Some(wake),
        },
    );
    Session {
        token,
        url: MediaSource::new(format!("{SCHEME}:{token}")),
        title,
        status,
        closed,
        busy,
    }
}

impl Session {
    pub fn url(&self) -> MediaSource {
        self.url.clone()
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn snapshot(&self) -> Snapshot {
        let status = lock(&self.status);
        Snapshot {
            loading: status.loading,
            detail: status.detail.clone(),
            error: status.error.clone(),
        }
    }

    /// Stop an in-flight read and drop the token; the host keeps only the
    /// decoded picture it already has, under a URL no later preview reuses.
    /// Waits for the engine call to return so the caller can delete staging.
    pub fn stop(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            lock(served()).remove(&self.token);
        }
        let _busy = lock(&self.busy);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

fn fail(served: &Served, error: String) {
    if served.closed.load(Ordering::Relaxed) {
        return;
    }
    {
        let mut status = lock(&served.status);
        status.loading = false;
        status.error = Some(error.clone());
        status.detail = error;
    }
    if let Some(wake) = &served.wake {
        wake.wake();
    }
}

fn ready(served: &Served, info: &PictureInfo, bytes: u64) {
    if served.closed.load(Ordering::Relaxed) {
        return;
    }
    let detail = format!(
        "{} · {} × {} · {}",
        info.format,
        info.width,
        info.height,
        browser::size_text(bytes)
    );
    {
        let mut status = lock(&served.status);
        status.loading = false;
        status.error = None;
        status.detail = detail;
    }
    if let Some(wake) = &served.wake {
        wake.wake();
    }
}

fn read(served: &Served) -> io::Result<Box<dyn Read + Send>> {
    // One engine read per token at a time; `stop` waits on the same lock.
    let _busy = lock(&served.busy);
    if served.size > MAX_BYTES {
        fail(
            served,
            format!("图片超过 {}，暂不预览", browser::size_text(MAX_BYTES)),
        );
        return Err(io::Error::new(io::ErrorKind::InvalidData, "图片过大"));
    }
    let mut bytes = Vec::with_capacity(served.size.min(1024 * 1024) as usize);
    let mut stopped = false;
    let mut too_big = false;
    let outcome = backend::stream_entry(
        &served.archive,
        served.password.as_deref(),
        &served.entry,
        Some(served.closed.as_ref()),
        |chunk| {
            if served.closed.load(Ordering::Relaxed) {
                stopped = true;
                return false;
            }
            if bytes.len() as u64 + chunk.len() as u64 > MAX_BYTES {
                too_big = true;
                return false;
            }
            bytes.extend_from_slice(chunk);
            true
        },
    );
    if stopped {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "图片预览已关闭"));
    }
    if let Err(error) = outcome {
        fail(served, error.clone());
        return Err(io::Error::new(io::ErrorKind::Other, error));
    }
    if too_big {
        fail(
            served,
            format!("图片超过 {}，暂不预览", browser::size_text(MAX_BYTES)),
        );
        return Err(io::Error::new(io::ErrorKind::InvalidData, "图片过大"));
    }
    if bytes.is_empty() {
        fail(served, "这个条目没有可读取的图片数据".into());
        return Err(io::Error::new(io::ErrorKind::InvalidData, "图片为空"));
    }
    let Some(info) = dimensions(&bytes) else {
        fail(served, "无法识别的图片格式".into());
        return Err(io::Error::new(io::ErrorKind::InvalidData, "图片格式未知"));
    };
    if u64::from(info.width) * u64::from(info.height) > MAX_PIXELS {
        fail(
            served,
            format!("图片尺寸过大：{} × {}", info.width, info.height),
        );
        return Err(io::Error::new(io::ErrorKind::InvalidData, "图片像素过多"));
    }
    ready(served, &info, bytes.len() as u64);
    Ok(Box::new(Cursor::new(bytes)))
}

struct PictureInfo {
    format: &'static str,
    width: u32,
    height: u32,
}

/// Dimensions from the header alone: enough for the pixel cap and for the
/// status line, without decoding the picture twice.
fn dimensions(bytes: &[u8]) -> Option<PictureInfo> {
    png(bytes)
        .or_else(|| jpeg(bytes))
        .or_else(|| gif(bytes))
        .or_else(|| bmp(bytes))
        .or_else(|| webp(bytes))
}

fn png(bytes: &[u8]) -> Option<PictureInfo> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some(PictureInfo {
        format: "PNG",
        width,
        height,
    })
}

fn jpeg(bytes: &[u8]) -> Option<PictureInfo> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut index = 2;
    while index + 1 < bytes.len() {
        if bytes[index] != 0xFF {
            index += 1;
            continue;
        }
        let marker = bytes[index + 1];
        if marker == 0xFF {
            index += 1;
            continue;
        }
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            index += 2;
            continue;
        }
        if marker == 0xDA || marker == 0x00 {
            return None;
        }
        if index + 3 >= bytes.len() {
            return None;
        }
        let length = u16::from_be_bytes([bytes[index + 2], bytes[index + 3]]) as usize;
        if length < 2 {
            return None;
        }
        if matches!(
            marker,
            0xC0 | 0xC1
                | 0xC2
                | 0xC3
                | 0xC5
                | 0xC6
                | 0xC7
                | 0xC9
                | 0xCA
                | 0xCB
                | 0xCD
                | 0xCE
                | 0xCF
        ) {
            if index + 9 > bytes.len() {
                return None;
            }
            let height = u16::from_be_bytes([bytes[index + 5], bytes[index + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[index + 7], bytes[index + 8]]) as u32;
            return (width > 0 && height > 0).then_some(PictureInfo {
                format: "JPEG",
                width,
                height,
            });
        }
        index += 2 + length;
    }
    None
}

fn gif(bytes: &[u8]) -> Option<PictureInfo> {
    if bytes.len() < 10 || (&bytes[..6] != b"GIF87a" && &bytes[..6] != b"GIF89a") {
        return None;
    }
    let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    (width > 0 && height > 0).then_some(PictureInfo {
        format: "GIF",
        width,
        height,
    })
}

fn bmp(bytes: &[u8]) -> Option<PictureInfo> {
    if bytes.len() < 26 || &bytes[..2] != b"BM" {
        return None;
    }
    let width = i32::from_le_bytes(bytes[18..22].try_into().ok()?).unsigned_abs();
    let height = i32::from_le_bytes(bytes[22..26].try_into().ok()?).unsigned_abs();
    (width > 0 && height > 0).then_some(PictureInfo {
        format: "BMP",
        width,
        height,
    })
}

fn webp(bytes: &[u8]) -> Option<PictureInfo> {
    if bytes.len() < 30 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    match &bytes[12..16] {
        b"VP8 " => {
            let width = u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF;
            let height = u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF;
            (width > 0 && height > 0).then_some(PictureInfo {
                format: "WebP",
                width: width as u32,
                height: height as u32,
            })
        }
        b"VP8L" => {
            if bytes[20] != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            Some(PictureInfo {
                format: "WebP",
                width,
                height,
            })
        }
        b"VP8X" => {
            let width = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]) + 1;
            let height = u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]) + 1;
            Some(PictureInfo {
                format: "WebP",
                width,
                height,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 24];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        bytes[12..16].copy_from_slice(b"IHDR");
        bytes[16..20].copy_from_slice(&width.to_be_bytes());
        bytes[20..24].copy_from_slice(&height.to_be_bytes());
        bytes
    }

    #[test]
    fn png_ihdr_gives_the_size() {
        let info = dimensions(&png_bytes(1920, 1080)).unwrap();
        assert_eq!((info.format, info.width, info.height), ("PNG", 1920, 1080));
    }

    #[test]
    fn gif_and_bmp_headers_give_the_size() {
        let mut gif = vec![0u8; 10];
        gif[..6].copy_from_slice(b"GIF89a");
        gif[6..8].copy_from_slice(&320u16.to_le_bytes());
        gif[8..10].copy_from_slice(&200u16.to_le_bytes());
        let info = dimensions(&gif).unwrap();
        assert_eq!((info.format, info.width, info.height), ("GIF", 320, 200));

        let mut bmp = vec![0u8; 26];
        bmp[..2].copy_from_slice(b"BM");
        bmp[18..22].copy_from_slice(&640i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&(-480i32).to_le_bytes());
        let info = dimensions(&bmp).unwrap();
        assert_eq!((info.format, info.width, info.height), ("BMP", 640, 480));
    }

    #[test]
    fn jpeg_sof_gives_the_size() {
        let bytes = [
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x02, 0x00, 0x03,
        ];
        let info = dimensions(&bytes).unwrap();
        assert_eq!((info.format, info.width, info.height), ("JPEG", 3, 2));
    }

    #[test]
    fn webp_vp8x_and_vp8l_give_the_size() {
        let mut vp8x = vec![0u8; 30];
        vp8x[..4].copy_from_slice(b"RIFF");
        vp8x[8..12].copy_from_slice(b"WEBP");
        vp8x[12..16].copy_from_slice(b"VP8X");
        vp8x[24..27].copy_from_slice(&[0x3F, 0x02, 0x00]);
        vp8x[27..30].copy_from_slice(&[0x8F, 0x01, 0x00]);
        let info = dimensions(&vp8x).unwrap();
        assert_eq!((info.format, info.width, info.height), ("WebP", 576, 400));

        let mut vp8l = vec![0u8; 30];
        vp8l[..4].copy_from_slice(b"RIFF");
        vp8l[8..12].copy_from_slice(b"WEBP");
        vp8l[12..16].copy_from_slice(b"VP8L");
        vp8l[20] = 0x2F;
        let bits = 99u32 | (49u32 << 14);
        vp8l[21..25].copy_from_slice(&bits.to_le_bytes());
        let info = dimensions(&vp8l).unwrap();
        assert_eq!((info.format, info.width, info.height), ("WebP", 100, 50));
    }

    #[test]
    fn unknown_and_truncated_headers_are_rejected() {
        assert!(dimensions(b"").is_none());
        assert!(dimensions(b"<svg></svg>").is_none());
        assert!(dimensions(&[0xFF, 0xD8, 0xFF]).is_none());
        assert!(dimensions(&png_bytes(0, 0)).is_none());
    }
}
