# Builds the installer into target\installer. Run after `cargo build --release`:
#
#   powershell -File installer\build.ps1
#
# Needs Inno Setup 6 (`winget install JRSoftware.InnoSetup`); CI installs it with Chocolatey.
param([string]$Bin = '..\target\release')

$iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue).Source
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (!$iscc -and $env:CI) {
    choco install innosetup --no-progress -y
    $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
}
if (!$iscc) {
    Write-Error 'Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup'
    exit 1
}
& $iscc "/DBin=$Bin" "$PSScriptRoot\rsnip.iss"
exit $LASTEXITCODE
