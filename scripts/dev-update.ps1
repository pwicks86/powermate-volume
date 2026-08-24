<#
Fast dev loop: rebuild the release binary and hot-swap it into the
already-installed copy, without going through the full installer.

Requires PowerMate Volume to already be installed once (see
build-installer.ps1). Building runs unelevated; only the final copy into
Program Files prompts for elevation (one UAC prompt), so this is safe to
run from a normal PowerShell.
#>

param(
    # Internal: re-invocation used to perform just the elevated copy step.
    [switch]$ElevatedCopyPhase,
    [string]$InstalledExePath,
    [string]$BuiltExePath
)

$ErrorActionPreference = "Stop"

function Copy-AndRelaunch {
    param([string]$InstalledExe, [string]$BuiltExe)

    $wasRunning = $false
    $proc = Get-Process -Name "powermate-volume" -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $InstalledExe }
    if ($proc) {
        $wasRunning = $true
        Write-Host "Stopping running instance..."
        Stop-Process -InputObject $proc -Force
        $proc.WaitForExit(5000) | Out-Null
    }

    Write-Host "Copying new build into $InstalledExe ..."
    Copy-Item -Path $BuiltExe -Destination $InstalledExe -Force

    if ($wasRunning) {
        Write-Host "Relaunching..."
        Start-Process -FilePath $InstalledExe
    }
}

if ($ElevatedCopyPhase) {
    Copy-AndRelaunch -InstalledExe $InstalledExePath -BuiltExe $BuiltExePath
    exit 0
}

$repoRoot = Split-Path -Parent $PSScriptRoot
# Must match [Setup] AppId in installer\powermate-volume.iss.
$appId = "{F32A52FF-B73D-457E-9419-7D974C44CEF8}"
$uninstallKeyPaths = @(
    "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\${appId}_is1",
    "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\${appId}_is1"
)

$installLocation = $null
foreach ($path in $uninstallKeyPaths) {
    $key = Get-ItemProperty -Path $path -ErrorAction SilentlyContinue
    if ($key) {
        $installLocation = $key.InstallLocation
        break
    }
}

if (-not $installLocation) {
    Write-Error "PowerMate Volume doesn't appear to be installed. Run scripts\build-installer.ps1 first."
}

$installedExe = Join-Path $installLocation "powermate-volume.exe"
$builtExe = Join-Path $repoRoot "target\release\powermate-volume.exe"

Write-Host "Building release..."
cargo build --release --manifest-path (Join-Path $repoRoot "Cargo.toml")

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Copy-AndRelaunch -InstalledExe $installedExe -BuiltExe $builtExe
} else {
    Write-Host "Elevation required to update the installed copy -- prompting..."
    $argList = @(
        "-NoProfile", "-ExecutionPolicy", "Bypass",
        "-File", "`"$PSCommandPath`"",
        "-ElevatedCopyPhase",
        "-InstalledExePath", "`"$installedExe`"",
        "-BuiltExePath", "`"$builtExe`""
    )
    Start-Process -FilePath "powershell.exe" -ArgumentList $argList -Verb RunAs -Wait
}

Write-Host "Done."
