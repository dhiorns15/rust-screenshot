# Installs RSnip with the built setup into a temp folder, upgrades it while it is running, then
# uninstalls it, checking the files and the start-at-sign-in entry along the way.
#
# CI only: it closes any running RSnip and replaces the start-at-sign-in entry.

$ErrorActionPreference = 'Stop'
if (!$env:CI) { throw 'Run this on CI only: it closes RSnip and changes its start-at-sign-in entry.' }

$setup = (Get-ChildItem "$PSScriptRoot\..\target\installer\*-setup.exe" | Sort-Object LastWriteTime | Select-Object -Last 1).FullName
$dir = Join-Path ([IO.Path]::GetTempPath()) 'rsnip-setup-test'
$exe = "$dir\rsnip.exe"
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'

function Install([string[]]$Extra = @()) {
    $flags = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=`"$dir`"", '/TASKS=startup') + $Extra
    # Wait for setup alone: -Wait would also wait for the RSnip it restarts.
    $p = Start-Process $setup -ArgumentList $flags -PassThru
    $null = $p.Handle # keeps the exit code readable
    $p.WaitForExit()
    if ($p.ExitCode) { throw "Setup exited with code $($p.ExitCode)" }
}

# Nothing else runs RSnip on CI, so any rsnip process is the installed one.
function Running {
    Get-Process rsnip -ErrorAction SilentlyContinue | Select-Object -First 1
}

Install
if (!(Test-Path $exe)) { throw 'rsnip.exe was not installed' }
if ((Get-ItemProperty $runKey).RSnip -notlike "*$exe*") { throw 'The start-at-sign-in entry is missing' }
if (Running) { throw 'A silent install started RSnip without /relaunch=yes' }

# Upgrading over a running copy closes it first; RSnip's updater also asks for a restart.
$app = Start-Process $exe -PassThru
Start-Sleep 2
Install '/relaunch=yes'
if (!$app.HasExited) { throw 'Setup left RSnip running' }
Start-Sleep 2
$app = Running
if (!$app) { throw 'RSnip was not restarted after the update' }
Start-Process "$dir\unins000.exe" -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES' -Wait
# The uninstaller reruns itself from a temp copy and may return early, so wait for it.
for ($i = 0; $i -lt 60 -and (Test-Path $exe); $i++) { Start-Sleep 1 }
if (Test-Path $exe) { throw 'rsnip.exe was not removed' }
if (!$app.HasExited) { throw 'Uninstalling left RSnip running' }
if ($null -ne (Get-ItemProperty $runKey).RSnip) { throw 'The start-at-sign-in entry was left behind' }
'Install, upgrade and uninstall OK'
