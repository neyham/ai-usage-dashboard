# Changelog

All notable changes to this project are documented in this file.

## Unreleased

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
