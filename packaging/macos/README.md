# macOS packaging notes

Release builds on `macos-latest` produce:

| Asset | Notes |
| --- | --- |
| `AI-Usage-Dashboard_${version}_aarch64.dmg` | Apple Silicon |
| `AI-Usage-Dashboard_${version}_x64.dmg` | Intel (when the release matrix includes it) |

Users install with:

```sh
curl -fsSL https://github.com/neyham/ai-usage-dashboard/releases/latest/download/install-macos.sh | sh
```

Install destination defaults to `/Applications`. Override with
`APPLICATIONS_DIR="$HOME/Applications"`.

## Signing

Release DMGs are unsigned unless Apple Developer credentials are configured in
CI. Gatekeeper may require **Open Anyway** on first launch.

## Config and cache paths

| Kind | Path |
| --- | --- |
| Config | `~/Library/Application Support/AiUsageDashboard/config.json` (via `dirs::config_dir`) |
| Cache | `~/Library/Application Support/AiUsageDashboard/state.json` (via `dirs::data_local_dir`) |
| DeepSeek key | macOS Keychain via `keyring`, then `DEEPSEEK_API_KEY` |
