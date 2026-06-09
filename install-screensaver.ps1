param(
    [int]$TimeoutSeconds = 900,
    [string]$ExePath
)

# Installs the dashboard as the current user's screensaver.
#
# A Windows .scr is just an .exe that understands /s (run), /c (config), and
# /p <HWND> (preview). This app handles all three, so we copy the built exe to
# AiUsageDashboard.scr and register it.
#
# NOTE (experimental): as a WebView2 app, the Settings preview pane stays blank
# (the app intentionally no-ops on /p rather than rendering into the tiny
# preview window). The full-screen /s path is what actually matters. If you'd
# rather not use the .scr route at all, use install-idle-task.ps1 instead.

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path

if (-not $ExePath) {
    $ExePath = Join-Path $Root "src-tauri\target\release\ai-usage-dashboard.exe"
}

if (-not (Test-Path $ExePath)) {
    throw "Build first: npm run app:build  (expected exe at $ExePath)"
}

$ScrDir = Join-Path $Root "dist"
New-Item -ItemType Directory -Force -Path $ScrDir | Out-Null
$Scr = Join-Path $ScrDir "AiUsageDashboard.scr"
Copy-Item -Force $ExePath $Scr

$DesktopKey = "HKCU:\Control Panel\Desktop"
Set-ItemProperty -Path $DesktopKey -Name "SCRNSAVE.EXE" -Value $Scr
Set-ItemProperty -Path $DesktopKey -Name "ScreenSaveActive" -Value "1"
Set-ItemProperty -Path $DesktopKey -Name "ScreenSaveTimeOut" -Value ([string]$TimeoutSeconds)

Write-Host "Installed screensaver for current user:"
Write-Host "  $Scr"
Write-Host "  TimeoutSeconds=$TimeoutSeconds"
Write-Host "Open Settings > Personalization > Lock screen > Screen saver to confirm."
