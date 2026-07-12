param(
    [ValidateRange(60, 86400)]
    [int]$TimeoutSeconds = 900,
    [string]$ExePath,
    [switch]$Remove
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
$DesktopKey = "HKCU:\Control Panel\Desktop"
$BackupKey = "HKCU:\Software\AiUsageDashboard\ScreenSaverBackup"
$DesktopValueNames = @("SCRNSAVE.EXE", "ScreenSaveActive", "ScreenSaveTimeOut")

if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
    throw "LOCALAPPDATA is not available for the current user."
}

$ScrDir = Join-Path $env:LOCALAPPDATA "AiUsageDashboard"
$Scr = Join-Path $ScrDir "AiUsageDashboard.scr"

function Get-RegistryValue {
    param(
        [string]$LiteralPath,
        [string]$Name
    )

    $Item = Get-ItemProperty -LiteralPath $LiteralPath -Name $Name -ErrorAction SilentlyContinue
    if ($null -eq $Item) {
        return $null
    }
    $Property = $Item.PSObject.Properties[$Name]
    if ($null -eq $Property) {
        return $null
    }
    return $Property.Value
}

function Test-SamePath {
    param(
        [AllowNull()]
        [string]$Left,
        [string]$Right
    )

    if ([string]::IsNullOrWhiteSpace($Left)) {
        return $false
    }

    $ExpandedLeft = [Environment]::ExpandEnvironmentVariables($Left.Trim().Trim('"'))
    try {
        $ExpandedLeft = [System.IO.Path]::GetFullPath($ExpandedLeft)
    }
    catch {
        # Fall back to the literal registry value when it is not a valid path.
    }

    try {
        $Right = [System.IO.Path]::GetFullPath($Right)
    }
    catch {
        return $false
    }

    return [string]::Equals(
        $ExpandedLeft.TrimEnd('\'),
        $Right.TrimEnd('\'),
        [StringComparison]::OrdinalIgnoreCase
    )
}

function Save-DesktopValue {
    param([string]$Name)

    $Item = Get-ItemProperty -LiteralPath $DesktopKey -Name $Name -ErrorAction SilentlyContinue
    $Property = if ($null -ne $Item) { $Item.PSObject.Properties[$Name] } else { $null }
    $Present = [int]($null -ne $Property)
    New-ItemProperty -LiteralPath $BackupKey -Name ($Name + "Present") `
        -PropertyType DWord -Value $Present -Force | Out-Null

    if ($Present -eq 1) {
        New-ItemProperty -LiteralPath $BackupKey -Name ($Name + "Value") `
            -PropertyType String -Value ([string]$Property.Value) -Force | Out-Null
    }
}

function Restore-DesktopValue {
    param([string]$Name)

    $Present = Get-RegistryValue -LiteralPath $BackupKey -Name ($Name + "Present")
    if ($Present -eq 1) {
        $Value = Get-RegistryValue -LiteralPath $BackupKey -Name ($Name + "Value")
        New-ItemProperty -LiteralPath $DesktopKey -Name $Name -PropertyType String `
            -Value ([string]$Value) -Force | Out-Null
    }
    else {
        Remove-ItemProperty -LiteralPath $DesktopKey -Name $Name -ErrorAction SilentlyContinue
    }
}

if ($Remove) {
    if ($PSBoundParameters.ContainsKey("ExePath") -or
        $PSBoundParameters.ContainsKey("TimeoutSeconds")) {
        throw "-ExePath and -TimeoutSeconds cannot be used with -Remove."
    }

    $CurrentScreenSaver = Get-RegistryValue -LiteralPath $DesktopKey -Name "SCRNSAVE.EXE"
    $OwnsCurrentSelection = Test-SamePath -Left $CurrentScreenSaver -Right $Scr
    $BackupVersion = if (Test-Path -LiteralPath $BackupKey) {
        Get-RegistryValue -LiteralPath $BackupKey -Name "BackupVersion"
    }
    else {
        $null
    }
    $BackupScreenSaver = if ($BackupVersion -eq 1 -and
        (Get-RegistryValue -LiteralPath $BackupKey -Name "SCRNSAVE.EXEPresent") -eq 1) {
        Get-RegistryValue -LiteralPath $BackupKey -Name "SCRNSAVE.EXEValue"
    }
    else {
        $null
    }
    $BackupPointsToOurs = Test-SamePath -Left $BackupScreenSaver -Right $Scr

    if ($OwnsCurrentSelection -and $BackupVersion -eq 1 -and -not $BackupPointsToOurs) {
        foreach ($Name in $DesktopValueNames) {
            Restore-DesktopValue -Name $Name
        }
        Write-Host "Restored the previous screen saver settings."
    }
    elseif ($OwnsCurrentSelection) {
        # Older versions did not create a backup. Avoid leaving a missing .scr
        # selected, but do not guess the user's former timeout value.
        Remove-ItemProperty -LiteralPath $DesktopKey -Name "SCRNSAVE.EXE" `
            -ErrorAction SilentlyContinue
        New-ItemProperty -LiteralPath $DesktopKey -Name "ScreenSaveActive" `
            -PropertyType String -Value "0" -Force | Out-Null
        Write-Warning "No usable previous screen saver backup was found; screen saver activation was disabled."
    }
    else {
        Write-Host "Current screen saver belongs to another application; its settings were preserved."
    }

    if (Test-Path -LiteralPath $Scr -PathType Leaf) {
        Remove-Item -LiteralPath $Scr -Force
    }
    if (Test-Path -LiteralPath $BackupKey) {
        Remove-Item -LiteralPath $BackupKey -Force
    }

    Write-Host "Removed AI Usage Dashboard screen saver:"
    Write-Host "  $Scr"
    return
}

if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $Root "src-tauri\target\release\ai-usage-dashboard.exe"
}

if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
    throw "Executable not found: $ExePath"
}
if ([System.IO.Path]::GetExtension($ExePath) -ine ".exe") {
    throw "ExePath must point to an .exe file: $ExePath"
}
$ExePath = (Resolve-Path -LiteralPath $ExePath).Path

New-Item -ItemType Directory -Force -Path $ScrDir | Out-Null
Copy-Item -LiteralPath $ExePath -Destination $Scr -Force

$BackupVersion = if (Test-Path -LiteralPath $BackupKey) {
    Get-RegistryValue -LiteralPath $BackupKey -Name "BackupVersion"
}
else {
    $null
}
if ($BackupVersion -ne 1) {
    if (Test-Path -LiteralPath $BackupKey) {
        # An incomplete key can only have been written before desktop settings
        # were changed; BackupVersion is committed before those settings change.
        Remove-Item -LiteralPath $BackupKey -Force
    }
    New-Item -Path $BackupKey -Force | Out-Null
    foreach ($Name in $DesktopValueNames) {
        Save-DesktopValue -Name $Name
    }
    New-ItemProperty -LiteralPath $BackupKey -Name "BackupVersion" `
        -PropertyType DWord -Value 1 -Force | Out-Null
}

New-ItemProperty -LiteralPath $DesktopKey -Name "SCRNSAVE.EXE" -PropertyType String `
    -Value $Scr -Force | Out-Null
New-ItemProperty -LiteralPath $DesktopKey -Name "ScreenSaveActive" -PropertyType String `
    -Value "1" -Force | Out-Null
New-ItemProperty -LiteralPath $DesktopKey -Name "ScreenSaveTimeOut" -PropertyType String `
    -Value ([string]$TimeoutSeconds) -Force | Out-Null

Write-Host "Installed screensaver for current user:"
Write-Host "  $Scr"
Write-Host "  TimeoutSeconds=$TimeoutSeconds"
Write-Host "Open Settings > Personalization > Lock screen > Screen saver to confirm."
Write-Host "Remove with:"
Write-Host "  .\install-screensaver.ps1 -Remove"
