# Devpost Submission Draft

## Project name

AI Usage Dashboard: Selective Local Quota Monitor

## Tagline

One local Surface dashboard for AI quotas, with explicit control over what gets
displayed and refreshed.

## Track

Developer Tools

## Project description

AI Usage Dashboard is a local-first Windows desktop dashboard for Codex and
Claude usage windows plus DeepSeek API balance. It turns a spare Surface or any
Windows display into an always-visible quota monitor, so developers can see
limits and reset times before an active coding session is interrupted.

The OpenAI Build Week extension adds end-to-end provider selection. A developer
can show any combination of the three providers, and an unchecked provider is
not merely hidden: its credential discovery and network request paths are
skipped. The choice persists atomically, disabled-provider caches are retained
for a later re-enable, and the interface adapts to zero, one, two, or three
panels across Surface landscape, portrait, Snap, compact, mouse, and touch
layouts.

The project remains local-only. There is no project server, analytics, or
telemetry. Secrets remain in the Rust backend, while the renderer receives only
sanitized percentages, reset times, balance values, and status text.

## Technical implementation

- **Tauri + Rust backend:** owns credentials, provider HTTP requests, cache,
  refresh coordination, and atomic configuration writes.
- **React + TypeScript renderer:** renders sanitized DTOs and a touch-friendly
  provider settings dialog without receiving tokens or raw provider errors.
- **Selective refresh orchestration:** an `EnabledProviders` value crosses the
  config, cache-summary, live-fetch, mock, IPC, and layout boundaries. Each
  provider branch short-circuits before credential access when disabled.
- **Concurrency safety:** settings changes made during an in-flight refresh are
  queued, stale selections are not republished, and a new cycle runs for the
  latest selection.
- **Cache behavior:** successful cards remain cached per provider; disabling a
  provider stops updates without deleting its last-known-good record.
- **Responsive layout:** CSS and Playwright assertions cover zero, one, two, and
  three panels at seven desktop and Surface viewports.
- **Judge Demo:** `--judge-demo` starts from `Config::default()` and an empty
  cache, uses embedded synthetic fixtures through the production parsers, and
  hard-routes every refresh to mock data before any live fetch path. Demo
  selections persist in a separate credential-free file.
- **Windows packaging:** the NSIS candidate creates an **AI Usage Dashboard
  (Judge Demo)** Start menu shortcut with the isolated launch flag.

## Impact

AI coding subscriptions are increasingly used together, but quota status is
fragmented across tools and reset windows. This dashboard reduces mid-task
surprises without introducing another hosted account or sending credentials to
a third party. Selective refresh also lets a developer remove an expired or
unused subscription cleanly, eliminating repeated authentication errors and
unnecessary background traffic while keeping the option to restore it later.

## Innovation

The key distinction is that provider choice is enforced at the data-access
boundary, not treated as cosmetic card visibility. The same preference controls
credential reads, provider requests, aggregate health, cache presentation, and
responsive composition. The result is a small but complete local control plane
for a multi-provider AI workflow.

The Judge Demo applies the same principle to evaluation: it is visibly marked,
credential-free, cache-isolated, and structurally unable to fall through to a
real provider request. Judges can inspect the complete workflow without an AI
subscription or source build.

## Judge testing steps

1. Download and run the Windows NSIS installer. No Node.js, Rust, provider
   login, or API key is required.
2. Open **AI Usage Dashboard (Judge Demo)** from the Windows Start menu.
3. Confirm the footer reads `SYNTHETIC DEMO · OFFLINE` and three synthetic
   provider panels are visible.
4. Open the settings button, disable Claude, and save. Confirm the home screen
   immediately becomes a balanced two-panel Codex/DeepSeek layout.
5. Disable the remaining providers to inspect the zero-panel standby state,
   then enable one provider to inspect the centered single-panel layout.
6. Close and reopen the Judge Demo shortcut to confirm the demo selection
   persists separately from live configuration.
7. Use Refresh and confirm the synthetic values return without credential
   prompts or provider setup.

Judge Demo validates the product workflow and responsive UI. It does not test
live provider authentication, endpoint availability, or real quota accuracy.

## Links

- Public repository: https://github.com/neyham/ai-usage-dashboard
- Public v0.3.0 NSIS installer: https://github.com/neyham/ai-usage-dashboard/releases/download/v0.3.0/AI-Usage-Dashboard_0.3.0_x64-setup.exe
- Public SHA-256 checksum: https://github.com/neyham/ai-usage-dashboard/releases/download/v0.3.0/AI-Usage-Dashboard_0.3.0_SHA256SUMS.txt
- Devpost project URL: `[ADD DEVPOST PROJECT URL BEFORE SUBMISSION]`

## Video storyboard (2:40 maximum)

| Time | Visual | Narration |
| --- | --- | --- |
| 0:00-0:15 | Surface running the dashboard | AI coding sessions can stop unexpectedly when quota windows are scattered across providers. |
| 0:15-0:30 | Title card: existing project vs Build Week extension | The local dashboard and provider integrations existed before the event; Build Week added selective control across the full stack. |
| 0:30-0:50 | Launch **AI Usage Dashboard (Judge Demo)** | The one-click judge path uses bundled synthetic data, reads no normal config or credentials, and makes no provider requests. |
| 0:50-1:25 | Open settings; switch from three panels to two, then one | A provider is not just hidden. Disabling it skips credential discovery and refresh traffic, while the layout recomposes immediately. |
| 1:25-1:45 | Show zero-panel standby, then re-enable Codex | Every combination is intentional, including no active subscriptions. Preferences persist and old caches are retained for later. |
| 1:45-2:05 | Brief code view of Rust refresh branches and sanitized DTO | Rust owns secrets and enforces the selection before provider access. React sees only sanitized usage data. |
| 2:05-2:25 | Surface landscape, portrait, and Snap screenshots/tests | The extension is tested across seven viewports, touch targets, settings persistence, and zero-to-three-panel layouts. |
| 2:25-2:40 | Final two-panel Surface view and repository URL | The result is a local, inspectable developer tool that monitors only the subscriptions a developer actually uses. |

## Pre-event work vs Build Week work

### Existing before OpenAI Build Week

- Tauri/Rust/React desktop architecture and sanitized IPC boundary.
- Claude, Codex, and DeepSeek parsing and credential integrations.
- Per-provider cache, cooldown, retry, degraded-state, and refresh behavior.
- Surface-responsive dashboard, fullscreen, idle task, and screensaver modes.
- Windows installer/release automation, mock fixtures, and viewport tests.
- Public MIT-licensed v0.2.0 release.
- Codex single-window usage-response compatibility fix.

### Added on 2026-07-17 during OpenAI Build Week

- Persistent provider-selection model and settings UI.
- Disabled-provider short-circuit before credential and network operations.
- Aggregate health calculations that ignore disabled providers.
- Retained caches and refresh queuing for settings changes during live work.
- Zero, one, two, and three-panel responsive compositions.
- Surface device installation and cache-timestamp validation with Claude
  disabled while Codex and DeepSeek refreshed.
- Isolated synthetic Judge Demo and one-click NSIS Start menu shortcut.
- Expanded Playwright/Rust coverage, 0.3.0 candidate packaging, and this
  submission draft.

This extension was developed with Codex and GPT-5.6. Product direction,
requirements, device validation, and final submission decisions remain with the
human entrant.

## OpenAI feedback

- `/feedback` Session ID: `019f4a54-b848-7a33-8360-57e6213c76be`
- Feedback summary: `[ADD OPTIONAL SUMMARY BEFORE SUBMISSION]`

## Known limitations and disclosure

- Live provider interfaces include undocumented endpoints and can change.
- The Windows installer is not code-signed and may trigger SmartScreen.
- Judge Demo uses fixed synthetic fixtures and cannot prove live-provider
  compatibility.
- The v0.3.0 candidate has not been committed, pushed, released, or uploaded at
  the time this draft was prepared.
