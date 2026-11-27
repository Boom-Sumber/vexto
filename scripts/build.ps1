$ErrorActionPreference = "Stop"
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$root = Split-Path $PSScriptRoot -Parent

Push-Location $root
try {
    Write-Host "==> cargo build --release" -ForegroundColor Cyan
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "构建失败" }

    # workspace 的 target 目录在根上（不是 src-tauri 下）
    # cargo 产物名是包名 lightbox；安装包里的 Vexto.exe 由 cargo tauri build 重命名（ADR-036 / 082）
    $exe = Join-Path $root "target\release\lightbox.exe"
    if (-not (Test-Path $exe)) { throw "找不到产物: $exe" }

    $size = [Math]::Round((Get-Item $exe).Length / 1MB, 2)
    Write-Host "==> 产物: $exe ($size MB)" -ForegroundColor Green
} finally {
    Pop-Location
}
