# Linux Download Manager — IDM alternative for Linux

<p align="center">
  <img src="src-tauri/icons/icon.png" width="128" alt="Linux Download Manager icon">
</p>

<p align="center">
  <strong>Fast, IDM-inspired download manager for Arch, Ubuntu, Fedora. Built with Rust + Tauri. YouTube, Twitter/X, Reddit, TikTok support via yt-dlp.</strong>
</p>

<p align="center">
  <a href="#installation"><b>Install from source</b></a>
  &nbsp;·&nbsp;
  <a href="#troubleshooting">Troubleshooting</a>
</p>

<p align="center">
  <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-blue.svg">
  <img alt="Platform: Linux" src="https://img.shields.io/badge/platform-Linux-1f6feb.svg">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-orange.svg">
</p>

![Linux Download Manager interface with download categories and settings](docs/images/download-manager.png)

---

## What is this?

A lightweight, fast download manager for **Linux** — think **IDM (Internet Download Manager) alternative** for Arch, Ubuntu, Fedora. Integrates directly into Chromium-based browsers (Chrome, Brave, Edge, Vivaldi) through a native messaging bridge and an extension, and lets you download videos from YouTube, Twitter/X, Reddit, TikTok and ~1000 more sites through `yt-dlp` with a single click.

**Why this exists:** Linux never got an official IDM port, and most download-manager alternatives are either abandoned, require Wine, or don't integrate with the browser. This one does — a Rust binary + a Chromium extension + a Tauri GUI, installed for the current user from source.

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
- **Generic video capture** - Detects direct video, HLS and DASH streams on other websites and falls back to `yt-dlp` for supported pages

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

### System Requirements
- **OS**: Linux (x86_64)
- **Browser**: Google Chrome, Chromium, Brave, Edge, or Vivaldi
- **ffmpeg**: Required for HLS/DASH streams
  ```bash
  # Arch Linux / CachyOS
  sudo pacman -S ffmpeg

  # Ubuntu/Debian
  sudo apt install ffmpeg

  # Fedora
  sudo dnf install ffmpeg    # RPM Fusion, or use Fedora's ffmpeg-free
  ```

### Install from Source

```bash
# Clone
git clone https://github.com/ekremx25/linux-download-manager.git
cd linux-download-manager

# Install for the current user (Fedora, Arch, CachyOS, Debian/Ubuntu)
./install.sh
```

The installer checks Rust and Tauri's WebKitGTK build libraries, installs missing packages with `dnf`, `pacman`, or `apt` using `sudo`, builds the app, and installs the browser bridge. On Fedora, `ffmpeg-free` is used if RPM Fusion's `ffmpeg` package is unavailable. On Arch and CachyOS, the same `pacman` packages are used. Loading the unpacked extension in the browser remains a one-time manual step.

To load the browser extension, open `chrome://extensions`, enable Developer mode, choose **Load unpacked**, and select `~/Documents/Linux Download Manager Extension/`.

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
| Packaging | Source installer |

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

## Download progress and performance

Video downloads now read yt-dlp's live, structured progress instead of waiting
for the process to finish. The list and Details window show download speed and
estimated time left. Video and audio may be fetched separately: each estimate
refers to the current transfer, not extraction or the final merge. Unknown totals
and live streams show no invented ETA.

Progress events and SQLite checkpoints are limited to one every 400 ms, with
completion/error status still recorded immediately. The browser extension
coalesces bursts of page mutations into a single pending media scan. Progress
bars update without continuous width animation, and hidden windows skip periodic
refreshes. These changes reduce avoidable work; yt-dlp's site extraction and
normal networking/merging still require CPU.

Verification (from the repository root):

```bash
cargo test --workspace
node scripts/test-progress-ui.cjs
# Optional integration test: installed yt-dlp, only a localhost fixture
cargo test --workspace real_ytdlp_reports_live_speed_and_eta -- --ignored
cargo build --release --locked -p linux-download-manager-custom --bin linux-download-manager-custom
```

After replacing the installed extension's files, reload the extension in the
browser's Extensions page and refresh the video page to activate the change.

## English interface and installer package

The download center shows active and completed downloads. Each file row shows
its transfer speed and estimated time remaining. Filter by status or file type, select files to
pause or resume together, and use the context menu for details or Show in folder.
Removing history preserves downloaded files. Speed and time remaining stay
visible in narrow windows. In the Add download window, **Start download** checks
the link automatically; the separate check button is optional.

Install or reinstall the current source:

```bash
./install.sh --check
./install.sh
```

Update a clean Git checkout with a configured tracking branch:

```bash
./install.sh --update
```

The updater creates a backup Git tag first and stops if there are local changes.
It only performs a fast-forward update; commit local changes before using it.
`--no-deps` skips system package installation and tool downloads. `--skip-build`
installs existing binaries without compiling source changes.

Build the separate installer package:

```bash
./scripts/build-installer.sh
```

Extract `dist/LinuxDownloadManager-0.2.0-linux-x86_64.tar.gz` and launch
**Install.desktop**. Your desktop may ask you to trust the launcher. Alternatively,
run `bash ./install.sh --prebuilt` inside the extracted folder. No Rust compiler
is needed; GTK3, WebKitGTK 4.1 and compatible system libraries are required.
Binaries built on this machine may not run on older glibc versions; install from
source on those distributions.

Installation backs up existing application and extension files under `backups/`
in the application data directory. Uninstallation preserves history, downloads,
backups and shared yt-dlp settings. Reload the browser extension after updating.
Installer tests use isolated directories without touching the real user profile:

```bash
python3 scripts/test-installer.py
node scripts/test-progress-ui.cjs
```

## Reduced CPU overhead

Progress notifications and database snapshots are limited to once per second;
completion and failure states are still recorded immediately. Hidden app windows
cache live progress without repainting and refresh when shown again. The detail
window checks segment information every five seconds while live speed and ETA
continue to arrive through events.

The browser extension skips background-tab scans, coalesces repeated page
changes, and checks idle pages every five seconds. On returning to a tab it
refreshes media discovery. Download controls may take about a second to appear
after a page change. Reload the extension and refresh video pages after updating.
These changes reduce bookkeeping; network encryption, video extraction and
audio/video merging still require CPU. No transfer speed cap is introduced.
