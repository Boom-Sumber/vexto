//! 删除文件：移入回收站（默认）或彻底删除。
//!
//! 回收站走 `SHFileOperationW` + `FOF_ALLOWUNDO` —— 这是 Windows 上唯一
//! 「按资源管理器的语义删除」的官方入口；`std::fs::remove_file` 是直接抹掉，
//! 用户删错了没有任何补救。
//!
//! **调用方负责先确认**：这里不做二次询问，也没有撤销。

use std::path::Path;

/// 删一个文件或目录。
///
/// - `recycle = true`：进回收站（`FOF_ALLOWUNDO`），可撤销；
/// - `recycle = false`：直接删，不可恢复。
///
/// 路径已经不存在时返回 `Ok(())`：目标状态（这个文件没了）已经达成，
/// 报错只会让「删任务」这个动作莫名其妙地失败。
pub fn delete(path: &str, recycle: bool) -> Result<(), String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("没有文件路径".into());
    }
    let target = Path::new(p);
    if !target.exists() {
        return Ok(());
    }
    if !recycle {
        return if target.is_dir() {
            std::fs::remove_dir_all(target).map_err(|e| format!("删除目录失败：{e}"))
        } else {
            std::fs::remove_file(target).map_err(|e| format!("删除文件失败：{e}"))
        };
    }
    to_recycle_bin(p)
}

#[cfg(windows)]
fn to_recycle_bin(path: &str) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, SHFILEOPSTRUCTW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOERRORUI,
        FOF_NOCONFIRMATION, FOF_SILENT,
    };

    // `pFrom` 要求「若干个以 NUL 结尾的路径 + 末尾再一个 NUL」，少一个就会
    // 读到缓冲区外面去。这里只有一个路径，所以是「路径 + 双 NUL」。
    let mut from: Vec<u16> = path.encode_utf16().collect();
    from.push(0);
    from.push(0);

    // 三个「别弹窗」的标志都必须带：不带的话资源管理器会在后台弹一个
    // 确认框，而我们在托盘里，用户根本看不到那个框。
    //
    // 常量是 `FILEOPERATION_FLAGS(u32)`，结构体字段却是 `u16`——必须显式收窄。
    let flags = (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16;
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: flags,
        ..Default::default()
    };
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 {
        return Err(format!("移入回收站失败（错误码 {rc}）"));
    }
    if op.fAnyOperationsAborted.as_bool() {
        return Err("已取消删除".into());
    }
    Ok(())
}

#[cfg(not(windows))]
fn to_recycle_bin(path: &str) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("删除文件失败：{e}"))
}

/// 在资源管理器里打开所在目录并选中文件。
///
/// 用 `raw_arg` 而不是 `arg`：`arg` 在路径含空格时会自动加引号，而
/// `explorer.exe` 的 `/select,` 参数一旦被引号包住就整串当成路径，
/// 结果是打开「文档」而不是选中文件（台账会话 15 的 A/B 实测）。
pub fn reveal(path: &str) -> Result<(), String> {
    let p = path.trim().replace('/', "\\");
    if p.is_empty() {
        return Err("没有文件路径".into());
    }
    let mut cmd = std::process::Command::new("explorer.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // 文件还在就选中它；已经不在了就打开目录（explorer 会用自己的规则兜底）
        if std::path::Path::new(&p).exists() {
            cmd.raw_arg(format!("/select,{p}"));
        } else {
            if let Some(dir) = std::path::Path::new(&p).parent() {
                cmd.raw_arg(dir.display().to_string());
            } else {
                return Err("路径没有上级目录".into());
            }
        }
    }
    #[cfg(not(windows))]
    cmd.arg(&p);

    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("打开资源管理器失败：{e}"))
}

#[cfg(test)]
mod tests {
    use super::delete;

    #[test]
    fn empty_path_is_rejected() {
        assert!(delete("", true).is_err());
        assert!(delete("   ", true).is_err());
    }

    /// 路径不存在时算成功：调用方是「删任务顺带删文件」，
    /// 文件早就被用户手工删掉了不该让删任务失败。
    #[test]
    fn missing_file_is_not_an_error() {
        assert!(delete(r"C:\definitely\not\here\nope.bin", true).is_ok());
        assert!(delete(r"C:\definitely\not\here\nope.bin", false).is_ok());
    }
}
