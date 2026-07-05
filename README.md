# AI Usage Dashboard (Tauri)

Cross-platform AI usage dashboard for a mostly-idle machine. Shows real
server-side usage/balance for **Codex**, **Claude**, and **DeepSeek** in a dark,
mecha-terminal full-screen view. Tauri rewrite of the WinForms prototype in
Rust + React.

> Architecture: the **Rust backend owns all secrets and network access**. The
> renderer only ever receives a sanitized `UsageSummary` — never tokens, keys,
> credential file contents, or raw API error bodies.

## Layout

```
src/                     React + TypeScript renderer (dark dashboard UI)
  components/            ClockHeader, ServicePanel, ProgressMeter, StatusChip
src-tauri/
  src/
    lib.rs              Tauri app: state, commands, background refresh loop
    config.rs           %APPDATA%\AiUsageDashboard\config.json
    cache.rs            %LOCALAPPDATA%\AiUsageDashboard\state.json (last-known-good)
    secrets.rs          DeepSeek key: Credential Manager / Keychain / env / config
    util.rs             percent normalize, date parse/format
    mock.rs             mock-mode summaries (normal / claude429 / failures)
    fetchers/
      mod.rs            HTTP helpers + per-cycle orchestration
      claude.rs         OAuth usage, refresh, 401 retry, 429 cooldown, WSL creds
      codex.rs          ChatGPT/Codex backend usage
      deepseek.rs       balance
mocks/                   sample payloads (also embedded for mock mode)
scripts/make-icons.mjs   regenerates the icon set
```

## Prerequisites (Windows — the primary build target)

1. **Node 18+** (you have 22).
2. **Rust** (stable, MSVC toolchain): https://rustup.rs — pick the
   `x86_64-pc-windows-msvc` host. Also install **Visual Studio Build Tools**
   with the "Desktop development with C++" workload, and the **WebView2**
   runtime (preinstalled on Windows 11).

> Build on Windows, not inside WSL. WSL would produce a Linux binary and cannot
> reach Windows Credential Manager or the screensaver hooks. The backend already
> knows how to read the Claude credentials *out of* WSL via `wsl.exe`.

## Run

```powershell
cd ai-usage-dashboard
npm install
npm run app:dev      # dev build with hot-reload (tauri dev)
```

Production bundle:

```powershell
npm run app:build    # tauri build -> installers + AiUsageDashboard.exe
```

If you'd rather regenerate icons from a custom source image:

```powershell
node scripts/make-icons.mjs              # the bundled generator, or:
npm run tauri icon path\to\your-1024.png # Tauri's own generator
```

## Mock mode (verify the three required states first)

Set `"mockMode"` in `%APPDATA%\AiUsageDashboard\config.json` to one of:

| value        | what it shows                                              |
|--------------|-----------------------------------------------------------|
| `normal`     | all three services healthy                                |
| `claude429`  | Claude rate-limited → cached data + cooldown chip         |
| `failures`   | Codex + DeepSeek failed → cached data, UI does not blank  |

Leave it `""` for live data. Mock payloads live in `mocks/` and are embedded
into the binary, so mock mode needs no network.

## Live data sources

| Service  | Endpoint                                             | Credential source |
|----------|------------------------------------------------------|-------------------|
| Claude   | `GET api.anthropic.com/api/oauth/usage`              | `%USERPROFILE%\.claude\.credentials.json`, else a configured WSL Claude credential file via `wsl.exe`. Refreshes on 401; 429 → cooldown (`retry-after` + 30s, else 30 min). If Claude's web OAuth refresh is blocked, the UI reports `REFRESH BLOCKED`; an optional Claude Code fallback can be enabled in config. |
| Codex    | `GET chatgpt.com/backend-api/wham/usage`             | `%USERPROFILE%\.codex\auth.json` → `tokens.access_token` \| `id_token` |
| DeepSeek | `GET api.deepseek.com/user/balance`                  | Windows Credential Manager target `AiUsageDashboard/DeepSeekApiKey`, else `DEEPSEEK_API_KEY` env, else config |

Refresh: every `refreshIntervalMinutes` (default 30, min 15). Network timeout
15s, one retry on 408/5xx. Each service keeps last-known-good; data older than
6h is marked possibly stale.

## Launch modes (fullscreen / screensaver)

The exe parses the same flags as the WinForms prototype:

| flag           | behavior |
|----------------|----------|
| _(none)_       | normal resizable window |
| `--fullscreen` | borderless fullscreen, cursor hidden; **Esc** quits |
| `/s` or `-s`   | screensaver: fullscreen + always-on-top; **any real input** quits (after a ~1.2s arm delay) |
| `--config` / `/c` | opens `config.json` in the editor and exits |
| `/p <HWND>`    | screensaver preview — intentionally no-ops (does not crash) |

Idle auto-launch (Task Scheduler, recommended over `.scr`):

```powershell
# after npm run app:build
.\install-idle-task.ps1 -IdleMinutes 10
# remove: schtasks.exe /Delete /TN "AI Usage Dashboard Idle" /F
```

Real Windows screensaver (`.scr`, experimental — copies the exe to a `.scr`):

```powershell
.\install-screensaver.ps1 -TimeoutSeconds 900
```

Both scripts default to `src-tauri\target\release\ai-usage-dashboard.exe`; pass
`-ExePath` to point at an installed location instead.

## Config (`%APPDATA%\AiUsageDashboard\config.json`)

```json
{
  "refreshIntervalMinutes": 30,
  "networkTimeoutSeconds": 15,
  "deepSeekApiKey": "",
  "deepSeekCredentialTarget": "AiUsageDashboard/DeepSeekApiKey",
  "claudeCredentialsPath": "",
  "claudeCodeRefreshEnabled": false,
  "claudeCodeCommand": "claude",
  "claudeCodeRefreshTimeoutSeconds": 30,
  "claudeCodeRefreshMaxBudgetUsd": 0.03,
  "codexAuthPath": "",
  "mockMode": ""
}
```

`claudeCredentialsPath` accepts a normal path or a `wsl:<distro>:<path>` spec,
e.g. `wsl:Ubuntu:/home/your-user/.claude/.credentials.json`.

`claudeCodeRefreshEnabled` is off by default because it may spend a small amount
of Claude Code usage. When enabled, the backend uses Claude Code as a last-resort
credential refresher after direct OAuth refresh fails, then reloads the refreshed
credential file and retries the usage request. On Windows, WSL-backed credential
paths run the command through `wsl.exe -d <distro> --`.

## Status — what's done vs. next

**Scaffolded & implemented:** full project structure, the dark dashboard UI,
config/cache/secrets, all three live fetchers, Claude refresh + cooldown, mock
mode, the background refresh loop, the icon set, **and the launch modes**
(`--fullscreen` / `/s` screensaver with exit-on-input / `/c` config / `/p`
preview) plus the idle-task and screensaver install scripts.

**Verified locally:** TypeScript type-checking and Rust `cargo check` pass on
the source tree. Windows installer packaging still needs to be rebuilt on a
Windows host before publishing binaries.

**Then:** rebuild the Windows app bundle, walk the three mock states, then verify
live credential reads (Claude via WSL, Codex `auth.json`, DeepSeek Credential
Manager).
