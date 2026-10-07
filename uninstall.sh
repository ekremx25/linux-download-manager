#!/usr/bin/env bash
# Remove installed integration only. Download history, backups, files and shared tools stay.
set -euo pipefail
case "${1:-}" in
  --help|-h) echo 'Usage: ./uninstall.sh [--check]. Downloads, history, backups and yt-dlp are preserved.'; exit 0 ;;
  --check) echo 'The app, menu shortcut, LDM extension and native bridge will be removed. History and downloaded files will be kept.'; exit 0 ;;
  '') ;;
  *) echo "Unknown option: $1" >&2; exit 2 ;;
esac
[[ "$EUID" != 0 ]] || { echo 'Run without sudo.' >&2; exit 1; }
install_home="${LDM_INSTALL_HOME:-${HOME}}"
data="${XDG_DATA_HOME:-${install_home}/.local/share}"
rm -f -- "$install_home/.local/bin/linux-download-manager" \
  "$data/applications/linux-download-manager.desktop" \
  "$data/icons/hicolor/256x256/apps/linux-download-manager.png" \
  "$data/linux-download-manager-custom/bin/browser_native_host" \
  "$data/linux-download-manager-custom/.setup_done"
# Only the known extension files are removed, never arbitrary files in the folder.
for file in manifest.json service-worker.js player-manifest-observer.js content-script.js content-style.css; do
  rm -f -- "$install_home/Documents/Linux Download Manager Extension/$file"
done
rmdir "$install_home/Documents/Linux Download Manager Extension" 2>/dev/null || true
for browser in google-chrome chromium BraveSoftware/Brave-Browser microsoft-edge-dev microsoft-edge vivaldi; do
  rm -f -- "$install_home/.config/$browser/NativeMessagingHosts/com.eko.linuxdownloadmanager.json"
done
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" 2>/dev/null || true
echo 'Application uninstalled. Close any running session and remove the LDM browser extension.'
echo "Download history and backups are preserved: $data/linux-download-manager-custom"
