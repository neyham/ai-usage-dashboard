# WinGet 0.8.4

Did not bump the product version, retag, redo the GitHub Release, or edit `cursor.rs`.

## SHA-256

Installer: `AI-Usage-Dashboard_0.8.4_x64-setup.exe`  
Source: GitHub Release `v0.8.4` plus `AI-Usage-Dashboard_0.8.4_SHA256SUMS.txt`

```
D507014891BCDD8701B44E725D1C9ED8FB442D75FA29192D44AE0F2969235D53
```

Local `shasum -a 256` of the downloaded NSIS file matched the SUMS line for `x64-setup.exe` (not the MSI `9E2F...` line). Manifest uses that value (uppercase, no spaces).

## Product repo

- Commit: `chore: add WinGet 0.8.4 manifests` on `origin/main`
- Path: `packaging/winget/0.8.4/` (installer / locale.en-US / version, schema 1.12.0)
- `ReleaseDate: 2026-08-31`
- InstallerUrl: `https://github.com/neyham/ai-usage-dashboard/releases/download/v0.8.4/AI-Usage-Dashboard_0.8.4_x64-setup.exe`
- LicenseUrl / ReleaseNotesUrl point at `v0.8.4`
- Publisher / Scope / ProductCode same as 0.8.2 (`InstallerType: nullsoft`, `Scope: user`, `ProductCode: AI Usage Dashboard`, `Publisher: neyha` in AppsAndFeaturesEntries)
- Older untracked REVIEW files were not committed

## winget-pkgs PR

- https://github.com/microsoft/winget-pkgs/pull/426827
- Title: `New version: neyham.AIUsageDashboard version 0.8.4`
- Head: `neyham:winget-ai-usage-dashboard-0.8.4` → `microsoft/winget-pkgs` `master`
- State: **OPEN** (not merged)

## winget validate

Not run. `wingetvalidate` and `wingetcreate` are not installed on this Mac. Manifests are a field-for-field 0.8.2 copy with version, date, URL, SHA, and tag links updated. Pipeline on the PR will be the official check.
