# Linux Download Manager

<p align="center">
  <img src="src-tauri/icons/icon.png" width="128" alt="LDM Icon">
</p>

<p align="center">
  <strong>Linux-first IDM-inspired download manager with social media video support</strong>
</p>

<p align="center">
  <a href="https://github.com/ekremx25/linux-download-manager/releases/latest">Download AppImage</a>
</p>

---

## What is this?

A lightweight, fast download manager built with **Rust + Tauri** for Linux. It integrates directly into your browser via a Chromium extension and lets you download videos from popular platforms with a single click.

## Features

### Video Download Support
| Platform | Quality Picker | Auto Audio | Download Button |
|----------|:---:|:---:|:---:|
| **YouTube** | Yes (360p-4K) | Yes | Player overlay |
| **Facebook** | Yes | Yes (yt-dlp) | Auto-intercept |
| **Twitter/X** | Yes | Yes | Inline on tweets |
| **Reddit** | - | Yes | Above video posts |
| **Instagram** | Yes | Yes | Auto-intercept |

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
