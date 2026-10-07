#!/usr/bin/env bash
# Build an archive with a local, prebuilt installer. Does not publish anything.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
case "${1:-}" in
  --skip-build) ;;
  '') CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}" cargo build --release --locked --bins ;;
  --help|-h) echo 'Usage: ./scripts/build-installer.sh [--skip-build]'; exit 0 ;;
  *) echo 'Unknown option' >&2; exit 2 ;;
esac
[[ "$(uname -m)" == x86_64 ]] || { echo 'This package requires x86_64.' >&2; exit 1; }
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' src-tauri/Cargo.toml | head -n 1)"
name="LinuxDownloadManager-${version}-linux-x86_64"
mkdir -p dist
stage="$(mktemp -d)"
trap 'rm -rf -- "$stage"' EXIT
package="$stage/$name"
mkdir -p "$package/bin" "$package/browser/chromium" "$package/src-tauri/icons"
for binary in linux-download-manager-custom browser_native_host; do
  install -m 0755 "target/release/$binary" "$package/bin/$binary"
done
install -m 0755 install.sh uninstall.sh "$package/"
for file in manifest.json service-worker.js player-manifest-observer.js content-script.js content-style.css; do
  install -m 0644 "browser/chromium/$file" "$package/browser/chromium/$file"
done
install -m 0644 src-tauri/icons/icon.png "$package/src-tauri/icons/icon.png"
cat > "$package/Install.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Install Linux Download Manager
Comment=Install or update the application
Exec=sh -c "exec bash \\"\\$(dirname \\"\\$1\\")/Install.sh\\"" ldm-install %k
Icon=system-software-install
Terminal=true
Categories=Utility;
DESKTOP
cat > "$package/Install.sh" <<'LAUNCHER'
#!/usr/bin/env bash
cd "$(dirname "${BASH_SOURCE[0]}")" || exit 1
bash ./install.sh --prebuilt "$@"
result=$?
if [[ -t 0 ]]; then read -r -p "Press Enter to close…" _; fi
exit "$result"
LAUNCHER
chmod +x "$package/Install.sh"
chmod +x "$package/Install.desktop"
cat > "$package/INSTALL.txt" <<'HELP'
Linux Download Manager

1. Extract the archive to a folder before running the installer.
2. Double-click Install.desktop. If prompted, choose to trust/run the launcher.
   Alternatively, open a terminal here and run: bash ./install.sh --prebuilt
3. Open Linux Download Manager from your applications menu.
4. In your browser's Extensions page, enable Developer mode > Load unpacked:
   ~/Documents/Linux Download Manager Extension
   After updating, reload the extension and refresh the video page.

This prebuilt package is for x86_64 Linux. No Rust compiler is required.
GTK3, WebKitGTK 4.1 and compatible system libraries are required. Binaries built
with a newer glibc may not run on older distributions; use install.sh from the
source repository to build on those systems. Video support requires yt-dlp and
ffmpeg.

Update: run install.sh from the newer package.
Uninstall: bash ./uninstall.sh (history and downloaded files are preserved).
Installation backs up existing app/extension files and prints the backup path.
HELP
(cd "$package" && find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS)
archive="$root/dist/$name.tar.gz"
tar -C "$stage" -czf "$archive.tmp" "$name"
mv -f -- "$archive.tmp" "$archive"
sha256sum "$archive" > "$archive.sha256"
printf 'Installer package: %s\n' "$archive"
