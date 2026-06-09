param(
    [int]$IdleMinutes = 10,
    [string]$TaskName = "AI Usage Dashboard Idle",
    [string]$ExePath
)

# Registers a Scheduled Task that launches the dashboard fullscreen when the
# machine has been idle for $IdleMinutes. Adapted from the WinForms prototype to
# point at the Tauri build output.

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path

if (-not $ExePath) {
    $ExePath = Join-Path $Root "src-tauri\target\release\ai-usage-dashboard.exe"
}

if (-not (Test-Path $ExePath)) {
    throw "Build first: npm run app:build  (expected exe at $ExePath)"
}

$Action = '"' + $ExePath + '" --fullscreen'
schtasks.exe /Create /F /TN $TaskName /SC ONIDLE /I $IdleMinutes /TR $Action | Out-Host

Write-Host "Installed idle task:"
Write-Host "  $TaskName"
Write-Host "  IdleMinutes=$IdleMinutes"
Write-Host "  Action=$Action"
Write-Host "Remove with:"
Write-Host "  schtasks.exe /Delete /TN `"$TaskName`" /F"
