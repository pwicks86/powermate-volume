@echo off
REM Wrapper for dev-update.ps1. Bypasses the execution policy for this one
REM process only, so unsigned local scripts run without relaxing the
REM machine's AllSigned policy.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0dev-update.ps1" %*
