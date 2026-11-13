//! 滚动长图的两帧拼接（纯函数，不碰窗口 / GDI / 文件）。
//!
//! 输入上一帧与当前帧的像素视图，输出「当前帧里哪几行是新内容」。
//! 这里只做判定，像素写盘由 `scroll.rs` 负责，所以所有阈值都能单测。
//!
//! 判定顺序：
//! 1. 宽度 / 帧高变化 → 失败（窗口被拖动或改了大小）；
//! 2. 剩余容量放不下整帧 → 失败（调用方按上限记为部分成功）；
//! 3. 自动识别底部固定栏（任务栏 / 聊天输入框）；
//! 4. 逐行精确比较找偏移（忽略左右边缘的动态内容）；
//! 5. 没有精确匹配时才退到「大部分行一致」的兜底命中，并标记质量降级。

/// 像素格式不参与比较，只要求每像素 4 字节（BGRA / RGBA 都行）。
pub const BYTES_PER_PIXEL: usize = 4;
/// 单次滚动截图最多抓多少帧（防无限滚动页面写满磁盘）。
pub const MAX_FRAMES: usize = 100;
/// 长图最大总高（物理像素）。
pub const MAX_TOTAL_HEIGHT: usize = 20_000;

/// 兜底命中阈值：重叠区至少 85% 的行一致才算「大概对上了」。
const FALLBACK_NUM: usize = 17;
const FALLBACK_DEN: usize = 20;
/// 兜底命中至少要有这么多一致行，避免两行白底被当成接缝。
const FALLBACK_MIN_ROWS: usize = 8;

/// 一帧像素的只读视图（自上而下、每像素 4 字节）。
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: &'a [u8],
}

impl<'a> Frame<'a> {
    pub fn new(width: usize, height: usize, pixels: &'a [u8]) -> Self {
        Self {
            width,
            height,
            pixels,
        }
    }

    fn row(&self, y: usize) -> &'a [u8] {
        let stride = self.width * BYTES_PER_PIXEL;
        let start = y * stride;
        &self.pixels[start..start + stride]
    }

    fn is_consistent(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.pixels.len() >= self.width * self.height * BYTES_PER_PIXEL
    }
}

/// 接缝可信度。`Fallback` 表示没有整行精确匹配，只能按比例兜底，
/// 调用方必须把最终结果标成 `PartiallySuccessful`（前端提示「接缝可能不准」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Exact,
    Fallback,
}

/// 一次拼接的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advance {
    /// 当前帧中第一行新内容的下标。
    pub first_new_row: usize,
    /// 新增行数（长图要追加的行数）。
    pub new_rows: usize,
    /// 当前帧底部被当成固定栏忽略的行数。
    pub ignored_bottom: usize,
    pub quality: Quality,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StitchError {
    /// 实际像素长度与声明的尺寸对不上（调用方构造错误）。
    EmptyFrame,
    /// 宽度变化：窗口被拖到别处或改了大小，继续拼必然错位。
    WidthChanged {
        previous: usize,
        current: usize,
        result: usize,
    },
    /// 两帧高度不同：窗口高度变了。
    HeightChanged {
        previous: usize,
        current: usize,
    },
    /// 剩余可写高度放不下这一帧（总高上限）。
    FrameTallerThanRemaining {
        frame: usize,
        remaining: usize,
    },
    /// 两帧之间找不到重叠：滚动没生效，或内容变化太大。
    NoMatch,
}

/// 左右各忽略多少像素。`clamp(w/20, 50, w/3)` 在窄窗口上会 `min > max`
/// 让 `clamp` 直接 panic，所以先把下限压到上限以内。
pub fn side_offset(width: usize) -> usize {
    let hi = width / 3;
    let lo = 50.min(hi);
    (width / 20).clamp(lo, hi)
}

/// 两帧是否完全一致（含尺寸）。用于「可能到底 / 滚动未生效」判定。
pub fn frames_equal(a: Frame<'_>, b: Frame<'_>) -> bool {
    if a.width != b.width || a.height != b.height {
        return false;
    }
    let len = a.width * a.height * BYTES_PER_PIXEL;
    a.pixels.len() >= len && b.pixels.len() >= len && a.pixels[..len] == b.pixels[..len]
}

/// 比较 `current[0..overlap]` 与 `previous[offset..offset+overlap]`，
/// 只看中间列（左右 `side` 像素忽略，滚动条 / 时间戳 / 广告位不影响接缝）。
fn rows_match(
    previous: Frame<'_>,
    current: Frame<'_>,
    offset: usize,
    overlap: usize,
    side: usize,
    width: usize,
) -> bool {
    let end = width - side;
    (0..overlap).all(|i| {
        previous.row(i + offset)[side * BYTES_PER_PIXEL..end * BYTES_PER_PIXEL]
            == current.row(i)[side * BYTES_PER_PIXEL..end * BYTES_PER_PIXEL]
    })
}

/// 自动识别底部固定栏：从最下面一行往上数，两帧逐行相同的连续行数。
///
/// 任务栏 / 聊天输入框 / 播放条这类固定元素不随页面滚动，如果当成内容参与
/// 匹配，每一帧都会把固定栏拼进图里。少于 3 行不认（避免把白底误当固定栏）。
fn auto_ignore_bottom_edge(previous: Frame<'_>, current: Frame<'_>, side: usize) -> usize {
    let max = previous.height / 4;
    let width = previous.width;
    let end = width - side;
    let mut n = 0;
    while n < max {
        let y = previous.height - 1 - n;
        if previous.row(y)[side * BYTES_PER_PIXEL..end * BYTES_PER_PIXEL]
            != current.row(y)[side * BYTES_PER_PIXEL..end * BYTES_PER_PIXEL]
        {
            break;
        }
        n += 1;
    }
    if n < 3 {
        0
    } else {
        n
    }
}

/// 两帧 → 新增行。
///
/// `result_height` 是已拼出的总高，`max_result_height` 是硬上限。
/// 成功时返回的 `first_new_row` 是**当前帧**里的行下标，
/// 调用方按 `[first_new_row, first_new_row + new_rows)` 追加 RGB 行。
pub fn advance(
    previous: Frame<'_>,
    current: Frame<'_>,
    result_width: usize,
    result_height: usize,
    max_result_height: usize,
) -> Result<Advance, StitchError> {
    if !previous.is_consistent() || !current.is_consistent() || result_width == 0 {
        return Err(StitchError::EmptyFrame);
    }
    if previous.width != current.width || current.width != result_width {
        return Err(StitchError::WidthChanged {
            previous: previous.width,
            current: current.width,
            result: result_width,
        });
    }
    if previous.height != current.height {
        return Err(StitchError::HeightChanged {
            previous: previous.height,
            current: current.height,
        });
    }
    let remaining = max_result_height.saturating_sub(result_height);
    if current.height > remaining {
        return Err(StitchError::FrameTallerThanRemaining {
            frame: current.height,
            remaining,
        });
    }

    let width = current.width;
    let height = current.height;
    let side = side_offset(width);
    let ignored_bottom = auto_ignore_bottom_edge(previous, current, side);
    let usable = height - ignored_bottom;
    let match_limit = (height / 2).min(usable.saturating_sub(1));

    for offset in 1..=match_limit {
        let overlap = usable - offset;
        if overlap == 0 {
            break;
        }
        if rows_match(previous, current, offset, overlap, side, width) {
            return Ok(Advance {
                first_new_row: overlap,
                new_rows: offset,
                ignored_bottom,
                quality: Quality::Exact,
            });
        }
    }

    // 没有精确匹配：找「一致行最多」的偏移，够 85% 才认，否则明确报无匹配。
    let mut best: Option<(usize, usize)> = None;
    for offset in 1..=match_limit {
        let overlap = usable - offset;
        if overlap == 0 {
            break;
        }
        let mut matched = 0usize;
        let mut abandoned = false;
        for i in 0..overlap {
            let same = previous.row(i + offset)[side * BYTES_PER_PIXEL..(width - side) * BYTES_PER_PIXEL]
                == current.row(i)[side * BYTES_PER_PIXEL..(width - side) * BYTES_PER_PIXEL];
            if same {
                matched += 1;
            } else if best.is_some_and(|(m, _)| matched + (overlap - i - 1) < m) {
                // 剩下的全对上也无法超过当前最佳，提前放弃这个偏移。
                abandoned = true;
                break;
            }
        }
        if !abandoned && best.is_none_or(|(m, _)| matched > m) {
            best = Some((matched, offset));
        }
    }

    if let Some((matched, offset)) = best {
        let overlap = usable - offset;
        if matched >= FALLBACK_MIN_ROWS && matched * FALLBACK_DEN >= overlap * FALLBACK_NUM {
            return Ok(Advance {
                first_new_row: overlap,
                new_rows: offset,
                ignored_bottom,
                quality: Quality::Fallback,
            });
        }
    }
    Err(StitchError::NoMatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 60;
    const H: usize = 40;

    /// 生成一帧：每行一个灰度值（第 y 行 = y + base），方便造「滚动」。
    fn frame_with_rows(base: usize, count: usize) -> Vec<u8> {
        let mut px = vec![0u8; W * count * BYTES_PER_PIXEL];
        for y in 0..count {
            let v = ((base + y) % 251) as u8;
            for x in 0..W {
                let o = (y * W + x) * BYTES_PER_PIXEL;
                px[o] = v;
                px[o + 1] = v;
                px[o + 2] = v;
                px[o + 3] = 255;
            }
        }
        px
    }

    fn view(pixels: &[u8], height: usize) -> Frame<'_> {
        Frame::new(W, height, pixels)
    }

    /// 向下滚 10 行：当前帧的第 i 行 = 上一帧第 i+10 行。
    #[test]
    fn exact_offset_is_found() {
        let prev = frame_with_rows(0, H);
        let cur = frame_with_rows(10, H);
        let adv = advance(
            view(&prev, H),
            view(&cur, H),
            W,
            H,
            MAX_TOTAL_HEIGHT,
        )
        .unwrap();
        assert_eq!(adv.new_rows, 10);
        assert_eq!(adv.first_new_row, H - 10);
        assert_eq!(adv.quality, Quality::Exact);
        assert_eq!(adv.ignored_bottom, 0);
    }

    /// 左右边缘一直在变（滚动条 / 广告位），中间内容仍应匹配。
    #[test]
    fn side_offsets_are_ignored() {
        let prev = frame_with_rows(0, H);
        let mut cur = frame_with_rows(12, H);
        let side = side_offset(W);
        assert!(side > 0);
        for y in 0..H {
            for x in 0..side {
                for edge in [x, W - 1 - x] {
                    let o = (y * W + edge) * BYTES_PER_PIXEL;
                    cur[o] = (y * 7) as u8;
                    cur[o + 1] = 0;
                    cur[o + 2] = 255;
                }
            }
        }
        let adv = advance(view(&prev, H), view(&cur, H), W, H, MAX_TOTAL_HEIGHT).unwrap();
        assert_eq!(adv.new_rows, 12);
        assert_eq!(adv.quality, Quality::Exact);
    }

    /// 完全不相干的两帧 → 明确无匹配，不许硬凑。
    #[test]
    fn unrelated_frames_do_not_match() {
        let mut prev = frame_with_rows(0, H);
        let mut cur = frame_with_rows(0, H);
        for (i, b) in prev.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        for (i, b) in cur.iter_mut().enumerate() {
            *b = ((i * 13 + 7) % 251) as u8;
        }
        assert_eq!(
            advance(view(&prev, H), view(&cur, H), W, H, MAX_TOTAL_HEIGHT),
            Err(StitchError::NoMatch)
        );
    }

    /// 滚动没生效（两帧全等）→ 无匹配；由主循环结合 GetScrollInfo 判到底。
    #[test]
    fn identical_frames_have_no_advance() {
        let prev = frame_with_rows(5, H);
        let cur = prev.clone();
        assert!(frames_equal(view(&prev, H), view(&cur, H)));
        assert_eq!(
            advance(view(&prev, H), view(&cur, H), W, H, MAX_TOTAL_HEIGHT),
            Err(StitchError::NoMatch)
        );
    }

    /// 宽度变化 → 失败。
    #[test]
    fn width_change_fails() {
        let prev = frame_with_rows(0, H);
        let cur = prev.clone();
        assert_eq!(
            advance(
                view(&prev, H),
                Frame::new(W - 1, H, &cur),
                W,
                H,
                MAX_TOTAL_HEIGHT
            ),
            Err(StitchError::WidthChanged {
                previous: W,
                current: W - 1,
                result: W,
            })
        );
    }

    /// 帧高变化 → 失败。
    #[test]
    fn height_change_fails() {
        let prev = frame_with_rows(0, H);
        let cur = frame_with_rows(10, H + 1);
        assert_eq!(
            advance(view(&prev, H), view(&cur, H + 1), W, H, MAX_TOTAL_HEIGHT),
            Err(StitchError::HeightChanged {
                previous: H,
                current: H + 1,
            })
        );
    }

    /// 总高上限：剩余高度放不下这一帧 → 明确的失败原因。
    #[test]
    fn remaining_height_limit_fails() {
        let prev = frame_with_rows(0, H);
        let cur = frame_with_rows(10, H);
        assert_eq!(
            advance(view(&prev, H), view(&cur, H), W, 19_990, MAX_TOTAL_HEIGHT),
            Err(StitchError::FrameTallerThanRemaining {
                frame: H,
                remaining: 10,
            })
        );
    }

    /// 底部固定栏：最后 8 行两帧相同，应被忽略，不参与匹配也不拼进图。
    #[test]
    fn fixed_bottom_edge_is_ignored() {
        let bar = 8usize;
        let content_h = H - bar;
        let prev = frame_with_rows(0, content_h);
        let cur = frame_with_rows(10, content_h);
        let bar_row = vec![200u8; W * BYTES_PER_PIXEL];
        let mut prev_full = prev.clone();
        let mut cur_full = cur.clone();
        for _ in 0..bar {
            prev_full.extend_from_slice(&bar_row);
            cur_full.extend_from_slice(&bar_row);
        }
        let adv = advance(
            view(&prev_full, H),
            view(&cur_full, H),
            W,
            H,
            MAX_TOTAL_HEIGHT,
        )
        .unwrap();
        assert_eq!(adv.ignored_bottom, bar);
        assert_eq!(adv.new_rows, 10);
        assert_eq!(adv.first_new_row, content_h - 10);
        assert_eq!(adv.quality, Quality::Exact);
    }

    /// 没有整行精确匹配、但重叠区大部分一致 → 兜底命中并标降级。
    #[test]
    fn best_match_falls_back_with_lower_quality() {
        let prev = frame_with_rows(0, H);
        let mut cur = frame_with_rows(10, H);
        // 改 3 行（动态内容），仍低于 15% 的不一致阈值。
        // 必须改**中间列**：左右边缘本来就在忽略范围内。
        for y in [5usize, 17, 29] {
            let o = (y * W + W / 2) * BYTES_PER_PIXEL;
            cur[o] ^= 0xFF;
        }
        let adv = advance(view(&prev, H), view(&cur, H), W, H, MAX_TOTAL_HEIGHT).unwrap();
        assert_eq!(adv.new_rows, 10);
        assert_eq!(adv.quality, Quality::Fallback);
    }

    /// 阈值函数：`clamp(w/20, 50, w/3)`，窄窗口不许 panic。
    #[test]
    fn side_offset_clamps() {
        assert_eq!(side_offset(1920), 96);
        assert_eq!(side_offset(300), 50);
        assert_eq!(side_offset(100), 33);
        assert_eq!(side_offset(3), 1);
    }
}
