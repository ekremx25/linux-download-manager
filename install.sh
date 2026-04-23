#!/usr/bin/env bash
# Linux Download Manager — one-click installer
#
# Builds the Rust binary, places it under ~/.local/bin, installs a desktop
# entry + icon, drops the browser extension into ~/Documents/, writes the
# Chrome/Brave native messaging host JSONs.
#
# Run from the repo root:
#     ./install.sh
#
# Or one-shot (no prior clone):
#     curl -fsSL https://raw.githubusercontent.com/ekremx25/linux-download-manager/main/install.sh | bash
set -euo pipefail

# ── Colour helpers ────────────────────────────────────────────────────────────
if [[ -t 1 ]]; then
  BOLD=$'\e[1m'; DIM=$'\e[2m'; GREEN=$'\e[32m'; YELLOW=$'\e[33m'; RED=$'\e[31m'; RESET=$'\e[0m'
else
  BOLD=""; DIM=""; GREEN=""; YELLOW=""; RED=""; RESET=""
fi
log()    { echo -e "${BOLD}${GREEN}==>${RESET} $*"; }
info()   { echo -e "${DIM}    $*${RESET}"; }
warn()   { echo -e "${BOLD}${YELLOW}!!${RESET}  $*" >&2; }
die()    { echo -e "${BOLD}${RED}✗${RESET}  $*" >&2; exit 1; }

# ── Paths ─────────────────────────────────────────────────────────────────────
BIN_DIR="${HOME}/.local/bin"
APP_DATA_DIR="${HOME}/.local/share/linux-download-manager"
DESKTOP_DIR="${HOME}/.local/share/applications"
ICON_DIR="${HOME}/.local/share/icons/hicolor/256x256/apps"
EXT_DIR="${HOME}/Documents/Linux Download Manager Extension"

NATIVE_HOST_NAME="com.eko.linuxdownloadmanager"
NATIVE_HOST_BIN="${APP_DATA_DIR}/bin/browser_native_host"

# ── Detect mode: local repo or remote curl|bash ──────────────────────────────
# If this file lives inside a git/source checkout, reuse it. Otherwise clone
# into a temporary directory.
if [[ -n "${BASH_SOURCE[0]:-}" && -f "${BASH_SOURCE[0]}" ]]; then
  REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
else
  REPO_DIR=""
fi

if [[ -z "${REPO_DIR}" || ! -f "${REPO_DIR}/src-tauri/Cargo.toml" ]]; then
  log "Fetching source…"
  tmp_dir="$(mktemp -d)"
  git clone --depth 1 https://github.com/ekremx25/linux-download-manager "${tmp_dir}/ldm" \
    || die "git clone failed — is git installed and do you have network access?"
  REPO_DIR="${tmp_dir}/ldm"
fi

cd "${REPO_DIR}"
log "Source root: ${REPO_DIR}"

# ── Prerequisite: cargo ───────────────────────────────────────────────────────
ensure_cargo() {
  if command -v cargo >/dev/null 2>&1; then
    return 0
  fi
  warn "cargo not found. Install rust first:"
  if command -v pacman >/dev/null 2>&1; then
    info "  sudo pacman -S rust     # Arch/Manjaro"
  elif command -v apt >/dev/null 2>&1; then
    info "  sudo apt install cargo  # Debian/Ubuntu"
  elif command -v dnf >/dev/null 2>&1; then
    info "  sudo dnf install rust cargo  # Fedora"
  else
    info "  https://rustup.rs (any distro)"
  fi
  die "Install rust/cargo and re-run this script."
}

# ── Prerequisite: system libs (webkit2gtk-4.1 etc.) ─────────────────────────
ensure_system_libs() {
  local missing=()
  pkg-config --exists webkit2gtk-4.1 2>/dev/null || missing+=("webkit2gtk-4.1")
  pkg-config --exists gtk+-3.0         2>/dev/null || missing+=("gtk3")
  pkg-config --exists javascriptcoregtk-4.1 2>/dev/null || missing+=("javascriptcoregtk-4.1")
  pkg-config --exists libsoup-3.0     2>/dev/null || missing+=("libsoup3")

  if [[ ${#missing[@]} -eq 0 ]]; then
    return 0
  fi

  warn "Missing system libraries: ${missing[*]}"
  if command -v pacman >/dev/null 2>&1; then
    info "  sudo pacman -S webkit2gtk-4.1 gtk3 libsoup3 pkgconf"
  elif command -v apt >/dev/null 2>&1; then
    info "  sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev pkg-config"
  elif command -v dnf >/dev/null 2>&1; then
    info "  sudo dnf install webkit2gtk4.1-devel gtk3-devel libsoup3-devel pkgconf"
  fi
  die "Install the libraries above and re-run this script."
}

# ── Build ─────────────────────────────────────────────────────────────────────
build_binary() {
  log "Building release binary (this takes 1–2 minutes on first run)…"
  PATH="${HOME}/.cargo/bin:${PATH}" cargo build --release --manifest-path src-tauri/Cargo.toml >/dev/null \
    || die "cargo build failed. Re-run with: cargo build --release --manifest-path src-tauri/Cargo.toml"
}

# ── Install all the pieces ───────────────────────────────────────────────────
install_binary() {
  log "Installing binary → ${BIN_DIR}/linux-download-manager"
  mkdir -p "${BIN_DIR}" "${APP_DATA_DIR}/bin"
  install -m 0755 "target/release/linux-download-manager" "${BIN_DIR}/linux-download-manager"
  install -m 0755 "target/release/browser_native_host"    "${NATIVE_HOST_BIN}"
}

install_extension() {
  log "Copying browser extension → ${EXT_DIR}"
  mkdir -p "${EXT_DIR}"
  cp browser/chromium/manifest.json     "${EXT_DIR}/"
  cp browser/chromium/service-worker.js "${EXT_DIR}/"
  cp browser/chromium/content-script.js "${EXT_DIR}/"
  cp browser/chromium/content-style.css "${EXT_DIR}/"
}

install_desktop_entry() {
  log "Installing desktop entry + icon"
  mkdir -p "${DESKTOP_DIR}" "${ICON_DIR}"
  install -m 0644 "src-tauri/icons/icon.png" "${ICON_DIR}/linux-download-manager.png"

  cat > "${DESKTOP_DIR}/linux-download-manager.desktop" <<EOF
[Desktop Entry]
Version=1.0
Type=Application
Name=Linux Download Manager
Comment=IDM-inspired download manager with YouTube/Facebook/Twitter/Reddit support
Exec=${BIN_DIR}/linux-download-manager %U
Icon=linux-download-manager
StartupWMClass=linux-download-manager
Terminal=false
Categories=Network;FileTransfer;
EOF

  command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor/" 2>/dev/null || true
}

install_native_host() {
  log "Writing Chromium native messaging manifest"
  local browsers=(
    "${HOME}/.config/google-chrome/NativeMessagingHosts"
    "${HOME}/.config/chromium/NativeMessagingHosts"
    "${HOME}/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts"
    "${HOME}/.config/microsoft-edge/NativeMessagingHosts"
    "${HOME}/.config/vivaldi/NativeMessagingHosts"
  )

  local payload
  payload=$(cat <<EOF
{
  "name": "${NATIVE_HOST_NAME}",
  "description": "Native messaging bridge for Linux Download Manager",
  "path": "${NATIVE_HOST_BIN}",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://unknown/"]
}
EOF
)

  # Only install for browsers the user has actually used (config dir exists).
  for browser_root in "${browsers[@]}"; do
    local parent="$(dirname "${browser_root}")"
    if [[ -d "${parent}" ]]; then
      mkdir -p "${browser_root}"
      echo "${payload}" > "${browser_root}/${NATIVE_HOST_NAME}.json"
      info "    → $(basename "${parent}")"
    fi
  done
}

# ── Post-install hint ─────────────────────────────────────────────────────────
final_instructions() {
  echo
  log "${BOLD}Installed. ${RESET}"
  echo
  echo "  Launch:"
  echo "     From your app menu → ${BOLD}Linux Download Manager${RESET}"
  echo "     Or in a terminal   → ${BOLD}linux-download-manager${RESET}"
  echo
  echo "  ${BOLD}One manual step left — load the browser extension:${RESET}"
  echo "     1. Open ${BOLD}chrome://extensions${RESET} (or brave://extensions)"
  echo "     2. Toggle ${BOLD}Developer mode${RESET} (top right)"
  echo "     3. Click ${BOLD}Load unpacked${RESET}"
  echo "     4. Select: ${BOLD}${EXT_DIR}${RESET}"
  echo "     5. Copy the extension ID shown under the extension name"
  echo "     6. Replace 'unknown' with that ID in the JSON files under"
  echo "        ${DIM}~/.config/<browser>/NativeMessagingHosts/${NATIVE_HOST_NAME}.json${RESET}"
  echo
  echo "  Uninstall: ${BOLD}./uninstall.sh${RESET}"
  echo
}

# ── Run ───────────────────────────────────────────────────────────────────────
ensure_cargo
ensure_system_libs
build_binary
install_binary
install_extension
install_desktop_entry
install_native_host
final_instructions
