param(
    [ValidateRange(1, 999)]
    [int]$IdleMinutes = 10,
    [string]$TaskName = "AI Usage Dashboard Idle",
    [string]$ExePath,
    [switch]$Remove
)

# Registers a Scheduled Task that launches the dashboard in screensaver mode
# when the machine has been idle for $IdleMinutes. Adapted from the WinForms
# prototype to point at the Tauri build output.

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path

if ([string]::IsNullOrWhiteSpace($TaskName)) {
    throw "TaskName must not be empty."
}
$TaskName = $TaskName.Trim()

if ($Remove) {
    if ($PSBoundParameters.ContainsKey("ExePath") -or
        $PSBoundParameters.ContainsKey("IdleMinutes")) {
        throw "-ExePath and -IdleMinutes cannot be used with -Remove."
    }

    & schtasks.exe /Query /TN $TaskName *> $null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Idle task is not installed:"
        Write-Host "  $TaskName"
        return
    }

    & schtasks.exe /Delete /TN $TaskName /F | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "schtasks.exe failed to remove task '$TaskName' (exit code $LASTEXITCODE)"
    }

    Write-Host "Removed idle task:"
    Write-Host "  $TaskName"
    return
}

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $InstalledExe = $null
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        $InstalledExe = Join-Path $env:LOCALAPPDATA "AI Usage Dashboard\ai-usage-dashboard.exe"
    }
    $BuildExe = Join-Path $Root "src-tauri\target\release\ai-usage-dashboard.exe"

    if ($InstalledExe -and (Test-Path -LiteralPath $InstalledExe -PathType Leaf)) {
        $ExePath = $InstalledExe
    }
    elseif (Test-Path -LiteralPath $BuildExe -PathType Leaf) {
        $ExePath = $BuildExe
    }
    else {
        $CheckedPaths = @($InstalledExe, $BuildExe) | Where-Object { $_ }
        throw "AI Usage Dashboard executable not found. Checked: $($CheckedPaths -join '; ')"
    }
}

if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    throw "Executable not found: $ExePath"
}
if ([System.IO.Path]::GetExtension($ExePath) -ine ".exe") {
    throw "ExePath must point to an .exe file: $ExePath"
}
$ExePath = (Resolve-Path -LiteralPath $ExePath).Path

$Action = '"' + $ExePath + '" /s'
& schtasks.exe /Create /F /TN $TaskName /SC ONIDLE /I $IdleMinutes /TR $Action | Out-Host
if ($LASTEXITCODE -ne 0) {
    throw "schtasks.exe failed with exit code $LASTEXITCODE"
}

Write-Host "Installed idle task:"
Write-Host "  $TaskName"
Write-Host "  IdleMinutes=$IdleMinutes"
Write-Host "  Action=$Action"
Write-Host "Remove with:"
Write-Host "  .\install-idle-task.ps1 -Remove -TaskName `"$TaskName`""
