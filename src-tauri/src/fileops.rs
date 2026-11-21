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
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FO_DELETE,
        SHFILEOPSTRUCTW,
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

    /// 另存为的移动语义：目标已存在要覆盖，源文件消失。
    #[test]
    fn move_file_overwrites_and_removes_source() {
        let dir = std::env::temp_dir().join(format!("vexto-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.png");
        let dst = dir.join("b.png");
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dst, b"old").unwrap();
        super::move_file(&src, &dst).unwrap();
        assert!(!src.exists(), "源文件应被移走");
        assert_eq!(std::fs::read(&dst).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

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

/// 系统原生「打开文件」对话框（多选）。S4b 发送文件用。
///
/// 走 `GetOpenFileNameW` + `OFN_ALLOWMULTISELECT|OFN_EXPLORER`：不引
/// tauri-plugin-dialog（少一个依赖），`windows` crate 本来就在树上。
/// 不设 owner 句柄——从工作线程调用也安全（模态到桌面）。
pub fn pick_files() -> Result<Vec<String>, String> {
    use windows::core::{PCWSTR, PWSTR};
    // windows 0.62 里 commdlg 在 `UI::Controls::Dialogs`（不在 Shell）。
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_ALLOWMULTISELECT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };

    // 多选结果是「目录\0文件\0文件\0\0」，64K 缓冲够几千个文件。
    let mut buffer = vec![0u16; 64 * 1024];
    // 过滤串：显示名\0模式\0，以两个 \0 结束。
    let filter: Vec<u16> = "所有文件\0*.*\0\0".encode_utf16().collect();

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        Flags: OFN_ALLOWMULTISELECT | OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
        ..Default::default()
    };

    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if !ok.as_bool() {
        // 用户取消也是常态，不报错——返回空列表让调用方当「没选」处理。
        return Ok(Vec::new());
    }

    // 解析双 \0 结尾的缓冲：单选 = 一个完整路径；多选 = 目录 + 文件名列表。
    let strings: Vec<String> = buffer
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect();
    match strings.len() {
        0 => Ok(Vec::new()),
        1 => Ok(strings),
        _ => {
            let dir = &strings[0];
            Ok(strings[1..]
                .iter()
                .map(|f| {
                    let mut p = std::path::PathBuf::from(dir);
                    p.push(f);
                    p.to_string_lossy().to_string()
                })
                .collect())
        }
    }
}

/// 系统原生「保存文件」对话框。返回 `None` 表示用户取消。
///
/// 走 `GetSaveFileNameW`（与 `pick_files` 同一个 commdlg 模块，不新增依赖）。
/// `default_path` 同时给出初始目录与文件名；扩展名固定 `.png`。
pub fn pick_save_png(default_path: &Path) -> Result<Option<std::path::PathBuf>, String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetSaveFileNameW, OFN_EXPLORER, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut buffer = vec![0u16; 32 * 1024];
    let init: Vec<u16> = default_path.to_string_lossy().encode_utf16().collect();
    if init.len() >= buffer.len() {
        return Err("保存路径太长".into());
    }
    buffer[..init.len()].copy_from_slice(&init);

    let filter: Vec<u16> = "PNG 图片\0*.png\0所有文件\0*.*\0\0"
        .encode_utf16()
        .collect();
    let ext: Vec<u16> = "png\0".encode_utf16().collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFile: PWSTR(buffer.as_mut_ptr()),
        nMaxFile: buffer.len() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrDefExt: PCWSTR(ext.as_ptr()),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_EXPLORER | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    let ok = unsafe { GetSaveFileNameW(&mut ofn) };
    if !ok.as_bool() {
        let err = unsafe { CommDlgExtendedError() };
        if err.0 != 0 {
            return Err(format!("打开保存对话框失败（错误码 {}）", err.0));
        }
        return Ok(None);
    }
    let n = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Ok(Some(std::path::PathBuf::from(String::from_utf16_lossy(
        &buffer[..n],
    ))))
}

/// 系统原生「选择文件夹」对话框。返回 `None` 表示用户取消。
///
/// 用 `SHBrowseForFolderW`（`windows` crate 已在树上，不引 tauri-plugin-dialog）。
/// `BIF_NEWDIALOGSTYLE` 需要当前线程初始化 COM；`RPC_E_CHANGED_MODE` 时
/// 说明调用线程已经是别的套间模型，照样继续，只是不做配对的反初始化。
pub fn pick_folder() -> Result<Option<std::path::PathBuf>, String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BIF_EDITBOX, BIF_NEWDIALOGSTYLE,
        BIF_RETURNONLYFSDIRS, BROWSEINFOW,
    };

    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let initialized = hr.is_ok();
    let title: Vec<u16> = "选择 clone 保存位置\0".encode_utf16().collect();
    let mut display = [0u16; 260];
    let bi = BROWSEINFOW {
        pszDisplayName: PWSTR(display.as_mut_ptr()),
        lpszTitle: PCWSTR(title.as_ptr()),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX,
        ..Default::default()
    };

    let pidl = unsafe { SHBrowseForFolderW(&bi) };
    if pidl.is_null() {
        if initialized {
            unsafe { CoUninitialize() };
        }
        return Ok(None);
    }
    let mut buf = [0u16; 260];
    let ok = unsafe { SHGetPathFromIDListW(pidl, &mut buf) };
    unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
    if initialized {
        unsafe { CoUninitialize() };
    }
    if !ok.as_bool() {
        return Err("无法读取所选文件夹路径".into());
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    if s.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(std::path::PathBuf::from(s)))
    }
}

/// 把 `src` 移到 `dst`（跨盘时复制后删源）。目标已存在会被覆盖
/// —— 保存对话框的 `OFN_OVERWRITEPROMPT` 已经问过用户。
pub fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建保存目录失败：{e}"))?;
    }
    std::fs::copy(src, dst).map_err(|e| format!("保存文件失败（{}）：{e}", dst.display()))?;
    let _ = std::fs::remove_file(src);
    Ok(())
}
