//! S7 截图输出：PNG 编码、存盘、剪贴板、复制路径。
//!
//! **长图不在这里**：滚动截图（会话 62）用 `png::Writer::stream_writer()`
//! 直接逐行写盘，不经过 `encode_png`。本文件只处理单张截图。
//!
//! 图片**不走 IPC base64**（ADR-040）：这里只产出文件路径，前端要预览时用
//! `convertFileSrc` 读文件。剪贴板走 `arboard`，失败必须把原因交给用户。

use crate::config::Config;
use crate::ocr::CapturedImage;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 一次截图交付的结果。`message` 是给用户看的一句话。
#[derive(Debug, Clone)]
pub struct Delivered {
    pub path: Option<PathBuf>,
    pub message: String,
}

/// 系统「图片」文件夹下的 Vexto 子目录。
///
/// 与下载的 `下载\Vexto` 同构。不能直接拼 `%USERPROFILE%\Pictures`：
/// 用户可能把图片文件夹重定向到别的盘（`SHGetKnownFolderPath` 读的是真实路径）。
pub fn default_dir() -> PathBuf {
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{FOLDERID_Pictures, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

        let got = unsafe { SHGetKnownFolderPath(&FOLDERID_Pictures, KF_FLAG_DEFAULT, None) };
        if let Ok(ptr) = got {
            if !ptr.is_null() {
                let s = unsafe { ptr.to_string() }.unwrap_or_default();
                unsafe { CoTaskMemFree(Some(ptr.0 as *const core::ffi::c_void)) };
                if !s.trim().is_empty() {
                    return PathBuf::from(s).join("Vexto");
                }
            }
        }
    }
    let base = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("Pictures").join("Vexto")
}

/// 这次截图该落到哪个目录：设置 > 系统默认。
pub fn resolve_dir(cfg: &Config) -> PathBuf {
    let s = cfg.screenshot_dir.trim();
    if s.is_empty() {
        default_dir()
    } else {
        PathBuf::from(s)
    }
}

/// BGRA → RGBA。`CapturedImage` 的 alpha 恒 255，这里只交换 B/R。
fn bgra_to_rgba(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    let (chunks, _) = pixels.as_chunks::<4>();
    for px in chunks {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

/// 逐行写 PNG，避免再分配一整张 RGBA 缓冲。
fn write_png<W: Write>(w: W, img: &CapturedImage) -> Result<(), String> {
    let width = img.width as usize;
    let height = img.height as usize;
    if width == 0 || height == 0 {
        return Err("截图尺寸不合法".into());
    }
    let mut enc = png::Encoder::new(w, width as u32, height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| format!("写 PNG 头失败：{e}"))?;
    let mut stream = writer
        .stream_writer()
        .map_err(|e| format!("写 PNG 失败：{e}"))?;
    for row in img.pixels.chunks_exact(width * 4) {
        stream
            .write_all(&bgra_to_rgba(row))
            .map_err(|e| format!("写 PNG 失败：{e}"))?;
    }
    stream.finish().map_err(|e| format!("写 PNG 失败：{e}"))?;
    Ok(())
}

/// 存盘并返回最终路径（内部临时文件用，名字固定为「截图_时间」）。
/// 用户截图走 [`save_png_named`]，文件名由配置的命名规则生成。
pub fn save_png(img: &CapturedImage, dir: &Path) -> Result<PathBuf, String> {
    save_png_named(img, dir, &format!("{}.png", file_stem()))
}

/// 按指定文件名存盘。目录不存在会创建；重名递增 ` (1)`。
pub fn save_png_named(img: &CapturedImage, dir: &Path, name: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建截图目录失败（{}）：{e}", dir.display()))?;
    let path = unique_named_path(dir, name);
    let file = std::fs::File::create(&path)
        .map_err(|e| format!("创建截图文件失败（{}）：{e}", path.display()))?;
    let mut w = std::io::BufWriter::new(file);
    write_png(&mut w, img)?;
    w.flush().map_err(|e| format!("写截图文件失败：{e}"))?;
    Ok(path)
}

fn file_stem() -> String {
    let t = local_time();
    format!(
        "截图_{:04}{:02}{:02}_{:02}{:02}{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    )
}

/// 把命名规则渲染成最终文件名（会话 81）。
///
/// 规则里 `$...$` 是格式占位，可以出现多次；其余字符原样保留：
/// - `yyyyMMdd_HHmmss`（以及任意 `yyyy` / `MM` / `dd` / `HH` / `mm` / `ss` 组合）
/// - `uuid`：随机 UUID v4
/// - `unix`：Unix 时间戳（秒）
///
/// 生成的名称会过滤 Windows 非法字符、补 `.png` 后缀；规则不合法时返回错误，
/// 由设置页预览 / 保存路径回退处理。
pub fn render_name(rule: &str) -> Result<String, String> {
    render_name_at(rule, &local_time())
}

fn render_name_at(
    rule: &str,
    t: &windows::Win32::Foundation::SYSTEMTIME,
) -> Result<String, String> {
    let rule = rule.trim();
    if rule.is_empty() {
        return Err("命名规则不能为空".into());
    }
    let mut out = String::new();
    let mut rest = rule;
    let mut replaced = false;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('$') else {
            return Err("命名规则里的 $ 没有配对".into());
        };
        let token = after[..j].trim();
        if token.is_empty() {
            return Err("$ $ 之间不能为空".into());
        }
        out.push_str(&render_token(token, t)?);
        rest = &after[j + 1..];
        replaced = true;
    }
    out.push_str(rest);
    if !replaced {
        return Err(format!(
            "命名规则必须包含 $格式$，例如 {}",
            crate::config::DEFAULT_SCREENSHOT_NAME_RULE
        ));
    }
    let name = sanitize_file_name(&out);
    if name.is_empty() || name.eq_ignore_ascii_case(".png") {
        return Err("命名规则生成的文件名为空".into());
    }
    Ok(name)
}

fn render_token(
    token: &str,
    t: &windows::Win32::Foundation::SYSTEMTIME,
) -> Result<String, String> {
    let lower = token.to_ascii_lowercase();
    if lower == "uuid" {
        return Ok(uuid::Uuid::new_v4().to_string());
    }
    if lower == "unix" || lower == "timestamp" {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        return Ok(secs.to_string());
    }

    // 日期格式：只认固定写法，按最长匹配替换；没命中任何一项就当非法格式。
    let mut out = String::new();
    let mut hit = false;
    let mut i = 0;
    while i < token.len() {
        let rest = &token[i..];
        let (rep, adv) = if rest.starts_with("yyyy") {
            (format!("{:04}", t.wYear), 4)
        } else if rest.starts_with("MM") {
            (format!("{:02}", t.wMonth), 2)
        } else if rest.starts_with("dd") {
            (format!("{:02}", t.wDay), 2)
        } else if rest.starts_with("HH") {
            (format!("{:02}", t.wHour), 2)
        } else if rest.starts_with("mm") {
            (format!("{:02}", t.wMinute), 2)
        } else if rest.starts_with("ss") {
            (format!("{:02}", t.wSecond), 2)
        } else {
            let ch = rest.chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
            continue;
        };
        out.push_str(&rep);
        i += adv;
        hit = true;
    }
    if !hit {
        return Err(format!(
            "不支持的格式「{token}」，可用 yyyyMMdd_HHmmss / uuid / unix"
        ));
    }
    Ok(out)
}

fn sanitize_file_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    s = s.trim().to_string();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if !s.to_ascii_lowercase().ends_with(".png") {
        s.push_str(".png");
    }
    s
}

/// 按配置的命名规则生成文件名；规则坏掉时回退默认规则并记日志，
/// 不能因为用户手改坏配置就让每次截图都失败。
fn file_name_for(cfg: &Config) -> String {
    render_name(&cfg.screenshot_name_rule).unwrap_or_else(|e| {
        crate::dlog::warn(&format!(
            "截图命名规则不合法（{e}），回退默认：{}",
            cfg.screenshot_name_rule
        ));
        render_name(crate::config::DEFAULT_SCREENSHOT_NAME_RULE)
            .unwrap_or_else(|_| format!("{}.png", file_stem()))
    })
}

/// 完整文件名（含扩展名）去重：`name.png` → `name (1).png`。
fn unique_named_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "screenshot".to_string());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..10_000 {
        let q = dir.join(format!("{stem} ({n}){ext}"));
        if !q.exists() {
            return q;
        }
    }
    dir.join(format!("{stem} ({}){ext}", std::process::id()))
}

/// 长图 / 预览共用的下一个可用 PNG 路径（不覆盖已有文件，按配置规则命名）。
pub(crate) fn next_png_path(dir: &Path, cfg: &Config) -> PathBuf {
    unique_named_path(dir, &file_name_for(cfg))
}

/// 把一张已经编码好的 PNG（滚动长图 / 全屏结果）复制到输出目录。
/// 长图不走解码，避免整图进内存。
pub(crate) fn copy_file_to_dir(src: &Path, dir: &Path, cfg: &Config) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建截图目录失败：{e}"))?;
    let dst = unique_named_path(dir, &file_name_for(cfg));
    std::fs::copy(src, &dst).map_err(|e| format!("保存截图失败（{}）：{e}", dst.display()))?;
    Ok(dst)
}

#[cfg(windows)]
fn local_time() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

/// 写图片到剪贴板（RGBA）。
pub fn copy_image(img: &CapturedImage) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("打开剪贴板失败：{e}"))?;
    cb.set_image(arboard::ImageData {
        width: img.width.max(0) as usize,
        height: img.height.max(0) as usize,
        bytes: std::borrow::Cow::Owned(bgra_to_rgba(&img.pixels)),
    })
    .map_err(|e| format!("写剪贴板失败：{e}"))
}

/// 写文本到剪贴板（复制文件路径）。
pub fn copy_path(path: &Path) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("打开剪贴板失败：{e}"))?;
    cb.set_text(path.to_string_lossy().to_string())
        .map_err(|e| format!("写剪贴板失败：{e}"))
}

fn save_for(img: &CapturedImage, cfg: &Config) -> Result<PathBuf, String> {
    save_png_named(img, &resolve_dir(cfg), &file_name_for(cfg))
}

/// 按 `screenshot_after_action` 交付一张截图。
///
/// `both` 先复制图片再存盘：存盘失败时用户至少还有剪贴板里的图，
/// 错误消息里也会说清楚哪一步失败。
pub fn deliver(img: &CapturedImage, cfg: &Config) -> Result<Delivered, String> {
    let action = cfg.screenshot_after_action.trim();
    match action {
        "clipboard" => {
            copy_image(img)?;
            Ok(Delivered {
                path: None,
                message: "已复制到剪贴板".into(),
            })
        }
        "path" => {
            let path = save_for(img, cfg)?;
            if let Err(e) = copy_path(&path) {
                return Err(format!("已保存到 {}，但复制路径失败：{e}", path.display()));
            }
            Ok(Delivered {
                message: format!("已保存并复制路径：{}", path.display()),
                path: Some(path),
            })
        }
        "save" => {
            let path = save_for(img, cfg)?;
            Ok(Delivered {
                message: format!("已保存：{}", path.display()),
                path: Some(path),
            })
        }
        // 默认 both（也兜住用户手改坏的配置）
        _ => {
            copy_image(img)?;
            match save_for(img, cfg) {
                Ok(path) => Ok(Delivered {
                    message: format!("已复制并保存：{}", path.display()),
                    path: Some(path),
                }),
                Err(e) => Err(format!("图片已复制到剪贴板，但保存失败：{e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CapturedImage {
        // 2×2：红、绿、蓝、白（BGRA 顺序）
        CapturedImage {
            width: 2,
            height: 2,
            pixels: vec![
                0, 0, 255, 255, // 红
                0, 255, 0, 255, // 绿
                255, 0, 0, 255, // 蓝
                255, 255, 255, 255, // 白
            ],
        }
    }

    #[test]
    fn png_roundtrip_keeps_size_and_pixels() {
        let img = sample();
        let mut bytes = Vec::new();
        write_png(&mut bytes, &img).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.width, 2);
        assert_eq!(info.height, 2);
        // 第一个像素应是红：R=255,G=0,B=0
        assert_eq!(&buf[0..3], &[255, 0, 0]);
    }

    #[test]
    fn unique_path_increments() {
        let dir = std::env::temp_dir().join(format!("vexto-shot-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_named_path(&dir, "x.png");
        std::fs::write(&a, b"1").unwrap();
        let b = unique_named_path(&dir, "x.png");
        assert_ne!(a, b);
        assert!(b.to_string_lossy().contains("(1)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 存盘路径 + PNG 可解码。用临时目录，不碰用户真实的「图片」文件夹。
    #[test]
    fn save_png_writes_decodable_file() {
        let dir = std::env::temp_dir().join(format!("vexto-save-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let img = sample();
        let path = save_png(&img, &dir).unwrap();
        assert!(path.exists(), "文件不存在：{}", path.display());
        let bytes = std::fs::read(&path).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_image_is_rejected() {
        let img = CapturedImage {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };
        let mut out = Vec::new();
        assert!(write_png(&mut out, &img).is_err());
    }

    fn sample_time() -> windows::Win32::Foundation::SYSTEMTIME {
        windows::Win32::Foundation::SYSTEMTIME {
            wYear: 2026,
            wMonth: 11,
            wDay: 16,
            wHour: 10,
            wMinute: 3,
            wSecond: 5,
            wMilliseconds: 0,
            wDayOfWeek: 0,
        }
    }

    /// 默认规则：前缀保留、时间占位替换、后缀补 .png。
    #[test]
    fn default_rule_renders_prefix_and_time() {
        let name = render_name_at("Vexto_$yyyyMMdd_HHmmss$.png", &sample_time()).unwrap();
        assert_eq!(name, "Vexto_20261116_100305.png");
    }

    /// 多段占位、自定义分隔符、小写扩展名都按原样保留。
    #[test]
    fn rule_keeps_user_prefix_and_supports_multiple_tokens() {
        let name = render_name_at(
            "shot_$yyyy-MM-dd$_$HHmmss$",
            &sample_time(),
        )
        .unwrap();
        assert_eq!(name, "shot_2026-11-16_100305.png");
    }

    #[test]
    fn uuid_and_unix_tokens_are_supported() {
        let name = render_name_at("V_$uuid$.png", &sample_time()).unwrap();
        assert!(name.starts_with("V_") && name.ends_with(".png"));
        assert_eq!(name.len(), "V_".len() + 36 + ".png".len());
        let name = render_name_at("V_$unix$.png", &sample_time()).unwrap();
        let secs = name.trim_start_matches("V_").trim_end_matches(".png");
        assert!(secs.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn invalid_rules_are_rejected() {
        assert!(render_name_at("Vexto.png", &sample_time()).is_err(), "没有占位符");
        assert!(render_name_at("Vexto_$yyyy$", &sample_time()).is_ok());
        assert!(render_name_at("Vexto_$nope$", &sample_time()).is_err(), "未知格式");
        assert!(render_name_at("Vexto_$yyyy", &sample_time()).is_err(), "$ 未配对");
        assert!(render_name_at("   ", &sample_time()).is_err(), "空规则");
    }

    /// 非法字符要被替换、扩展名要补上，不能生成带路径的文件名。
    #[test]
    fn names_are_sanitized_for_windows() {
        let name = render_name_at("a/b\\c:d*$yyyy$.png", &sample_time()).unwrap();
        assert!(!name.contains('/') && !name.contains('\\') && !name.contains(':'));
        assert!(!name.contains('*'));
        assert!(name.ends_with(".png"));
    }

    /// 完整文件名去重保留扩展名：`x.png` → `x (1).png`。
    #[test]
    fn unique_named_path_keeps_extension() {
        let dir = std::env::temp_dir().join(format!("vexto-name-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_named_path(&dir, "Vexto_1.png");
        std::fs::write(&a, b"1").unwrap();
        let b = unique_named_path(&dir, "Vexto_1.png");
        assert_eq!(b.file_name().unwrap().to_string_lossy(), "Vexto_1 (1).png");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
