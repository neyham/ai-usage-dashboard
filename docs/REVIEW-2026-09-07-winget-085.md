# WinGet 0.8.5

Did not bump the product version, retag, or redo the GitHub Release.

## SHA-256

Installer: `AI-Usage-Dashboard_0.8.5_x64-setup.exe`  
Source: GitHub Release `v0.8.5` plus `AI-Usage-Dashboard_0.8.5_SHA256SUMS.txt`

```
EDC53686222661304AF4D92EC7DFADEE3A0B474D30C61BBCB5504C7931698E1D
```

Local `shasum -a 256` of the downloaded NSIS file matched the SUMS line for `x64-setup.exe` (not the MSI `52D1...` line). Manifest uses that value (uppercase, no spaces). GitHub asset digest is the same hash. Size 7,487,396 bytes.

## Product repo

- Commit: `4581ab0` `chore: add WinGet 0.8.5 manifests` on `origin/main`
- Path: `packaging/winget/0.8.5/` (installer / locale.en-US / version, schema 1.12.0)
- `ReleaseDate: 2026-09-06` (GitHub Release `published_at`)
- InstallerUrl: `https://github.com/neyham/ai-usage-dashboard/releases/download/v0.8.5/AI-Usage-Dashboard_0.8.5_x64-setup.exe`
- LicenseUrl / ReleaseNotesUrl point at `v0.8.5`
- Publisher / Scope / ProductCode same as 0.8.4 (`InstallerType: nullsoft`, `Scope: user`, `ProductCode: AI Usage Dashboard`, `Publisher: neyha` in AppsAndFeaturesEntries)
- Older untracked REVIEW files were not committed

## winget-pkgs PR

- https://github.com/microsoft/winget-pkgs/pull/430917
- Title: `New version: neyham.AIUsageDashboard version 0.8.5`
- Head: `neyham:winget-ai-usage-dashboard-0.8.5` → `microsoft/winget-pkgs` `master`
- State: **OPEN** (not merged)

## winget validate

Not run. `wingetvalidate` and `wingetcreate` are not installed on this Mac. Manifests are a field-for-field 0.8.4 copy with version, date, URL, SHA, and tag links updated. Pipeline on the PR will be the official check.
