# Linux Download Manager — IDM alternative for Linux

<p align="center">
  <img src="src-tauri/icons/icon.png" width="128" alt="Linux Download Manager icon">
</p>

<p align="center">
  <strong>Fast, IDM-inspired download manager for Arch, Ubuntu, Fedora. Built with Rust + Tauri. YouTube, Twitter/X, Reddit, TikTok support via yt-dlp.</strong>
</p>

<p align="center">
  <a href="https://github.com/ekremx25/linux-download-manager/releases/latest"><b>⬇ Download AppImage</b></a>
  &nbsp;·&nbsp;
  <a href="#install">Install from source</a>
  &nbsp;·&nbsp;
  <a href="#troubleshooting">Troubleshooting</a>
</p>

<p align="center">
  <a href="https://github.com/ekremx25/linux-download-manager/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/ekremx25/linux-download-manager?label=release&color=17A090"></a>
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg">
  <img alt="Platform: Linux" src="https://img.shields.io/badge/platform-Linux-1f6feb.svg">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-orange.svg">
</p>

<details>
<summary>Türkçe açıklama</summary>

**Linux için IDM tarzı indirme yöneticisi.** Rust + Tauri ile yazıldı, Chromium tarayıcı eklentisi üzerinden çalışır. YouTube, Twitter, Reddit, TikTok video indirme desteği `yt-dlp` ile. Arch, Ubuntu, Fedora üzerinde AppImage olarak çalışır. Açık kaynak, ücretsiz, **IDM alternatifi**.

Tek komutla kurulum:
```bash
curl -fsSL https://raw.githubusercontent.com/ekremx25/linux-download-manager/main/install.sh | bash
```

</details>

---

## What is this?

A lightweight, fast download manager for **Linux** — think **IDM (Internet Download Manager) alternative** for Arch, Ubuntu, Fedora. Integrates directly into Chromium-based browsers (Chrome, Brave, Edge, Vivaldi) through a native messaging bridge and an extension, and lets you download videos from YouTube, Twitter/X, Reddit, TikTok and ~1000 more sites through `yt-dlp` with a single click.

**Why this exists:** Linux never got an official IDM port, and most download-manager alternatives are either abandoned, require Wine, or don't integrate with the browser. This one does — a small Rust binary + a Chromium extension + a Tauri GUI, distributed as a single AppImage.

## Features

### Video Download Support
| Platform | Quality Picker | Auto Audio | Download Button |
|----------|:---:|:---:|:---:|
| **YouTube** | Yes (360p-4K) | Yes | Player overlay |
| **Twitter/X** | Yes | Yes | Inline on tweets |
| **Reddit** | - | Yes | Above video posts |

### Core Features
- **Multi-segment downloads** - Up to 4 parallel segments for faster downloads
- **Pause / Resume / Cancel** - Full download control
- **Bandwidth throttling** - Per-download and global speed limits
- **Scheduled downloads** - Set a time, download starts automatically
- **SHA-256 verification** - Optional checksum validation
- **Queue management** - Configurable concurrent download limit (1-10)
- **Download history** - SQLite-backed, persistent across restarts

### Desktop Integration
- **System tray** - Runs in background, click to show/hide
- **Close to tray** - Window close minimizes to tray instead of quitting
- **Desktop notifications** - Download complete/failed alerts
- **Browser integration** - Chromium extension auto-captures downloads

### Browser Extension
- **Inline download buttons** on YouTube, Twitter, Reddit videos
- **Quality picker** - Choose resolution before downloading
- **Auto-intercept** - Captures browser downloads from supported sites
- **Media detection** - Detects video/audio streams on any page

## Screenshots

### Main Window
Dark-themed UI with download list, progress bars, speed/ETA indicators.

### Browser Integration
LDM button appears directly on video players - one click to download.

## Installation

### AppImage (Recommended)

1. Download `Linux_Download_Manager-x86_64.AppImage` from [Releases](https://github.com/ekremx25/linux-download-manager/releases/latest)
2. Make it executable and run:
   ```bash
   chmod +x Linux_Download_Manager-x86_64.AppImage
   ./Linux_Download_Manager-x86_64.AppImage
   ```
3. **First run** automatically:
   - Installs native messaging host for browser integration
   - Downloads [yt-dlp](https://github.com/yt-dlp/yt-dlp) to `~/.local/bin/`
   - Copies browser extension files
   - Opens `chrome://extensions` with setup instructions

4. **Browser extension setup** (one-time):
   - Go to `chrome://extensions`
   - Enable **Developer mode**
   - Click **Load unpacked** → select `~/.local/share/linux-download-manager/extension/`

### System Requirements
- **OS**: Linux (x86_64)
- **Browser**: Google Chrome, Chromium, or Brave
- **ffmpeg**: Required for HLS/DASH streams
  ```bash
  # Arch Linux
  sudo pacman -S ffmpeg

  # Ubuntu/Debian
  sudo apt install ffmpeg

  # Fedora
  sudo dnf install ffmpeg
  ```

### Build from Source

```bash
# Prerequisites
# Rust 1.85+, Node.js (optional), ffmpeg

# Clone
git clone https://github.com/ekremx25/linux-download-manager.git
cd linux-download-manager

# Build
cargo tauri build --bundles appimage

# Or run in development
cargo tauri dev
```

## How It Works

```
Browser Extension  →  Native Host  →  App (Rust/Tauri)
     (JS)              (stdin/stdout)      ↓
                                      yt-dlp / ffmpeg / HTTP
                                           ↓
                                      ~/Downloads/
```

1. **Browser extension** detects video streams and adds download buttons
2. When clicked, sends URL + page info to the **native messaging host**
3. Native host writes request to **inbox directory**
4. **App** polls inbox, queues download, uses **yt-dlp** (for social media) or **direct HTTP** (for regular files)
5. Downloads with progress tracking, speed calculation, ETA

## Tech Stack

| Component | Technology |
|-----------|-----------|
| Backend | Rust |
| Desktop Framework | Tauri 2 |
| Database | SQLite (rusqlite) |
| HTTP Client | reqwest (async streaming) |
| Video Download | yt-dlp + ffmpeg |
| Browser Extension | Manifest V3 (Chromium) |
| Frontend | Vanilla HTML/CSS/JS |
| Packaging | AppImage |

## Project Structure

```
├── browser/chromium/        # Browser extension
│   ├── manifest.json
│   ├── service-worker.js    # Background script
│   ├── content-script.js    # Page injection (buttons, overlays)
│   └── content-style.css
├── src-tauri/
│   ├── src/
│   │   ├── app.rs           # App state, queue management
│   │   ├── download/mod.rs  # Download engine (HTTP, HLS, yt-dlp)
│   │   ├── commands.rs      # Tauri IPC commands
│   │   ├── browser.rs       # Native messaging inbox
│   │   ├── jobs.rs          # Job queue processing
│   │   ├── storage/mod.rs   # SQLite persistence
│   │   ├── platform.rs      # Linux paths, first-run setup
│   │   └── lib.rs           # Tray menu, window management
│   └── tauri.conf.json
└── ui/                      # Frontend
    ├── index.html
    ├── main.js
    └── styles.css
```

## Configuration

Settings are accessible from the app UI:

| Setting | Default | Description |
|---------|---------|-------------|
| Download directory | `~/Downloads` | Where files are saved |
| Max concurrent downloads | 3 | Parallel download limit |
| Bandwidth limit | Unlimited | Global speed cap (KB/s) |

## License

MIT

## Credits

Built with [Tauri](https://tauri.app/), [yt-dlp](https://github.com/yt-dlp/yt-dlp), and [ffmpeg](https://ffmpeg.org/).
