# WinGet 0.8.6

Did not bump the product version again or retag. v0.8.6 was already published.

## SHA-256

Installer: `AI-Usage-Dashboard_0.8.6_x64-setup.exe`  
Source: GitHub Release `v0.8.6` plus `AI-Usage-Dashboard_0.8.6_SHA256SUMS.txt`

```
49A6B655766D3EC35222F609F50DD8581EC3D33A6FEB125B8542BAD2AB21B2D0
```

Local `shasum -a 256` matched the SUMS line for `x64-setup.exe`. Manifest uses that value (uppercase, no spaces). Size 7,488,390 bytes.

## Product repo

- Commit: `ee8bae6` `chore: add WinGet 0.8.6 manifests` on `origin/main`
- Path: `packaging/winget/0.8.6/` (installer / locale.en-US / version, schema 1.12.0)
- `ReleaseDate: 2026-09-25` (GitHub Release `published_at`)
- InstallerUrl: `https://github.com/neyham/ai-usage-dashboard/releases/download/v0.8.6/AI-Usage-Dashboard_0.8.6_x64-setup.exe`
- LicenseUrl / ReleaseNotesUrl point at `v0.8.6`
- Publisher / Scope / ProductCode same as 0.8.5

## winget-pkgs PR

- https://github.com/microsoft/winget-pkgs/pull/440901
- Title: `New version: neyham.AIUsageDashboard version 0.8.6`
- Head: `neyham:winget-ai-usage-dashboard-0.8.6` → `microsoft/winget-pkgs` `master`
- State: **OPEN** (not merged)

## winget validate

Not run. `wingetvalidate` and `wingetcreate` are not installed on this Mac. Manifests are a field-for-field 0.8.5 copy with version, date, URL, SHA, and tag links updated.
