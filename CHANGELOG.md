# Changelog

All notable changes to this project are documented in this file.

## Unreleased

## [0.4.0] - 2026-07-31

### Added

- Align Codex credential discovery with Claude and Grok on Windows: try the
  native `%USERPROFILE%\.codex\auth.json` first, then fall back to the default
  Ubuntu WSL home, and accept a bounded `wsl:<distro>:<absolute-path>` override.
- Show the available Codex banked-reset count and the earliest known expiry.
- Add opt-in Grok Build tracking for the server-reported usage period, reset
  time, and optional monthly billing allowance. The integration reuses the
  official CLI login, never sends or persists refresh-token values separately,
  and supports bounded `wsl:` credential reads or explicit Windows UNC paths
  into WSL.
- Restore dual-window Codex fixtures in Judge Demo / mock mode (5H + 7D) while
  keeping weekly-only as an independent UI regression scenario.

### Changed

- Render only rate-limit windows returned by a provider, so accounts without a
  Codex five-hour window no longer show an empty legacy row.
- Expand provider selection and responsive validation from zero-to-three to
  zero-to-four panels.

### Fixed

- Accept Claude responses with only one standard usage window or only the
  enterprise extra-usage budget instead of degrading the entire provider, and
  fall back to legacy or `limits[]` scoped weekly windows when necessary.
- Show the allowlisted Grok subscription tier reported by the official service
  (with the official CLI token claim as a fallback) instead of mislabeling the
  metered `GrokBuild` product as a `Build` plan.
- Renew an expired Grok access token through the official CLI before reporting
  `GROK SESSION EXPIRED`, retry one billing `401`, distinguish forbidden
  access, and serialize bounded renewal with a per-source 15-minute backoff.

### Security

- Bump PostCSS to 8.5.25 to address GHSA-r28c-9q8g-f849 (build-time path
  traversal advisory).

## [0.3.0] - 2026-07-17

### Added

- Added persistent home-screen provider selection. Disabled providers are
  removed from the dashboard and skipped by credential and network refresh
  paths.
- Added an isolated `--judge-demo` launch mode backed by bundled synthetic
  fixtures. It does not read normal configuration, credentials, or cache data,
  and never enters a live provider request path.
- Added an NSIS Start menu shortcut for the one-click Judge Demo experience.

### Changed

- Made the dashboard adapt cleanly to zero, one, two, or three selected provider
  panels across the existing Surface, compact, portrait, and Snap layouts.
- Preserved disabled-provider cache entries so a later re-enable can show the
  last known value before the next live refresh.
- Expanded Playwright coverage for provider selection, offline demo disclosure,
  and dynamic panel counts.

### Fixed

- Accept Codex usage responses that expose only one duration-labelled rate-limit
  window instead of failing the entire provider card when the legacy secondary
  window is null.

## [0.2.0] - 2026-07-12

### Changed

- Rebuilt the dashboard UI for Surface scaling, portrait, Snap, touch, and
  compact desktop layouts.
- Changed the normal refresh default and minimum to five minutes, with a
  15-minute floor in screensaver mode.
- Tightened the existing Rust-to-renderer boundary with sanitized status and
  error handling.
- Hardened the existing per-provider last-known-good cache and aggregate
  degraded-state calculation.
- Hardened Claude renewal with WSL-compatible locks, atomic writes, 429
  cooldowns, bounded CLI recovery, and a cross-process recovery throttle.
- Made malformed configuration, invalid mock modes, and malformed mock fixtures
  fail closed.
- Made the existing idle-task and screensaver installation scripts reversible
  and safer to upgrade or remove.
- Added Windows CI, Rust checks, and a Playwright viewport regression matrix.
- Added the MIT project license and bundled third-party font notices.

### Security

- Native Claude credential files are no longer directly modified during OAuth
  renewal.
- Provider credentials and raw API errors remain outside the renderer process.
- Updated `quick-xml`, `quinn-proto`, and `anyhow` to versions patched for the
  RustSec advisories current on 2026-07-11.

## [0.1.0] - 2026-06-09

- Initial public Tauri dashboard for Claude, Codex, and DeepSeek usage.
