//! 开机自启：只写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`。
//!
//! **不引 `tauri-plugin-autostart`**：它做的事就是写注册表，但会多一个依赖、
//! 多一套跨平台分支（macOS 的 LaunchAgent、Linux 的 .desktop），而我们只发 Windows。
//! `windows` crate 的注册表 API 本来就在依赖里（代理读 WinINET 用的同一个 feature）。
//!
//! 三条约定：
//! - 值名固定 `Vexto`（ADR-038；更名前是 `LightBox`），**不带版本号**：
//!   升级换了 exe 路径时才能原地覆盖；
//! - 值内容是**带引号的绝对路径**——Windows 会在第一个空格处切命令行，
//!   `C:\Program Files\Vexto\Vexto.exe` 不加引号会被当成 `C:\Program` + 参数；
//! - 只写 HKCU：不需要管理员权限，也不会影响这台机器上的其他用户。

use windows::core::HSTRING;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};

/// 当前用户的启动项。**不要**换成 HKLM：那需要管理员权限。
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// 注册表里的值名。任务管理器「启动」页会把它显示成 `Vexto`。
const VALUE_NAME: &str = "Vexto";

/// 要写进注册表的命令行：带引号的 exe 绝对路径。
///
/// 取 `current_exe()` 而不是 `argv[0]`：后者可能是相对路径或短名，
/// 登录时的工作目录和现在不一样，相对路径会失效。
pub fn exe_command() -> Result<String, String> {
    let p = std::env::current_exe().map_err(|e| format!("取程序路径失败：{e}"))?;
    Ok(format!("\"{}\"", p.display()))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 注册表里当前登记的命令行（没有就是 `None`）。
fn read_value() -> Option<String> {
    unsafe {
        let mut key = HKEY::default();
        let sub = HSTRING::from(RUN_KEY);
        if RegOpenKeyExW(HKEY_CURRENT_USER, &sub, Some(0), KEY_QUERY_VALUE, &mut key)
            != ERROR_SUCCESS
        {
            return None;
        }
        let name = HSTRING::from(VALUE_NAME);
        let mut buf = [0u16; 1024];
        let mut cb = (buf.len() * 2) as u32;
        let mut ty = REG_SZ;
        let r = RegQueryValueExW(
            key,
            &name,
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr() as *mut u8),
            Some(&mut cb),
        );
        let _ = RegCloseKey(key);
        if r != ERROR_SUCCESS {
            return None;
        }
        let len = (cb as usize / 2).saturating_sub(1).min(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// 注册表里登记的是不是**当前这个 exe**。
///
/// 只比字符串会漏掉「换了安装目录但还登记着旧路径」的情况——那种时候
/// 登录会启动一个不存在的程序，用户看到的是「自启失效」，实际是路径过期。
pub fn is_registered() -> bool {
    let Some(cur) = exe_command().ok() else {
        return false;
    };
    read_value()
        .map(|v| v.trim().eq_ignore_ascii_case(cur.trim()))
        .unwrap_or(false)
}

/// 打开（必要时创建）Run 键。
///
/// **删除值也要 `KEY_SET_VALUE`**：用只读权限打开再 `RegDeleteValueW` 会被
/// `ERROR_ACCESS_DENIED` 拒掉，表现就是「关掉自启了但下次开机还在」。
fn open_run_key(create: bool) -> Result<HKEY, String> {
    unsafe {
        let mut key = HKEY::default();
        let sub = HSTRING::from(RUN_KEY);
        let status = if create {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &sub,
                Some(0),
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut key,
                None,
            )
        } else {
            RegOpenKeyExW(HKEY_CURRENT_USER, &sub, Some(0), KEY_SET_VALUE, &mut key)
        };
        if status != ERROR_SUCCESS {
            return Err(format!("打开启动项注册表失败（错误码 {}）", status.0));
        }
        Ok(key)
    }
}

/// 打开 / 关闭开机自启。返回 `Err` 表示注册表没改成功（权限、被策略锁住等）。
pub fn apply(on: bool) -> Result<(), String> {
    if on {
        let cmd = exe_command()?;
        let key = open_run_key(true)?;
        let name = HSTRING::from(VALUE_NAME);
        let data = wide(&cmd);
        let bytes = unsafe {
            std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * std::mem::size_of::<u16>())
        };
        let r = unsafe {
            RegSetValueExW(
                key,
                &name,
                Some(0),
                REG_SZ,
                Some(bytes),
            )
        };
        unsafe {
            let _ = RegCloseKey(key);
        };
        if r != ERROR_SUCCESS {
            return Err(format!("写入启动项失败（错误码 {}）", r.0));
        }
        crate::dlog::dlog(&format!("autostart: 已注册 {cmd}"));
        Ok(())
    } else {
        let key = match open_run_key(false) {
            Ok(k) => k,
            // 键都不存在 = 本来就没登记过，关掉这件事已经成立
            Err(_) => return Ok(()),
        };
        let name = HSTRING::from(VALUE_NAME);
        let r = unsafe { RegDeleteValueW(key, &name) };
        unsafe {
            let _ = RegCloseKey(key);
        };
        // 值不存在（ERROR_FILE_NOT_FOUND）也当成功：目标状态就是「没有这个值」
        if r != ERROR_SUCCESS && r.0 != 2 {
            return Err(format!("删除启动项失败（错误码 {}）", r.0));
        }
        crate::dlog::dlog("autostart: 已移除启动项");
        Ok(())
    }
}

/// 启动时按配置自愈一次。
///
/// 两个方向都要修：配置说开、注册表却没登记（换了安装目录 / 被安全软件清掉）→ 补上；
/// 配置说关、注册表却还留着 → 删掉。配置是唯一真相来源，注册表只是它的投影。
///
/// 失败只记日志不阻塞启动：开机自启坏掉不该让整个程序起不来。
pub fn sync(wanted: bool) {
    let registered = is_registered();
    if wanted == registered {
        return;
    }
    if let Err(e) = apply(wanted) {
        crate::dlog::dlog(&format!("autostart: 自愈失败（wanted={wanted}）：{e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 命令行必须是带引号的绝对路径：不带引号时 Windows 会在第一个空格处切开。
    #[test]
    fn command_is_quoted_absolute_path() {
        let cmd = exe_command().unwrap();
        assert!(cmd.starts_with('"') && cmd.ends_with('"'), "{cmd}");
        let inner = &cmd[1..cmd.len() - 1];
        assert!(inner.to_ascii_lowercase().ends_with(".exe"), "{inner}");
    }

    /// 只写 HKCU 下的 Run 键，值名固定 —— 改这两项等于换了一套自启机制。
    #[test]
    fn target_is_the_hkcu_run_key() {
        assert_eq!(RUN_KEY, r"Software\Microsoft\Windows\CurrentVersion\Run");
        assert_eq!(VALUE_NAME, "Vexto");
    }

    /// UTF-16 + 结尾 NUL：REG_SZ 必须显式带终止符，否则系统会读串。
    #[test]
    fn wide_adds_terminating_nul() {
        let w = wide("a");
        assert_eq!(w, vec![97, 0]);
    }
}
