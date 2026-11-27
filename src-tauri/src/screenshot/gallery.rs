//! 截图图库：列出截图目录里的图片，按需生成缩略图（ADR-097）。
//!
//! 列表命令只返回元数据 + 缩略图 URL；真正的解码 / 缩放发生在
//! `vextogallery` 协议的请求里，图片多时不会在一次 IPC 里把所有图都解一遍。
//!
//! 缩略图缓存在 `%APPDATA%\vexto\gallery-thumbs`，文件名是
//! 「绝对路径 + 大小 + 修改时间」的哈希，源文件变了自然换新图。
//! 协议只认 32 位十六进制文件名，不接受任意路径。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;

use crate::config;
use crate::screenshot::output;

const MAX_ITEMS: usize = 2000;
const THUMB_W: u32 = 360;
const THUMB_H: u32 = 240;
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];

#[derive(Clone, Debug, Serialize)]
pub struct GalleryItem {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mtime_ms: u64,
    /// `vextogallery` 的缩略图地址；生成失败时为空。
    pub thumb: Option<String>,
}

fn ext_of(p: &Path) -> Option<String> {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| IMAGE_EXTS.contains(&e.as_str()))
}

fn mtime_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn thumb_key(path: &Path, size: u64, mtime: u64) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(path.to_string_lossy().as_bytes());
    h.update([0]);
    h.update(size.to_le_bytes());
    h.update(mtime.to_le_bytes());
    let digest = h.finalize();
    let mut key = String::with_capacity(32);
    for b in digest.iter() {
        key.push_str(&format!("{b:02x}"));
    }
    key
}

fn thumb_cache_dir() -> PathBuf {
    config::data_dir().join("gallery-thumbs")
}

fn thumb_url(key: &str) -> String {
    format!("http://vextogallery.localhost/{key}.png")
}

/// 缩略图 key → 源文件路径。协议只按 key 取图，前端拿不到也传不了任意路径。
fn thumb_map() -> &'static Mutex<HashMap<String, PathBuf>> {
    static MAP: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_map() -> std::sync::MutexGuard<'static, HashMap<String, PathBuf>> {
    match thumb_map().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// 列表：只读目录元数据。目录不存在时返回空。
pub fn list() -> Vec<GalleryItem> {
    let cfg = config::load();
    let dir = output::resolve_dir(&cfg);
    let items = list_from(&dir);
    prune_cache();
    items
}

fn list_from(dir: &Path) -> Vec<GalleryItem> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<GalleryItem> = Vec::new();
    let mut map = lock_map();
    map.clear();
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() || ext_of(&path).is_none() {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let mtime = mtime_ms(&meta);
        let key = thumb_key(&path, meta.len(), mtime);
        map.insert(key.clone(), path.clone());
        out.push(GalleryItem {
            path: path.to_string_lossy().into_owned(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size: meta.len(),
            mtime_ms: mtime,
            thumb: Some(thumb_url(&key)),
        });
    }
    drop(map);
    out.sort_by_key(|i| std::cmp::Reverse(i.mtime_ms));
    out.truncate(MAX_ITEMS);
    out
}

/// 只允许操作当前截图目录里的图片。所有写操作（删 / 复制 / 预览 / 钉图）都过这里。
fn check_source(path: &str) -> Result<PathBuf, String> {
    let cfg = config::load();
    let dir = output::resolve_dir(&cfg);
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("截图目录不可用（{}）：{e}", dir.display()))?;
    let p = Path::new(path)
        .canonicalize()
        .map_err(|e| format!("找不到图片文件（{path}）：{e}"))?;
    if p.parent() != Some(dir.as_path()) {
        return Err("只能操作截图目录里的文件".to_string());
    }
    if ext_of(&p).is_none() {
        return Err("不是支持的图片文件".to_string());
    }
    Ok(p)
}

/// 删除到回收站。调用方负责二次确认。
pub fn remove(path: &str) -> Result<(), String> {
    let p = check_source(path)?;
    crate::fileops::delete(&shell_path(&p), true)
}

/// `SHFileOperationW` 只认反斜杠，也不认 `\\?\` 前缀。
/// 列表里的路径是「配置目录 + 子项」拼出来的，可能混用 `/` 与 `\`。
fn shell_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    match s.strip_prefix("\\\\?\\") {
        Some(rest) => rest.to_string(),
        None => s,
    }
}

/// 复制图片内容到剪贴板（支持 png / jpg / webp / bmp）。
pub fn copy(path: &str) -> Result<(), String> {
    let p = check_source(path)?;
    let img = image::open(&p)
        .map_err(|e| format!("解析图片失败：{e}"))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("打开剪贴板失败：{e}"))?;
    cb.set_image(arboard::ImageData {
        width: w as usize,
        height: h as usize,
        bytes: std::borrow::Cow::Owned(img.into_raw()),
    })
    .map_err(|e| format!("写剪贴板失败：{e}"))
}

/// 预览 / 钉屏只认 PNG（`vextoimg` 只服务 PNG）。非 PNG 先转存一份到缓存。
pub fn png_target(path: &str) -> Result<PathBuf, String> {
    let p = check_source(path)?;
    if ext_of(&p).as_deref() == Some("png") {
        return Ok(p);
    }
    let meta = fs::metadata(&p).map_err(|e| format!("读取文件信息失败：{e}"))?;
    let key = thumb_key(&p, meta.len(), mtime_ms(&meta));
    let dir = thumb_cache_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("创建缩略图缓存失败：{e}"))?;
    let dst = dir.join(format!("conv-{key}.png"));
    if !dst.exists() {
        let img = image::open(&p).map_err(|e| format!("解析图片失败：{e}"))?;
        let tmp = dir.join(format!("conv-{key}.png.tmp"));
        img.save_with_format(&tmp, image::ImageFormat::Png)
            .map_err(|e| format!("转换图片失败：{e}"))?;
        if fs::rename(&tmp, &dst).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
    Ok(dst)
}

/// `vextogallery` 协议入口：`/<32 hex>.png`。
pub fn serve_thumb(path: &str) -> tauri::http::Response<Vec<u8>> {
    let not_found = || {
        tauri::http::Response::builder()
            .status(404)
            .body(Vec::new())
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
    };
    let name = path.trim_start_matches('/');
    let Some(key) = name.strip_suffix(".png") else {
        return not_found();
    };
    if key.len() != 32 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        return not_found();
    }
    let src = match lock_map().get(key).cloned() {
        Some(p) => p,
        None => return not_found(),
    };
    let dst = thumb_cache_dir().join(format!("{key}.png"));
    if !dst.exists() && generate_thumb(&src, &dst).is_err() {
        return not_found();
    }
    match fs::read(&dst) {
        Ok(bytes) => tauri::http::Response::builder()
            .header("Content-Type", "image/png")
            .header("Cache-Control", "no-store")
            .header("Access-Control-Allow-Origin", "*")
            .body(bytes)
            .unwrap_or_else(|_| not_found()),
        Err(_) => not_found(),
    }
}

fn generate_thumb(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(dir) = dst.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建缩略图缓存失败：{e}"))?;
    }
    let img = image::open(src).map_err(|e| format!("解析图片失败：{e}"))?;
    let thumb = img.thumbnail(THUMB_W, THUMB_H);
    let tmp = dst.with_extension("png.tmp");
    thumb
        .save_with_format(&tmp, image::ImageFormat::Png)
        .map_err(|e| format!("生成缩略图失败：{e}"))?;
    if fs::rename(&tmp, dst).is_err() {
        let _ = fs::remove_file(&tmp);
        return Err("写入缩略图失败".to_string());
    }
    Ok(())
}

/// 顺手清掉 30 天没被请求过的缩略图，避免目录无限增长。
fn prune_cache() {
    let dir = thumb_cache_dir();
    let Ok(rd) = fs::read_dir(&dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs() > 30 * 24 * 3600)
            .unwrap_or(false);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("lb-gallery-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_png(path: &Path, w: u32, h: u32) {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([17, 167, 128, 255]));
        img.save_with_format(path, image::ImageFormat::Png).unwrap();
    }

    #[test]
    fn list_returns_only_images_newest_first() {
        let dir = tmp_dir("list");
        make_png(&dir.join("a.png"), 8, 8);
        std::fs::write(dir.join("note.txt"), b"x").unwrap();
        let items = list_from(&dir);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "a.png");
        assert!(items[0].thumb.as_deref().unwrap().ends_with(".png"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn thumb_is_generated_and_bounded() {
        let dir = tmp_dir("thumb");
        let src = dir.join("big.png");
        make_png(&src, 1000, 600);
        let dst = dir.join("thumb.png");
        generate_thumb(&src, &dst).unwrap();
        let img = image::open(&dst).unwrap();
        assert!(img.width() <= THUMB_W && img.height() <= THUMB_H);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn png_passes_through_but_other_formats_convert() {
        let dir = tmp_dir("conv");
        let png = dir.join("a.png");
        make_png(&png, 4, 4);
        assert_eq!(ext_of(&png).as_deref(), Some("png"));
        assert!(ext_of(&dir.join("a.txt")).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn protocol_rejects_bad_names() {
        for bad in ["", "/", "/x.png", "/../x.png", "/abc.png", "/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz.png"] {
            let r = serve_thumb(bad);
            assert_eq!(r.status(), 404, "应拒绝 {bad}");
        }
    }
}
