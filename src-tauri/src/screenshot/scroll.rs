//! 滚动长图主循环：抓帧 → 判到底 → 滚动 → 拼接 → 流式写盘。
//!
//! **内存红线**（AGENTS §6.3 / ADR-040）：长图不进内存。拼接时把新行按 RGB
//! 顺序追加到 `%APPDATA%\vexto` 下的临时原始行文件，滚完拿到总高后再用
//! `png::Writer::stream_writer()` 逐行读回并编码。内存里只有「上一帧 +
//! 当前帧 + 一行转换缓冲」。
//!
//! 这个文件只做像素与文件，不碰事件循环：进度通过回调交给 `main.rs` 发事件。

use crate::config::Config;
use crate::ocr::{capture_region, CapturedImage};
use crate::screenshot::input::{self, Action};
use crate::screenshot::output;
use crate::screenshot::stitch::{self, Frame, Quality, MAX_FRAMES, MAX_TOTAL_HEIGHT};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 用户点「停止」/ 再次按截图热键时置位。每轮 `run` 开头复位。
static STOP: AtomicBool = AtomicBool::new(false);
/// 是否有滚动截图在跑（热键据此决定「开浮层」还是「停止」）。
static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

struct RunGuard;

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Capturing,
    Writing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    pub frames: usize,
    pub height: i32,
    pub added: i32,
}

/// 三态：成功 / 部分成功 / 失败（06 §6.4.5 B 组要求三态都有明确反馈）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Success,
    PartiallySuccessful,
    Failed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Success => "success",
            Status::PartiallySuccessful => "partial",
            Status::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScrollDone {
    pub status: Status,
    pub width: i32,
    pub height: i32,
    pub frames: usize,
    pub path: Option<PathBuf>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Bottom,
    UserStopped,
    FrameLimit,
    HeightLimit,
    Ineffective,
    NoTarget,
    GeometryChanged,
}

/// 跑一次滚动截图。阻塞直到完成；调用方负责放到工作线程。
pub fn run(rect: Rect, cfg: &Config, on_progress: &mut dyn FnMut(Progress)) -> ScrollDone {
    STOP.store(false, Ordering::SeqCst);
    RUNNING.store(true, Ordering::SeqCst);
    let _guard = RunGuard;
    match run_inner(rect, cfg, on_progress) {
        Ok(done) => done,
        Err(message) => ScrollDone {
            status: Status::Failed,
            width: rect.w,
            height: 0,
            frames: 0,
            path: None,
            message,
        },
    }
}

fn view(img: &CapturedImage) -> Frame<'_> {
    Frame::new(img.width as usize, img.height as usize, &img.pixels)
}

fn run_inner(
    rect: Rect,
    cfg: &Config,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<ScrollDone, String> {
    if rect.w <= 0 || rect.h <= 0 {
        return Err("选区尺寸不合法".into());
    }
    // 框选浮层刚被销毁：等一拍再抓第一帧。窗口销毁是异步的，给它留够时间，
    // 否则第一帧会把半透明的浮层拼进长图（B11）。
    std::thread::sleep(Duration::from_millis(400));

    let first = capture_region(rect.x, rect.y, rect.w, rect.h)
        .map_err(|e| format!("抓取选区失败：{e}"))?;
    let width = first.width as usize;
    let height = first.height as usize;
    if width == 0 || height == 0 {
        return Err("选区尺寸不合法".into());
    }

    let cx = rect.x + rect.w / 2;
    let cy = rect.y + rect.h / 2;
    let target = input::target_at(cx, cy);

    let configured = Action::parse(&cfg.scroll_action);
    let manual = cfg.scroll_mode.trim().eq_ignore_ascii_case("manual")
        || configured == Action::Manual;
    let mut action = if manual { Action::Manual } else { configured };

    let temp_dir = crate::config::data_dir();
    let mut raw = RawRows::create(&temp_dir, width)?;
    raw.append(&first, 0, height)?;
    on_progress(Progress {
        stage: Stage::Capturing,
        frames: 1,
        height: height as i32,
        added: 0,
    });

    let mut total = height;
    let mut prev = first;
    let mut frames = 1usize;
    let mut fallback_seam = false;
    let mut skipped = false;
    let mut trimmed = false;
    let mut reason: Option<StopReason> = None;

    while reason.is_none() {
        if STOP.load(Ordering::SeqCst) {
            reason = Some(StopReason::UserStopped);
            break;
        }
        if frames >= MAX_FRAMES {
            reason = Some(StopReason::FrameLimit);
            break;
        }

        if manual {
            std::thread::sleep(Duration::from_millis(180));
        } else {
            let Some(t) = target else {
                reason = Some(StopReason::NoTarget);
                break;
            };
            if !input::scroll(t, action, cx, cy) {
                reason = Some(StopReason::NoTarget);
                break;
            }
            std::thread::sleep(Duration::from_millis(action.settle_ms()));
        }

        let mut cur = match capture_region(rect.x, rect.y, rect.w, rect.h) {
            Ok(img) => img,
            Err(e) => {
                return Ok(deliver(
                    cfg,
                    raw,
                    frames,
                    total > height,
                    Status::Failed,
                    format!("滚动过程中抓帧失败：{e}"),
                    on_progress,
                ))
            }
        };

        // 两帧全等：可能真的到底，也可能是滚动没生效（坑 6）。
        if stitch::frames_equal(view(&prev), view(&cur)) {
            if manual {
                // 手动模式：用户还没滚，等下一次。
                continue;
            }
            if target.and_then(input::at_bottom) == Some(true) {
                reason = Some(StopReason::Bottom);
                break;
            }
            if action == Action::Wheel {
                reason = Some(StopReason::Ineffective);
                break;
            }
            // 默认滚动消息没反应：回退滚轮再试一次（坑 4）。
            action = Action::Wheel;
            if let Some(t) = target {
                input::scroll(t, action, cx, cy);
            }
            std::thread::sleep(Duration::from_millis(action.settle_ms()));
            match capture_region(rect.x, rect.y, rect.w, rect.h) {
                Ok(img) => cur = img,
                Err(e) => {
                    return Ok(deliver(
                        cfg,
                        raw,
                        frames,
                        total > height,
                        Status::Failed,
                        format!("滚动过程中抓帧失败：{e}"),
                        on_progress,
                    ))
                }
            }
            if stitch::frames_equal(view(&prev), view(&cur)) {
                reason = Some(if target.and_then(input::at_bottom) == Some(true) {
                    StopReason::Bottom
                } else {
                    StopReason::Ineffective
                });
                break;
            }
        }

        match stitch::advance(view(&prev), view(&cur), width, total, MAX_TOTAL_HEIGHT) {
            Ok(adv) => {
                // 第二帧才认得出底部固定栏：之前写进临时文件的底部行要截掉，
                // 否则固定栏会留在长图的中间（坑：聊天输入框 / 任务栏）。
                if adv.ignored_bottom > 0 && total == height && !trimmed {
                    let keep = height.saturating_sub(adv.ignored_bottom);
                    raw.truncate(keep)?;
                    total = keep;
                }
                trimmed = true;
                raw.append(&cur, adv.first_new_row, adv.new_rows)?;
                total += adv.new_rows;
                if adv.quality == Quality::Fallback {
                    fallback_seam = true;
                }
                prev = cur;
                frames += 1;
                on_progress(Progress {
                    stage: Stage::Capturing,
                    frames,
                    height: total as i32,
                    added: adv.new_rows as i32,
                });
            }
            Err(stitch::StitchError::FrameTallerThanRemaining { .. }) => {
                reason = Some(StopReason::HeightLimit);
                break;
            }
            Err(stitch::StitchError::NoMatch) => {
                if manual {
                    // 手动模式：抓到的可能正好是平滑滚动的中间帧，和上一帧对不上。
                    // 必须把基线换成当前帧，否则 `prev` 永远停在那一帧上，
                    // 后面所有帧都会一直 NoMatch（自测实测：结果只剩单帧）。
                    // 换基线意味着跳过了一段没拼上的内容，最终按部分成功提示。
                    crate::dlog::dlog("scroll: manual 帧无法拼接，重建基线（结果标记为不完整）");
                    skipped = true;
                    prev = cur;
                    continue;
                }
                if action != Action::Wheel {
                    action = Action::Wheel;
                    continue;
                }
                reason = Some(StopReason::Ineffective);
                break;
            }
            Err(e) => {
                // 宽度 / 帧高变化：继续拼必然错位，保留已拼出的部分并明确失败。
                let _ = e;
                reason = Some(StopReason::GeometryChanged);
                break;
            }
        }
    }

    let reason = reason.unwrap_or(StopReason::UserStopped);
    let (mut status, message) = classify(reason, total > height, fallback_seam);
    let mut message = if fallback_seam && status != Status::Failed {
        format!("{message}；部分接缝为兜底匹配，可能不准")
    } else {
        message
    };
    if skipped {
        if status == Status::Success {
            status = Status::PartiallySuccessful;
        }
        message.push_str("；部分内容未能拼接，结果可能不完整");
    }
    Ok(deliver(
        cfg,
        raw,
        frames,
        total > height,
        status,
        message,
        on_progress,
    ))
}

/// 停止原因 → 三态 + 中文说明。纯函数，单测覆盖。
fn classify(reason: StopReason, progressed: bool, fallback_seam: bool) -> (Status, String) {
    let status = match reason {
        StopReason::Bottom | StopReason::UserStopped => {
            if fallback_seam {
                Status::PartiallySuccessful
            } else {
                Status::Success
            }
        }
        StopReason::FrameLimit | StopReason::HeightLimit => Status::PartiallySuccessful,
        StopReason::Ineffective | StopReason::NoTarget => {
            if progressed {
                Status::PartiallySuccessful
            } else {
                Status::Failed
            }
        }
        StopReason::GeometryChanged => Status::Failed,
    };
    let message = match reason {
        StopReason::Bottom => "滚动到底，已拼出完整长图".to_string(),
        StopReason::UserStopped => "已按用户停止保存".to_string(),
        StopReason::FrameLimit => format!("达到 {MAX_FRAMES} 帧上限，已保留已拼出的部分"),
        StopReason::HeightLimit => {
            format!("达到 {MAX_TOTAL_HEIGHT} px 总高上限，已保留已拼出的部分")
        }
        StopReason::Ineffective => {
            if progressed {
                "滚动未生效，已保留已拼出的部分（可能没截全）".to_string()
            } else {
                "滚动未生效，目标窗口没有响应".to_string()
            }
        }
        StopReason::NoTarget => {
            if progressed {
                "没有找到可滚动的窗口，已保留已拼出的部分".to_string()
            } else {
                "没有找到可滚动的窗口".to_string()
            }
        }
        StopReason::GeometryChanged => "目标窗口尺寸变化，已保留已拼出的部分".to_string(),
    };
    (status, message)
}

/// 结束处理：够进度就编码落盘；失败且一帧都没拼出去时不产出「单帧假图」。
fn deliver(
    cfg: &Config,
    mut raw: RawRows,
    frames: usize,
    progressed: bool,
    status: Status,
    message: String,
    on_progress: &mut dyn FnMut(Progress),
) -> ScrollDone {
    let width = raw.width as i32;
    let height = raw.height as i32;

    // 一帧都没动过且失败：明确失败，不落一张「看起来像长图」的单帧（B7）。
    if status == Status::Failed && !progressed {
        return ScrollDone {
            status,
            width,
            height: 0,
            frames,
            path: None,
            message,
        };
    }

    on_progress(Progress {
        stage: Stage::Writing,
        frames,
        height,
        added: 0,
    });

    let dir = output::resolve_dir(cfg);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return ScrollDone {
            status: Status::Failed,
            width,
            height,
            frames,
            path: None,
            message: format!("{message}；创建截图目录失败（{}）：{e}", dir.display()),
        };
    }
    let path = output::next_png_path(&dir);
    if let Err(e) = raw.encode_png(&path) {
        return ScrollDone {
            status: Status::Failed,
            width,
            height,
            frames,
            path: None,
            message: format!("{message}；{e}"),
        };
    }

    let action = cfg.screenshot_after_action.trim();
    let mut message = format!("{message}（{width}×{height}）");
    if matches!(action, "both" | "path") {
        match output::copy_path(&path) {
            Ok(()) => message.push_str("；文件路径已复制"),
            Err(e) => message.push_str(&format!("；复制路径失败：{e}")),
        }
    }
    message.push_str(&format!("：{}", path.display()));

    ScrollDone {
        status,
        width,
        height,
        frames,
        path: Some(path),
        message,
    }
}

/// 临时原始行文件：每行 RGB（不含 alpha），顺序追加。
///
/// 长图的最终高度在滚动结束前未知，而 `stream_writer()` 要求写头时就给出高度，
/// 所以先把像素落到磁盘（门 3 选 B），结束时再流式编码成 PNG。
struct RawRows {
    file: std::fs::File,
    path: PathBuf,
    width: usize,
    height: usize,
    row: Vec<u8>,
}

impl RawRows {
    fn create(dir: &Path, width: usize) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = dir.join(format!("scroll-{}-{stamp}.raw", std::process::id()));
        // 必须同时可读可写：`File::create` 只给写权限，结束时流式编码读不回来
        // （Windows 上表现为 os error 5「拒绝访问」）。
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("创建长图临时文件失败：{e}"))?;
        Ok(Self {
            file,
            path,
            width,
            height: 0,
            row: Vec::with_capacity(width * 3),
        })
    }

    /// 从 `img` 的第 `first_row` 行起追加 `count` 行（BGRA → RGB）。
    fn append(
        &mut self,
        img: &CapturedImage,
        first_row: usize,
        count: usize,
    ) -> Result<(), String> {
        let w = self.width;
        let end = first_row
            .checked_add(count)
            .ok_or_else(|| "拼接行号溢出".to_string())?;
        if w == 0
            || img.width as usize != w
            || end > img.height as usize
            || img.pixels.len() < img.height as usize * w * 4
        {
            return Err("拼接行超出当前帧范围".into());
        }
        let stride = w * 4;
        for y in first_row..end {
            let src = &img.pixels[y * stride..(y + 1) * stride];
            self.row.clear();
            let (chunks, _) = src.as_chunks::<4>();
            for px in chunks {
                self.row.extend_from_slice(&[px[2], px[1], px[0]]);
            }
            self.file
                .write_all(&self.row)
                .map_err(|e| format!("写长图临时文件失败：{e}"))?;
        }
        self.height += count;
        Ok(())
    }

    /// 丢掉尾部若干行（第一帧的底部固定栏在第二帧才认出来）。
    fn truncate(&mut self, rows: usize) -> Result<(), String> {
        let len = (rows * self.width * 3) as u64;
        self.file
            .set_len(len)
            .map_err(|e| format!("截断长图临时文件失败：{e}"))?;
        self.file
            .seek(SeekFrom::Start(len))
            .map_err(|e| format!("定位长图临时文件失败：{e}"))?;
        self.height = rows;
        Ok(())
    }

    fn encode_png(&mut self, out: &Path) -> Result<(), String> {
        self.file
            .flush()
            .map_err(|e| format!("刷新长图临时文件失败：{e}"))?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| format!("定位长图临时文件失败：{e}"))?;
        let result = write_scroll_png(&mut self.file, self.width, self.height, out);
        if result.is_err() {
            let _ = std::fs::remove_file(out);
        }
        result
    }
}

impl Drop for RawRows {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 把原始行文件逐行流式编码成 RGB PNG。
fn write_scroll_png(
    raw: &mut std::fs::File,
    width: usize,
    height: usize,
    out: &Path,
) -> Result<(), String> {
    let file = std::fs::File::create(out)
        .map_err(|e| format!("创建长图文件失败（{}）：{e}", out.display()))?;
    let mut out_buf = std::io::BufWriter::new(file);
    {
        let mut enc = png::Encoder::new(&mut out_buf, width as u32, height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| format!("写 PNG 头失败：{e}"))?;
        let mut stream = writer
            .stream_writer()
            .map_err(|e| format!("写长图失败：{e}"))?;
        let expected = (width * height * 3) as u64;
        let reader = std::io::BufReader::new(raw);
        let copied = std::io::copy(&mut reader.take(expected), &mut stream)
            .map_err(|e| format!("写长图失败：{e}"))?;
        if copied != expected {
            return Err(format!(
                "长图临时文件不完整（{copied}/{expected} 字节）"
            ));
        }
        stream.finish().map_err(|e| format!("写长图失败：{e}"))?;
    }
    out_buf
        .flush()
        .map_err(|e| format!("写长图文件失败：{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `values` 每个元素代表一整行（同一灰度铺满整行）。
    fn image(width: i32, values: &[u8]) -> CapturedImage {
        let height = values.len() as i32;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for v in values {
            for _ in 0..width {
                pixels.extend_from_slice(&[*v, v.wrapping_add(1), v.wrapping_add(2), 255]);
            }
        }
        CapturedImage {
            width,
            height,
            pixels,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vexto-scroll-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 原始行 → PNG：尺寸、像素顺序（RGB）都对，临时文件不残留。
    #[test]
    fn raw_rows_encode_to_decodable_png() {
        let dir = temp_dir("png");
        let img = image(3, &[10, 20, 30, 40]);
        let mut raw = RawRows::create(&dir, 3).unwrap();
        raw.append(&img, 1, 3).unwrap();
        assert_eq!(raw.height, 3);
        let out = dir.join("out.png");
        raw.encode_png(&out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (3, 3));
        // 第一行来自 img 的第 1 行：BGRA 的 B=20/G=21/R=22 → PNG 的 RGB 22/21/20
        assert_eq!(&buf[0..3], &[22, 21, 20]);
        let raw_path = raw.path.clone();
        drop(raw);
        assert!(!raw_path.exists(), "临时文件必须在 Drop 时删除");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 截掉尾部行后，PNG 高度跟着变。
    #[test]
    fn truncate_shrinks_result() {
        let dir = temp_dir("trunc");
        let img = image(2, &[1, 2, 3, 4]);
        let mut raw = RawRows::create(&dir, 2).unwrap();
        raw.append(&img, 0, 4).unwrap();
        raw.truncate(2).unwrap();
        assert_eq!(raw.height, 2);
        let out = dir.join("trunc.png");
        raw.encode_png(&out).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!(info.height, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 越界追加必须报错，不许 panic。
    #[test]
    fn append_out_of_range_is_rejected() {
        let dir = temp_dir("range");
        let img = image(2, &[1, 2, 3, 4]);
        let mut raw = RawRows::create(&dir, 2).unwrap();
        assert!(raw.append(&img, 3, 4).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 三态分类：到底 = 成功；上限 = 部分成功；没滚动 = 失败。
    #[test]
    fn stop_reasons_map_to_three_states() {
        assert_eq!(classify(StopReason::Bottom, true, false).0, Status::Success);
        assert_eq!(
            classify(StopReason::Bottom, true, true).0,
            Status::PartiallySuccessful
        );
        assert_eq!(
            classify(StopReason::FrameLimit, true, false).0,
            Status::PartiallySuccessful
        );
        assert_eq!(
            classify(StopReason::HeightLimit, true, false).0,
            Status::PartiallySuccessful
        );
        assert_eq!(
            classify(StopReason::Ineffective, false, false).0,
            Status::Failed
        );
        assert_eq!(
            classify(StopReason::Ineffective, true, false).0,
            Status::PartiallySuccessful
        );
        assert_eq!(
            classify(StopReason::UserStopped, true, false).0,
            Status::Success
        );
        assert_eq!(
            classify(StopReason::GeometryChanged, true, false).0,
            Status::Failed
        );
    }
}
