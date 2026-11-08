//! 工作集收缩。
//!
//! 注意语义：这不是「释放」内存，只是把不活跃页换出去变成可换出状态；
//! 物理内存有压力时系统还会读回来。真正的开销是重建 WebView2，不是这个。
//! 所以只在窗口销毁之后调，别在稳态里反复调。

use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;

/// 延迟一会儿再收缩：窗口刚销毁时 WebView2 子进程还在退出的路上，
/// 立刻调只会把还没回收的页又换出去一次，等 1.2s 更稳（09-MVP-BASELINE §9.2 实测值）。
pub fn trim_soon(delay_ms: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        // SAFETY: GetCurrentProcess 返回的伪句柄由系统保证有效，不需要也不允许关闭。
        unsafe {
            let _ = EmptyWorkingSet(GetCurrentProcess());
        }
        crate::dlog::dlog("mem::trim_soon: EmptyWorkingSet done");
    });
}
