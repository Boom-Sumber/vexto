//! 启动 NSIS 安装器并退出当前进程。
//!
//! 参数来自 tauri-bundler 的 NSIS 模板（2.10.x）：
//! - `/UPDATE` 原地升级，不卸载旧版本、跳过快捷方式重建；
//! - `/P` 跳过所有向导页，只显示安装进度条，装完自动关闭；
//! - `/R` 安装完成后重新拉起 Vexto。
//!
//! 必须先退出进程再启动安装器：安装器要覆盖正在运行的 exe，文件锁不释放会失败。

use std::path::Path;
use std::sync::atomic::Ordering;

use tauri::AppHandle;

use crate::dlog;
use crate::QUITTING;

/// 延迟约 3 秒，留给 ExitRequested 里的 aria2 / 互传服务清理。
pub fn installer_cmdline(path: &str) -> String {
    format!(
        "ping -n 4 127.0.0.1 >NUL & start \"\" \"{}\" /UPDATE /P /R",
        path.replace('"', "")
    )
}

/// 启动安装器并在成功后退出。返回 Err 时进程不动，前端可以提示重试。
pub fn launch(app: &AppHandle, path: &Path) -> Result<(), String> {
    let raw = path.display().to_string();
    if raw.is_empty() {
        return Err("安装包路径为空".to_string());
    }
    let cmdline = installer_cmdline(&raw);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        match std::process::Command::new("cmd")
            .args(["/C", cmdline.as_str()])
            // CREATE_NO_WINDOW：别让 cmd 闪一个黑框
            .creation_flags(0x0800_0000)
            .spawn()
        {
            Ok(_) => {}
            Err(e) => {
                dlog::warn(&format!("update: 启动安装器失败：{e}"));
                return Err("无法启动安装程序，请稍后重试".to_string());
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = cmdline;
        return Err("仅支持 Windows".to_string());
    }

    // 顺序不能反：先置位再 exit，否则 prevent_exit 会挡掉这次退出。
    QUITTING.store(true, Ordering::SeqCst);
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline_has_upgrade_flags_and_quoted_path() {
        let line = installer_cmdline(r"C:\Users\a b\Vexto_0.3.0_x64-setup.exe");
        assert!(line.contains("\"C:\\Users\\a b\\Vexto_0.3.0_x64-setup.exe\""));
        assert!(line.contains("/UPDATE"));
        assert!(line.contains("/P"));
        assert!(line.contains("/R"));
        assert!(line.starts_with("ping -n 4"));
    }

    #[test]
    fn cmdline_strips_embedded_quotes() {
        let line = installer_cmdline(r#"C:\x"y\setup.exe"#);
        assert!(!line.contains("\"y"));
    }
}
