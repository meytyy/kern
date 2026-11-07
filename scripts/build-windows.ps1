# Reproducible KERN build on Windows 11 x64 (PowerShell 5.1 or 7).
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
# Needs: Rust (rustup, stable, x86_64-pc-windows-msvc), Visual Studio 2022 Build Tools
# with "Desktop development with C++" (MSVC v143 + Windows 11 SDK). The SDK provides
# fxc.exe for the GPUI shaders and rc.exe for the icon. Inno Setup 6 is optional.
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
cargo build --release --locked --target x86_64-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$out = "dist\windows\KERN"
New-Item -ItemType Directory -Force -Path $out | Out-Null
Copy-Item "target\x86_64-pc-windows-msvc\release\kern.exe" "$out\KERN.exe" -Force
Copy-Item "README.md" "$out\README.md" -Force

$zip = "dist\windows\KERN-$version-x64-portable.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path "$out\*" -DestinationPath $zip
Write-Host "Portable: $zip"

$iscc = @(
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($iscc) {
    & $iscc "/DAppVersion=$version" "installer\KERN.iss"
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
    Write-Host "Installer: dist\windows\KERN-$version-x64-setup.exe"
} else {
    Write-Host "Inno Setup 6 not found: installer skipped (winget install JRSoftware.InnoSetup)."
}
Write-Host "Unsigned build: Windows SmartScreen may ask once before the first start."
