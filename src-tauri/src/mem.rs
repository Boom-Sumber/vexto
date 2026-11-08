//! 工作集收缩。
//!
//! 注意语义：这不是「释放」内存，只是把不活跃页换出去变成可换出状态；
//! 物理内存有压力时系统还会读回来。真正的开销是重建 WebView2，不是这个。
//! 所以只在窗口销毁之后调，别在稳态里反复调。

use windows::Win32::System::ProcessStatus::EmptyWorkingSet;
use windows::Win32::System::Threading::GetCurrentProcess;

/// S0 没有任何窗口会被销毁，因此这个函数暂时没人调用；
/// S1 里「弹窗 destroy 后 trim」会用上它。删掉 `allow` 要等真正接上调用点。
#[allow(dead_code)]
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
