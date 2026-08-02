# Linux packaging notes

Release builds on `ubuntu-latest` produce:

| Asset | Notes |
| --- | --- |
| `AI-Usage-Dashboard_${version}_amd64.deb` | Debian/Ubuntu; declares WebKitGTK / GTK runtime depends |
| `AI-Usage-Dashboard_${version}_amd64.AppImage` | Distro-portable fallback |

Users install with:

```sh
curl -fsSL https://github.com/neyham/ai-usage-dashboard/releases/latest/download/install-linux.sh | sh
```

## Source build dependencies (Ubuntu 24.04)

```sh
sudo apt-get install -y \
  build-essential curl wget file libssl-dev libgtk-3-dev \
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf pkg-config
```

Plus Node.js 18+ and Rust 1.88+.

## Config and cache paths

| Kind | Path |
| --- | --- |
| Config | `~/.config/AiUsageDashboard/config.json` |
| Cache | `~/.local/share/AiUsageDashboard/state.json` |
| DeepSeek key | Display Settings / `deepSeekApiKey`, then `DEEPSEEK_API_KEY` |
