# 生成源图标（PNG + ICO），不依赖任何 npm 包。
# 用 System.Drawing 画圆角渐变底 + 白色「译」字。
# 抄自 MVP（D:\78260\Documents\DSH\lightbox\gen_icons.ps1），只把输出路径改成相对本仓库。
#
# 为什么要自己画：tauri.conf.json 的 bundle.icon 里四个路径必须真实存在，
# 否则 tauri-build 构建期就失败；没有图标时托盘位置是空白的。

Add-Type -AssemblyName System.Drawing

$out = Join-Path (Split-Path $PSScriptRoot -Parent) 'src-tauri\icons'
New-Item -ItemType Directory -Force -Path $out | Out-Null

function New-IconPng {
    param([int]$Size, [string]$Path)
    $bmp = New-Object System.Drawing.Bitmap($Size, $Size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.Clear([System.Drawing.Color]::Transparent)

    $r = [int]($Size * 0.24)
    if ($r -lt 2) { $r = 2 }
    $d = $r * 2
    $gp = New-Object System.Drawing.Drawing2D.GraphicsPath
    $gp.AddArc(0, 0, $d, $d, 180, 90)
    $gp.AddArc($Size - $d, 0, $d, $d, 270, 90)
    $gp.AddArc($Size - $d, $Size - $d, $d, $d, 0, 90)
    $gp.AddArc(0, $Size - $d, $d, $d, 90, 90)
    $gp.CloseFigure()

    $c1 = [System.Drawing.Color]::FromArgb(255, 61, 132, 255)
    $c2 = [System.Drawing.Color]::FromArgb(255, 22, 56, 150)
    $p1 = New-Object System.Drawing.Point(0, 0)
    $p2 = New-Object System.Drawing.Point($Size, $Size)
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush($p1, $p2, $c1, $c2)
    $g.FillPath($brush, $gp)

    if ($Size -ge 24) {
        $fs = [float]($Size * 0.54)
        $font = New-Object System.Drawing.Font('Microsoft YaHei', $fs, [System.Drawing.FontStyle]::Bold, [System.Drawing.GraphicsUnit]::Pixel)
        $fmt = New-Object System.Drawing.StringFormat
        $fmt.Alignment = [System.Drawing.StringAlignment]::Center
        $fmt.LineAlignment = [System.Drawing.StringAlignment]::Center
        $sf = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
        $rect = New-Object System.Drawing.RectangleF(0, 0, [float]$Size, [float]$Size)
        $g.DrawString('译', $font, $sf, $rect, $fmt)
        $sf.Dispose(); $font.Dispose(); $fmt.Dispose()
    }

    $g.Dispose()
    $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    $gp.Dispose(); $brush.Dispose()
}

New-IconPng -Size 32  -Path (Join-Path $out '32x32.png')
New-IconPng -Size 128 -Path (Join-Path $out '128x128.png')
New-IconPng -Size 256 -Path (Join-Path $out '128x128@2x.png')
New-IconPng -Size 512 -Path (Join-Path $out 'icon.png')

# ICO：用 256x256 的位图句柄导出（Windows Vista+ 支持 PNG 压缩的 ICO 条目）
$bmpIco = New-Object System.Drawing.Bitmap((Join-Path $out '128x128@2x.png'))
$hicon = $bmpIco.GetHicon()
$icon = [System.Drawing.Icon]::FromHandle($hicon)
$fsIco = [System.IO.File]::Create((Join-Path $out 'icon.ico'))
$icon.Save($fsIco)
$fsIco.Close()
$icon.Dispose()
$bmpIco.Dispose()

Get-ChildItem $out | Select-Object Name, Length | Format-Table -AutoSize
