<#
Full release workflow: rebuild, recompile the Inno Setup installer, and run
it silently to install/upgrade. This exercises the real install path
(registry, Start Menu shortcuts, autostart task, uninstall cleanup) the way
an end user's setup run would -- slower than dev-update.ps1, but the one to
use when you actually want to cut a real build.

The compiled Setup.exe has its own admin-required manifest, so it prompts
for elevation (UAC) on its own when it runs -- no need to launch this script
elevated yourself.
#>

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$installerDir = Join-Path $repoRoot "installer"

$iscc = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source
if (-not $iscc) {
    $candidates = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
    )
    $iscc = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $iscc) {
    Write-Error "ISCC.exe (Inno Setup Compiler) not found. Install it: winget install JRSoftware.InnoSetup"
}

Write-Host "Building release..."
cargo build --release --manifest-path (Join-Path $repoRoot "Cargo.toml")

Write-Host "Compiling installer..."
& $iscc (Join-Path $installerDir "powermate-volume.iss")
if ($LASTEXITCODE -ne 0) {
    Write-Error "ISCC failed with exit code $LASTEXITCODE"
}

$setupExe = Join-Path $installerDir "output\PowerMateVolumeSetup.exe"

Write-Host "Running installer (silent)..."
Start-Process -FilePath $setupExe -ArgumentList "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART" -Wait

Write-Host "Done."
