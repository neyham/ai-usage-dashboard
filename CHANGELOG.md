# Changelog

All notable changes to this project are documented in this file.

## Unreleased

### Added

- Show the available Codex banked-reset count and the earliest known expiry.

### Changed

- Render only rate-limit windows returned by a provider, so accounts without a
  Codex five-hour window no longer show an empty legacy row.

### Fixed

- Accept Claude responses with only one standard usage window or only the
  enterprise extra-usage budget instead of degrading the entire provider, and
  fall back to legacy or `limits[]` scoped weekly windows when necessary.

## [0.3.0] - 2026-07-17 (release candidate)

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
- Expanded Playwright coverage for provider selection, Judge Demo disclosure,
  and dynamic panel counts.

### Fixed

- Accept Codex usage responses that expose only one duration-labelled rate-limit
  window instead of failing the entire provider card when the legacy secondary
  window is null.

### Build Week

- The provider-selection workflow, isolated Judge Demo, judging documentation,
  and 0.3.0 candidate packaging were completed on 2026-07-17 with Codex and
  GPT-5.6 for OpenAI Build Week. The provider integrations, local-first Tauri
  architecture, cache, Surface UI, and v0.2.0 release existed before this work.

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
