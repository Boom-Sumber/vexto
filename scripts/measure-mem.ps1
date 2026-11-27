# 内存三态实测。数字必须来自这个脚本，不许估算（AGENTS.md 铁律 4）。
#
# 用法：
#   powershell -File scripts\measure-mem.ps1                      # 开发态 lightbox.exe
#   powershell -File scripts\measure-mem.ps1 -ExeName Vexto        # 已安装版
#
# 判据见 docs/06-TESTING.md §6.6.2：
#   冷启动空闲     WS <= 15 MB，WebView2 = 0
#   用过一次关闭后 WS <= 10 MB，WebView2 = 0
#   弹窗打开       总计 <= 350 MB（硬上限 400）
#   主窗口打开     总计 <= 400 MB（硬上限 450）
#   主窗口口径在会话 15 按用户决定「调阈值」校正（原 300/350 是 S1b 拍的、无主窗口实测），
#   依据见 docs/06-TESTING.md §6.6.2 与 docs/11-DEV-PROGRESS.md 会话 11 / 14 的实测。

param([string]$ExeName = "lightbox")

function Get-LightboxMem {
    $p = Get-Process -Name $ExeName -ErrorAction SilentlyContinue
    if (-not $p) { return "未运行（进程 $ExeName 不存在）" }
    $ws   = [Math]::Round(($p | Measure-Object WorkingSet64 -Sum).Sum / 1MB, 1)
    $priv = [Math]::Round(($p | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB, 1)
    $wv2 = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" |
             Where-Object { $_.CommandLine -match $ExeName })
    $wv2Mem = 0
    foreach ($w in $wv2) {
        $proc = Get-Process -Id $w.ProcessId -ErrorAction SilentlyContinue
        if ($proc) { $wv2Mem += $proc.WorkingSet64 }
    }
    [PSCustomObject]@{
        主进程WS_MB   = $ws
        主进程Priv_MB = $priv
        # 注意：不能写 `Measure-Object Threads.Count -Sum`，那是对不存在的属性名求和，
        # 结果为空（PS 7 不报错，静默给空值）。必须先展开成数字再求和。
        线程数        = [int](($p | ForEach-Object { @($_.Threads).Count } | Measure-Object -Sum).Sum)
        句柄数        = [int](($p | Measure-Object HandleCount -Sum).Sum)
        WebView2个数  = $wv2.Count
        WebView2_MB   = [Math]::Round($wv2Mem / 1MB, 1)
        总计_MB       = [Math]::Round($ws + $wv2Mem / 1MB, 1)
    }
}

Get-LightboxMem | Format-List
