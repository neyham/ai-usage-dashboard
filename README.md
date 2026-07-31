# AI Usage Dashboard

[![CI](https://github.com/neyham/ai-usage-dashboard/actions/workflows/ci.yml/badge.svg)](https://github.com/neyham/ai-usage-dashboard/actions/workflows/ci.yml)

A local-first desktop dashboard for viewing Claude Code, Codex, and Grok Build
usage alongside a DeepSeek API balance. Built with Tauri, Rust, React, and
TypeScript for Windows, Linux, and macOS. Windows also supports scheduled idle
launch and a screensaver mode.

![AI Usage Dashboard showing synthetic mock data](docs/assets/dashboard.png)

> The screenshot uses synthetic test data. It contains no account information or
> provider credentials.

## What it shows

- Claude utilization for the available session, weekly, or monthly extra-usage
  window, plus reset times, cooldowns, and cache state.
- Codex utilization for the available rate-limit windows, reset times, plan
  label, and banked-reset count with the earliest known expiry.
- Grok Build utilization for the server-reported credit period and reset time,
  subscription tier, and optional monthly billing allowance.
- DeepSeek API balance and insufficient-balance state.
- A combined health state that distinguishes fresh data, partial degradation,
  total failure, and an in-progress refresh.
- Normal window and borderless fullscreen on all platforms; scheduled idle and
  screensaver launch modes on Windows.

Each provider is checked independently. A missing credential or an outage for
one provider does not erase last-known-good data for the other providers.

## Project status

Version `0.4.0` is the current **Windows** release line (WinGet + NSIS). It
keeps provider selection and the offline demo from `0.3.0`, and adds adaptive
usage windows, Codex banked resets, optional Grok Build tracking, zero-to-four
provider layouts, and aligned WSL credential discovery for Claude, Codex, and
Grok on Windows. Multi-platform packaging (Linux `.deb` / AppImage, macOS
`.dmg`, and one-line install scripts) is in progress on `main` for the next
release; published installers are not code-signed.

This project reads credential formats and usage endpoints used by provider CLI
tools. Some of those interfaces are undocumented and can change without notice.
The project is not affiliated with, endorsed by, or supported by Anthropic,
OpenAI, xAI, or DeepSeek.

## Privacy and security

The Rust backend owns credentials and network requests. The React renderer only
receives a sanitized summary containing percentages, reset times, plan labels,
balances, timestamps, and status text.

- No project server, analytics, or telemetry is used.
- Provider tokens, API keys, credential files, and raw API error bodies are not
  sent to the renderer.
- DeepSeek keys are never written to the cache. The OS credential store is
  preferred (Windows Credential Manager, macOS Keychain, Linux Secret Service),
  then environment variables, then plaintext configuration.
- The dashboard only checks whether a Grok refresh token is non-empty. It never
  extracts that value into app state, sends it, logs it, or passes it to a
  child process, and it does not write Grok credentials itself. When an access
  token expires, it can run the official Grok CLI's non-interactive
  model-discovery command; that CLI owns any update to its `auth.json`.
- Sanitized usage cache is stored under the platform data directory
  (`%LOCALAPPDATA%\AiUsageDashboard\state.json` on Windows,
  `~/.local/share/AiUsageDashboard/state.json` on Linux, Application Support on
  macOS). It can still contain private account information and should be
  treated as sensitive.
- Malformed configuration and invalid mock modes fail closed and do not contact
  live provider services.

See [SECURITY.md](SECURITY.md) before reporting a vulnerability or suspected
credential exposure.

## Install

One-line installs (preferred once multi-platform release assets are published):

```powershell
# Windows
winget install --id neyham.AIUsageDashboard --exact --source winget
```

```sh
# macOS
curl -fsSL https://github.com/neyham/ai-usage-dashboard/releases/latest/download/install-macos.sh | sh
```

```sh
# Linux (Debian/Ubuntu .deb, otherwise AppImage)
curl -fsSL https://github.com/neyham/ai-usage-dashboard/releases/latest/download/install-linux.sh | sh
```

Pin a version with `VERSION=0.5.0` before the install script, for example
`VERSION=0.5.0 sh install-linux.sh`.

### Windows (WinGet and direct installer)

The package is live in WinGet as `neyham.AIUsageDashboard`. After a new GitHub
release, community-source indexes can lag briefly while the version manifest is
reviewed and propagated:

```powershell
winget source update
winget show --id neyham.AIUsageDashboard --exact --source winget
```

The latest public **Windows** release is `v0.4.0`:

- [AI Usage Dashboard v0.4.0 NSIS installer](https://github.com/neyham/ai-usage-dashboard/releases/download/v0.4.0/AI-Usage-Dashboard_0.4.0_x64-setup.exe)
- [AI Usage Dashboard v0.4.0 SHA-256 checksum](https://github.com/neyham/ai-usage-dashboard/releases/download/v0.4.0/AI-Usage-Dashboard_0.4.0_SHA256SUMS.txt)

The `v0.4.0` release publishes a current-user NSIS `setup.exe` only. Installers
are unsigned, so Windows SmartScreen may require confirmation. After
installation, open **AI Usage Dashboard (Judge Demo)** from the Start menu for
an offline walkthrough with synthetic data.

### Linux and macOS status

Linux and macOS install scripts and CI packaging live on `main` and will ship
with the next multi-platform release tag. Until then, build from source on
those platforms (see below). Screensaver and scheduled-idle helpers remain
Windows-only.

## Offline demo

The offline demo reuses the production mock parsers and embedded fixtures, with
an extra isolation boundary:

- launch with `--judge-demo` (Start menu shortcut: **AI Usage Dashboard (Judge
  Demo)**); the UI shows `SYNTHETIC DEMO · OFFLINE`;
- it does not load normal `config.json`, provider credentials, or the normal
  cache;
- refresh actions always regenerate embedded fixture data and never construct a
  live provider request;
- provider selections persist separately in the platform config directory
  (`judge-demo.json`) and cannot alter live settings;
- settings can exercise every layout supported by that build: zero through
  four panels in `v0.4.0`.

Use it to exercise the UI, selection workflow, parser-to-renderer boundary,
status presentation, and responsive layouts without touching live accounts. It
does not validate provider authentication, endpoint availability, or live quota
accuracy.

Command-line fallbacks:

```powershell
# Windows (current-user install)
& "$env:LOCALAPPDATA\AI Usage Dashboard\ai-usage-dashboard.exe" --judge-demo
```

```sh
# Linux / macOS (from a local build)
./src-tauri/target/release/ai-usage-dashboard --judge-demo
```

## Runtime requirements

| Platform | Requirements |
| --- | --- |
| Windows 10/11 | WebView2 Runtime (usually present on Windows 11) |
| Linux | WebKitGTK 4.1, GTK 3 (see [packaging/linux](packaging/linux/README.md)) |
| macOS | macOS 11+ |

## Source build requirements

- Node.js 18 or later.
- Rust 1.88 or later.

**Windows:** `x86_64-pc-windows-msvc` toolchain and Visual Studio Build Tools with
**Desktop development with C++**. Build from PowerShell for Windows integration
(Credential Manager, Task Scheduler, screensaver). A WSL build produces a Linux
binary instead.

**Linux (Ubuntu 24.04 example):**

```sh
sudo apt-get install -y build-essential curl wget file libssl-dev libgtk-3-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf pkg-config
```

**macOS:** Xcode Command Line Tools (`xcode-select --install`).

## Build and run

```sh
git clone https://github.com/neyham/ai-usage-dashboard.git
cd ai-usage-dashboard
npm ci
npm run app:dev
```

Build the native app and platform installers:

```sh
npm run app:build
```

Outputs land under `src-tauri/target/release/` and
`src-tauri/target/release/bundle/` (NSIS on Windows, `.deb`/AppImage on Linux,
`.dmg`/`.app` on macOS).

## Provider setup

Use the settings button in the bottom toolbar to choose which providers appear
on the home screen. Disabled providers are excluded from automatic and manual
refresh cycles, including credential reads and network requests. Changes apply
immediately and persist in `config.json`.

| Provider | Default credential source | Override |
| --- | --- | --- |
| Claude | `~/.claude/.credentials.json`, then `credentials.json`; on Windows also the `Ubuntu` WSL home | `claudeCredentialsPath` (`wsl:<distro>:<absolute-path>` on Windows only) |
| Codex | `~/.codex/auth.json`; on Windows also the `Ubuntu` WSL home | `codexAuthPath` (`wsl:` or `\\wsl.localhost\...` on Windows only) |
| DeepSeek | OS credential store target `AiUsageDashboard/DeepSeekApiKey`, then `DEEPSEEK_API_KEY` | `deepSeekCredentialTarget` or `deepSeekApiKey` |
| Grok Build | `~/.grok/auth.json`; on Windows also the `Ubuntu` WSL home | `grokCredentialsPath` (`wsl:` or UNC on Windows only) |

Sign in with the official Claude Code, Codex, and Grok Build clients before
enabling their panels. Grok is disabled by default so an upgrade never starts
reading a newly introduced credential source without an explicit selection.

**DeepSeek without a plaintext file:** on Windows, create a Generic credential
in Credential Manager with network address `AiUsageDashboard/DeepSeekApiKey`
and the API key as the password. On Linux/macOS, store the same
service/account pair via the platform keyring, or set `DEEPSEEK_API_KEY`.

For a Windows WSL distribution other than `Ubuntu`, configure an explicit path
such as:

```json
{
  "claudeCredentialsPath": "wsl:Ubuntu-24.04:/home/your-user/.claude/.credentials.json",
  "codexAuthPath": "wsl:Ubuntu-24.04:/home/your-user/.codex/auth.json",
  "grokCredentialsPath": "wsl:Ubuntu-24.04:/home/your-user/.grok/auth.json"
}
```

The dashboard invokes `wsl.exe` without interpolating the configured path into
a shell command, caps credential input at 64 KiB, and stops the reader after
15 seconds. A `\\wsl.localhost\...` UNC path remains supported when preferred.
Automatic Grok CLI renewal requires the native official path or a `wsl:` path;
UNC paths remain read-only. An explicit path is fail closed. If it cannot be
read, the dashboard reports an authentication or data error instead of silently
selecting another account.
When a recognized Grok access token expires and the login contains a refresh
token, the dashboard runs
`grok --no-auto-update models` through the official CLI with no shell
interpolation, no captured output, and a 20-second timeout. The official CLI
silently renews its own session, after which the dashboard rereads
`auth.json`. The dashboard never extracts the refresh-token value into app
state, sends it, logs it, passes it to the child process, or writes the
credential file itself. A billing `401` can trigger the same flow once, covering
clock skew or a token rejected before its local expiry. Attempts are serialized
across dashboard processes and held per credential source for 15 minutes before
another background attempt. An unavailable CLI is shown as
`GROK SESSION EXPIRED` instead of incorrectly claiming that the credential file
is missing; a `403` is reported separately as `GROK ACCESS UNAVAILABLE`.

CodexBar's Grok adapter informed the credential boundary, official-CLI renewal,
and provider selection. Grok Build 0.2.111 currently returns `Method not found`
for its `x.ai/billing` ACP probe, so this dashboard uses the live-verified
read-only JSON billing responses that include an explicit period type and Grok
Build product usage. These are not public APIs; incompatible responses fail
closed to sanitized last-known-good data.

### Claude token renewal

Native Windows Claude credential files are read-only to the dashboard. Direct
OAuth renewal is intentionally disabled for those files because the dashboard
cannot participate in every lock used by Claude Code. If the native token has
expired, refresh it with Claude Code or opt into the bounded CLI fallback in
configuration.

For explicit `wsl:<distro>:<absolute-path>` credentials, direct renewal uses
Claude Code-compatible lock directories, reloads the file while holding the
locks, merges only rotated OAuth fields, and writes atomically.

The optional Claude Code fallback is off by default because its recovery command
may consume a small amount of usage. When enabled, it is limited to selected
authentication failures, clamped to a maximum timeout and budget, and throttled
across dashboard processes to one attempt per 30 minutes.

## Configuration

Run the following to create or open the configuration file:

```sh
# Linux / macOS
./src-tauri/target/release/ai-usage-dashboard --config

# Windows
.\src-tauri\target\release\ai-usage-dashboard.exe --config
```

Config path by platform:

| Platform | Path |
| --- | --- |
| Windows | `%APPDATA%\AiUsageDashboard\config.json` |
| Linux | `~/.config/AiUsageDashboard/config.json` |
| macOS | `~/Library/Application Support/AiUsageDashboard/config.json` |

Example:

```json
{
  "refreshIntervalMinutes": 5,
  "networkTimeoutSeconds": 15,
  "enabledProviders": {
    "codex": true,
    "claude": true,
    "deepseek": true,
    "grok": false
  },
  "deepSeekApiKey": "",
  "deepSeekCredentialTarget": "AiUsageDashboard/DeepSeekApiKey",
  "claudeCredentialsPath": "",
  "claudeCodeRefreshEnabled": false,
  "claudeCodeCommand": "claude",
  "claudeCodeRefreshTimeoutSeconds": 30,
  "claudeCodeRefreshMaxBudgetUsd": 0.03,
  "codexAuthPath": "",
  "grokCredentialsPath": "",
  "mockMode": ""
}
```

Settings changed in the application apply immediately. Restart the application
after editing the JSON file directly. Timing and CLI recovery values are
clamped to safe bounds:

| Setting | Allowed range |
| --- | --- |
| `refreshIntervalMinutes` | 5 to 1,440 minutes |
| `claudeCodeRefreshTimeoutSeconds` | 5 to 120 seconds |
| `claudeCodeRefreshMaxBudgetUsd` | USD 0.001 to USD 0.10 |

Avoid `deepSeekApiKey` unless no safer storage option is available; it stores
the key as plaintext in `config.json`.

## Refresh and cache behavior

- Enabled providers refresh immediately at launch, then repeat every five
  minutes by default. Normal mode can be configured from 5 minutes to 24 hours.
- Disabled providers are not displayed, do not read credentials, and do not
  receive background or manual refresh requests. Their last-known cache remains
  available if they are enabled again later.
- Screensaver mode enforces a 15-minute minimum interval to avoid unnecessary
  idle-time traffic.
- Press `F5` or use the refresh button for an on-demand refresh in normal or
  fullscreen mode.
- Provider usage and balance requests receive one retry after transport
  failures, HTTP 408, or HTTP 5xx responses. Claude OAuth renewal is not
  automatically replayed after an ambiguous transport failure because refresh
  tokens can rotate.
- Claude HTTP 429 responses honor `Retry-After` plus a 30-second buffer. If no
  usable value is returned, the cooldown defaults to 30 minutes.
- A successful response is cached per provider. Cached data older than six
  hours is marked as possibly stale.

## Understanding status warnings

`SYSTEM NOMINAL` means every enabled provider returned fresh, healthy data.
`WARNING - SERVICE DEGRADED` means at least one enabled provider is cached,
unavailable, rate limited, unauthenticated, or reporting a non-nominal balance.
`SERVICE FAILURE` means every enabled check failed or only fallback data was
available. With no providers selected, the dashboard remains in standby.

If every enabled panel fails:

1. Confirm that Claude Code, Codex, and any enabled Grok Build provider are
   signed in and that a DeepSeek key is available through one of the documented
   sources.
2. Open `config.json` and check for invalid JSON or an incorrect explicit path.
3. Restart after editing the JSON directly, then trigger one manual refresh.
4. Check the providers' official status pages. A provider outage should not be
   repaired by deleting credentials.
5. Use mock mode to separate a local UI problem from a credential or provider
   problem.

Deleting `%LOCALAPPDATA%\AiUsageDashboard\state.json` only clears cached display
data; it does not repair authentication.

## Launch modes

| Argument | Behavior |
| --- | --- |
| none | Normal resizable window |
| `--fullscreen` | Borderless fullscreen; `Esc` exits |
| `--judge-demo` | Offline synthetic demo; no normal config, credential, cache, or provider access |
| `/s` or `-s` | Fullscreen, always on top, and exits on real input after a short arming delay |
| `--config` or `/c` | Opens `config.json` and exits |
| `/p <HWND>` | Windows screensaver preview; intentionally exits without rendering |

Task Scheduler is the recommended idle-launch method:

```powershell
.\install-idle-task.ps1 -IdleMinutes 10
.\install-idle-task.ps1 -Remove
```

The script prefers an installed executable under `%LOCALAPPDATA%`, then falls
back to the release build. Use `-ExePath` when the executable lives elsewhere.

The `.scr` integration is experimental because WebView2 is not embedded in the
small Windows Settings preview pane:

```powershell
.\install-screensaver.ps1 -TimeoutSeconds 900
.\install-screensaver.ps1 -Remove
```

The installer backs up the current user's screensaver registry values and only
restores them if AI Usage Dashboard is still the selected screensaver when it is
removed.

## Tests

Install the Playwright browser once, then run the renderer and Rust suites:

```powershell
npx playwright install chromium
npm test
npm run test:rust
cargo clippy --locked --all-targets --manifest-path src-tauri/Cargo.toml -- -D warnings
```

The UI suite exercises healthy, rate-limited, partial-failure, and
insufficient-balance states across seven viewports, including Surface 200%
landscape, portrait, and half-Snap layouts. It also checks overflow, touch
targets, refresh state, keyboard behavior, screensaver input exit, offline demo
disclosure, and zero/one/two/three/four-panel selection layouts.

Set `mockMode` to `normal`, `claude429`, or `failures` to exercise the embedded
provider fixtures without network access. An unknown value displays `INVALID
MOCK MODE` and remains offline. `--judge-demo` is the safer offline entrypoint
because it ignores normal configuration and cache files entirely.

## Repository layout

```text
src/                         React and TypeScript renderer
scripts/viewport-check.mjs   Playwright viewport and interaction suite
mocks/                       Embedded provider response fixtures
src-tauri/src/               Rust backend, cache, fetchers, and launch modes
src-tauri/nsis-hooks.nsh     Offline demo Start menu shortcut
docs/releases/               Per-release notes
install-idle-task.ps1        Scheduled idle-mode install and removal
install-screensaver.ps1      Experimental .scr install and safe restoration
```

## License

AI Usage Dashboard is licensed under the [MIT License](LICENSE). Embedded font
software keeps its separate OFL-1.1 terms; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
