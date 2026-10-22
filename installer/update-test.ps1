# Installs the release before this one, runs `rsnip --update`, and checks that it updates itself to
# this release and restarts. Runs on CI once a release is published.

$ErrorActionPreference = 'Stop'
if (!$env:CI) { throw 'Run this on CI only: it installs RSnip and closes any running copy.' }

$current = $env:GITHUB_REF_NAME
$tags = @(gh release list --exclude-drafts --exclude-pre-releases --limit 100 --json tagName --jq '.[].tagName')
$index = [array]::IndexOf($tags, $current)
$previous = if ($index -ge 0 -and $index + 1 -lt $tags.Count) { $tags[$index + 1] }
# The updater arrived in 0.3.0.
if (!$previous -or [version]$previous.TrimStart('v') -lt [version]'0.3.0') {
    "No earlier release with the updater before $current; nothing to test"
    exit 0
}

$dir = Join-Path $env:RUNNER_TEMP 'rsnip-update-test'
gh release download $previous --pattern '*-setup.exe' --dir $dir
$setup = (Get-ChildItem "$dir\*-setup.exe").FullName
$p = Start-Process $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
if ($p.ExitCode) { throw "Setup for $previous exited with code $($p.ExitCode)" }

$exe = "$env:LOCALAPPDATA\Programs\RSnip\rsnip.exe"
Start-Process $exe -ArgumentList '--update'
$want = $current.TrimStart('v')
for ($i = 0; $i -lt 180; $i++) {
    Start-Sleep 1
    $version = (Get-Item $exe -ErrorAction SilentlyContinue).VersionInfo.ProductVersion
    if ($version -eq $want -and (Get-Process rsnip -ErrorAction SilentlyContinue)) {
        "Updated $previous to $current, and RSnip restarted"
        exit 0
    }
}
throw "After 3 minutes the installed version is $version and RSnip is $(if (Get-Process rsnip -ErrorAction SilentlyContinue) { 'running' } else { 'not running' })"
