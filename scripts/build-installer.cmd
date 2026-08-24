@echo off
REM Wrapper for build-installer.ps1. Bypasses the execution policy for this
REM one process only, so unsigned local scripts run without relaxing the
REM machine's AllSigned policy.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0build-installer.ps1" %*
