# Changelog

All notable changes to this project are documented in this file.

## Unreleased

### Changed

- Two enabled panels stack as wide landscape cards instead of two portrait
  columns, so the IP skin can use labeled bars with the mascot on the side.

## [0.8.1] - 2026-08-26

### Added

- Show **Grok Bot** quota on the Cursor card (same `GetSandUsageStatus` feed
  the Telegram daily-monitor uses). Cursor Included/Auto/API stay as they are;
  a missing Bot response does not fail the Cursor panel.

### Changed

- Cursor Included and Auto share one billing-cycle reset label instead of
  printing the same time twice. Usage bars keep even row height.

## [0.8.0] - 2026-08-22

### Changed

- Restyle Grok, Cursor, and Codex IP mascots to the same dumpling clay-chibi
  娘 look as DeepSeek: simple oval eyes, round bodies, matte toy shading.
  Grok is a close-up dumpling Mars girl with a rocket hair clip; Cursor is a
  long-haired mouse girl without a tail; Codex is a mint headset operator in
  a listening pose.
- Show Antigravity's real Google AI plan instead of the product name. The
  product name `Antigravity` is not a plan and is no longer labeled Free;
  Google AI Pro/Ultra come from `paidTier` or a real tier id. The featured
  quota number is the 7-day window. Compact Antigravity cards keep the ring
  gauge. Token refresh scans a large installed `agy` binary and strips
  neighboring bytes that would otherwise corrupt the OAuth client.
- Show Cursor Included, Auto, and API usage as three labeled percentages
  instead of a blended monthly figure. Compact Cursor cards use the ring
  gauge; wide cards keep the three bars in the panel accent.
- Show Grok banked usage-limit resets when the account has any remaining.
- Classic EVA skin no longer draws mascots. It uses segmented NERV-style
  progress bars.
- Compact IP rings draw the hotter of the outer/inner windows so a spent 5H
  quota is not hidden behind a calm 7D arc.
- Refresh README screenshots: five-panel IP, six compact IP rings, and the
  classic EVA skin.

## [0.7.0] - 2026-08-21

### Added

- Add opt-in **Cursor** usage tracking on Windows, macOS, and Linux. The
  dashboard reads the official CLI login or the Cursor desktop session, then
  shows included monthly usage, named-model API usage, plan, and billing-cycle
  reset. Cursor stays off by default so an upgrade never starts reading a new
  credential source without an explicit selection.
- Add opt-in **Antigravity** Gemini quota tracking on Windows, macOS, and
  Linux. The dashboard reads the official `agy` OAuth file or the OS keyring
  item current `agy` builds write, may refresh an expired access token in
  memory without embedding Google OAuth client secrets, and shows the Gemini
  5-hour and weekly windows. Antigravity stays off
  by default.
- Add an **IP color-slab** art pack with original mascot restyles. Wide
  top-tier cards use labeled usage bars; compact cards keep the dual ring.

### Fixed

- Continue Cursor credential discovery after an expired CLI or Keychain token
  so a still-valid desktop `state.vscdb` session can be used.
- Report a missing macOS Cursor Keychain login as `LOGIN REQUIRED` instead of
  `CREDENTIAL ERROR`.
- Remove the dead Antigravity expired-session branch that always returned
  `SESSION EXPIRED`.
- Fetch enabled providers in parallel so six live panels no longer stack
  network timeouts into the frontend refresh limit.
- Read current `agy` logins from the OS keyring when
  `antigravity-oauth-token` is absent, so Linux GNOME Keyring sessions are not
  reported as `LOGIN REQUIRED`.

### Changed

- Make the IP color-slab skin the default. Display Settings can switch back to
  the circular ring-gauge HUD. Existing configs that already saved `artSkin`
  keep that choice.
- Stop embedding Google installed-app OAuth client secrets. Antigravity
  in-memory refresh now uses `ANTIGRAVITY_OAUTH_CLIENT_ID` /
  `ANTIGRAVITY_OAUTH_CLIENT_SECRET`, `client_id`/`client_secret` on the local
  login JSON, or a scan of an installed `agy` / Antigravity.app (same approach
  as CodexBar). If none are found, an expired session reports
  `SESSION EXPIRED` until `agy` refreshes the login.
- Windows now reads only native Windows credential files. Automatic Ubuntu WSL
  fallback and explicit `wsl:<distro>:<path>` credential paths are removed.
- Five enabled panels now fill the grid. Exactly one top-tier plan
  (SuperGrok Heavy, Cursor Ultra/Enterprise, or Google AI Ultra) gets the
  left half; two top-tier plans split the top row. One to four panels stay
  equal columns, and six stay a 3×2 wrap.
- Display Settings now shows the six provider toggles in two columns so Cursor
  and Antigravity stay on-screen on short displays.
- Soften the six-panel palette: chrome stays ink-colored, each provider keeps
  one muted accent, and the ring gauge no longer adds extra neon glows.
- Add a Q-version mascot to each panel. The face follows usage: idle under
  35%, focused to 70%, tense to 88%, collapsed above that, and a confused
  shrug on login or credential failures. The figure is the panel body
  protagonist, with a mood halo and peak-usage numeral; decorative radar and
  DeepSeek histogram are gone.
- Unify provider accents to one lightness/chroma and drop leftover cyan
  decoration so each panel is only ink + its own accent + ok/warn/danger.
- Replace segmented usage bars with a square ring gauge. The mascot stands
  in the ring, the arc is usage, and the peak numeral sits in the opening.
- Scale the ring numeral with the gauge, enlarge the mascot so it fills the
  dial, clear a faint box from the Cursor art, and tint DeepSeek's empty
  track with the panel accent.

## [0.6.0] - 2026-08-05

### Added

- Add a persisted **Low Power Mode** in Display Settings. It stops decorative
  animation, removes the most expensive full-screen WebView compositing layers
  and glows, updates the clock once per minute, pauses renderer work while the
  window is hidden, and raises automatic provider refreshes to a 15-minute
  minimum without affecting manual refresh.

## [0.5.0] - 2026-08-02

### Added

- Settings toggle for **windowed vs fullscreen** on Windows, macOS, and Linux.
  Choice is persisted as `windowMode` in `config.json`, applied at launch when
  no CLI override is present, and switchable at runtime from Display Settings
  (screensaver launches remain fixed at start).
- Write-only DeepSeek API key input in Display Settings on all three desktop
  platforms. Blank input preserves the current key, successful saves refresh
  immediately, and the backend never returns the stored value to the UI.

### Fixed

- Distinguish provider setup and auth failures from generic transport errors:
  DeepSeek reports `KEY MISSING` / `AUTH EXPIRED` / `AUTH FORBIDDEN` instead of
  a blanket `API ERROR` when the key is absent or rejected; Codex reports
  `LOGIN REQUIRED` / `AUTH EXPIRED` / `AUTH FORBIDDEN` / `RATE LIMITED` on the
  corresponding credential and HTTP paths, while malformed/unreadable Codex
  credential files report `CREDENTIAL ERROR`. Claude and Grok already used
  dedicated auth statuses; the UI chip treats credential/setup states as auth.
- Harden UTF-8 BOM handling for `config.json` parsing with an explicit regression
  test, and tolerate a BOM when loading `state.json` cache files with its own
  regression test.
- Save provider and window changes through one transactional command, submit only
  changed fields, include an optional DeepSeek key in the same config transaction,
  preserve process-only CLI overrides, and roll the native window
  back if persistence fails. If the rollback itself fails, synchronize the backend
  and renderer to the actual OS window state instead of reporting the old mode.
  Fullscreen remains interactive with a visible cursor; only screensaver mode hides
  it, and a screensaver that cannot enter its required window state exits fail-closed.
- Create Mac/Linux `config.json` replacement inodes with mode 0600 and tighten
  existing upgrade configs at load because the direct DeepSeek settings path
  stores its key in that per-user file.
- Release the shared cache lock during network refreshes and persist bounded
  provider `Retry-After` cooldowns for Codex, DeepSeek, and Grok.
- Keep rate-limit and Grok authentication tags visible even when cached data is
  stale, and make the default Windows Ubuntu WSL Codex fallback read
  `auth.json` reliably through `wsl.exe`; a missing file reports `LOGIN REQUIRED`
  without misclassifying other WSL failures as missing credentials.
- Prevent the Linux installer from falling back to an amd64 AppImage on arm64,
  and stage release bundles by the requested version instead of the first stale
  artifact found in a reused target directory.

## [0.4.1] - 2026-07-31

### Added

- Multi-platform packaging for Linux and macOS: CI jobs on Ubuntu and macOS,
  Release builds for `.deb` / AppImage / `.dmg`, and one-line install scripts
  (`scripts/install-linux.sh`, `scripts/install-macos.sh`) that download assets
  from GitHub Releases and verify SHA-256 checksums.
- Linux package metadata (`deb` runtime depends) and macOS minimum system
  version in `tauri.conf.json`.

### Fixed

- Gate Windows-only Codex WSL helper imports so Linux/macOS builds compile
  without unused-import warnings.
- Fix rustfmt import order in the Codex fetcher so multi-platform CI
  `cargo fmt --check` passes.

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
