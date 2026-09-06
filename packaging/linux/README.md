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

## Omarchy / Hyprland

`install-linux.sh` falls back to the AppImage on Arch. Two host issues show up
on Omarchy:

1. **Blank window** — the AppImage's Ubuntu WebKit process aborts (`EGL_BAD_PARAMETER`). Extract it and run `squashfs-root/usr/bin/ai-usage-dashboard` with `LD_LIBRARY_PATH=/usr/lib` so Arch `webkit2gtk-4.1` is used.
2. **Tiny type on HiDPI** — Omarchy sets Hyprland `xwayland:force_zero_scaling`. X11 GTK must set `GDK_SCALE` to the monitor scale (`hyprctl monitors`, often `2`). Bundled GTK also fails `gtk_init` on native Wayland; use `GDK_BACKEND=x11` and `DISPLAY=:0`.

Do not copy another machine's `state.json`. A launcher that matches the main
README lives at `~/.local/bin/ai-usage-dashboard` after those steps. It does
not edit Hyprland config.

## Config and cache paths

| Kind | Path |
| --- | --- |
| Config | `~/.config/AiUsageDashboard/config.json` |
| Cache | `~/.local/share/AiUsageDashboard/state.json` |
| DeepSeek key | Display Settings / `deepSeekApiKey`, then `DEEPSEEK_API_KEY` |
