# Captures the whole screen with the built rsnip.exe, as `rsnip --full` does for a user, and checks
# that a screenshot of the screen's size lands in the folder: the tray app starting, the capture,
# the save and the library's folder, end to end on a real Windows desktop.
#
# CI only: it writes RSnip's settings in the registry and closes any running RSnip.

$ErrorActionPreference = 'Stop'
if (!$env:CI) { throw 'Run this on CI only: it changes RSnip''s settings and closes RSnip.' }

$exe = Join-Path $PSScriptRoot '..\target\debug\rsnip.exe'
if (!(Test-Path $exe)) { throw "No ${exe}: run cargo build first" }
$dir = Join-Path ([IO.Path]::GetTempPath()) 'rsnip-capture-test'
if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
$null = New-Item -ItemType Directory $dir

# Save there, quietly, without opening the editor (see src/settings.rs for the names).
$key = 'HKCU:\Software\RSnip'
$null = New-Item -Path $key -Force
Set-ItemProperty $key -Name SaveFolder -Value $dir -Type String
Set-ItemProperty $key -Name Notify -Value 0 -Type DWord
Set-ItemProperty $key -Name OpenEditor -Value 0 -Type DWord

# Nothing else runs RSnip on CI; a running one would take the capture instead of the built exe.
Get-Process rsnip -ErrorAction SilentlyContinue | Stop-Process -Force
$app = Start-Process $exe -ArgumentList '--full' -PassThru
try {
    $deadline = (Get-Date).AddSeconds(60)
    do {
        Start-Sleep 1
        # The file is renamed into place once it is written whole, so the first one seen is complete.
        $shot = Get-ChildItem $dir -Filter 'Screenshot*.png' -ErrorAction SilentlyContinue | Select-Object -First 1
    } while (!$shot -and (Get-Date) -lt $deadline)
    if (!$shot) { throw "No screenshot was saved in $dir within 60 seconds" }
    if ($app.HasExited) { throw "RSnip exited (code $($app.ExitCode)) instead of staying in the tray" }

    Add-Type -AssemblyName System.Drawing
    Add-Type -AssemblyName System.Windows.Forms
    $screen = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $image = [System.Drawing.Image]::FromFile($shot.FullName)
    try {
        if ($image.Width -ne $screen.Width -or $image.Height -ne $screen.Height) {
            throw "$($shot.Name) is $($image.Width)x$($image.Height), not the screen's $($screen.Width)x$($screen.Height)"
        }
    } finally {
        $image.Dispose()
    }
    Write-Host "Captured $($shot.Name): $($screen.Width)x$($screen.Height), $($shot.Length) bytes"
} finally {
    if (!$app.HasExited) { Stop-Process -Id $app.Id -Force }
    Remove-ItemProperty $key -Name SaveFolder, Notify, OpenEditor -ErrorAction SilentlyContinue
}
