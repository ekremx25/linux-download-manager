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

# Options are parsed before any network or filesystem changes.
MODE=auto
NO_DEPS=0
CHECK_ONLY=0
SKIP_BUILD=0
UPDATE=0
usage() {
  cat <<'HELP'
Linux Download Manager — per-user installer
  ./install.sh                  Build from source / install the prebuilt package
  ./install.sh --update         Fast-forward a clean Git branch, build and install
  ./install.sh --check          Check requirements without changing anything
  ./install.sh --no-deps        Do not install system packages or download tools
  ./install.sh --prebuilt       Install the prebuilt bin/ files from the package
  ./install.sh --skip-build     Install existing target/release binaries
  ./install.sh --help           Show this help

Installation preserves history and downloads; failed installs are rolled back.
Do not run with sudo. Sudo is only used for missing system dependencies.
HELP
}
for option in "$@"; do
  case "$option" in
    --help|-h) usage; exit 0 ;;
    --check|--dry-run) CHECK_ONLY=1 ;;
    --no-deps) NO_DEPS=1 ;;
    --prebuilt) MODE=prebuilt ;;
    --skip-build) SKIP_BUILD=1 ;;
    --update) UPDATE=1 ;;
    *) echo "Unknown option: $option" >&2; usage; exit 2 ;;
  esac
done
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
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
INSTALL_HOME="${LDM_INSTALL_HOME:-${HOME}}"
BIN_DIR="${INSTALL_HOME}/.local/bin"
CONFIG_DIR="${XDG_CONFIG_HOME:-${INSTALL_HOME}/.config}"
export PATH="${INSTALL_HOME}/.cargo/bin:${BIN_DIR}:${PATH}"
APP_DATA_DIR="${XDG_DATA_HOME:-${INSTALL_HOME}/.local/share}/linux-download-manager"
DESKTOP_DIR="${XDG_DATA_HOME:-${INSTALL_HOME}/.local/share}/applications"
ICON_DIR="${XDG_DATA_HOME:-${INSTALL_HOME}/.local/share}/icons/hicolor/256x256/apps"
EXT_DIR="${INSTALL_HOME}/Documents/Linux Download Manager Extension"
FIREFOX_EXT_DIR="${INSTALL_HOME}/Documents/Linux Download Manager Firefox Extension"
FIREFOX_ZIP="${INSTALL_HOME}/Documents/Linux Download Manager Firefox Extension.zip"
FIREFOX_EXTENSION_ID="ldm-bridge@ekremx25.github.io"

NATIVE_HOST_NAME="com.eko.linuxdownloadmanager"
NATIVE_HOST_BIN="${APP_DATA_DIR}/bin/browser_native_host"
EXTENSION_ID="dhbkcopeagecbkoncdjefnjlcienhlpg"

# ── Detect mode: local repo or remote curl|bash ──────────────────────────────
# If this file lives inside a git/source checkout, reuse it. Otherwise clone
# into a temporary directory.
if [[ -n "${BASH_SOURCE[0]:-}" && -f "${BASH_SOURCE[0]}" ]]; then
  REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
else
  REPO_DIR=""
fi

tmp_dir=""
TRANSACTION_ACTIVE=0
INSTALL_FILES=()
TEMP_FILES=()
cleanup() {
  local result=$?
  trap - EXIT
  if [[ "$TRANSACTION_ACTIVE" == 1 ]]; then
    warn "Installation failed; restoring previous files from $BACKUP_DIR"
    if rollback_installation; then rm -rf -- "$BACKUP_DIR"
    else warn "Rollback incomplete; keep the backup at $BACKUP_DIR for recovery."; fi
  fi
  local file
  for file in "${TEMP_FILES[@]}"; do rm -f -- "$file" || true; done
  if [[ -n "$tmp_dir" && -d "$tmp_dir" ]]; then rm -rf -- "$tmp_dir"; fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
if [[ -z "${REPO_DIR}" || ( ! -f "${REPO_DIR}/src-tauri/Cargo.toml" && ! -d "${REPO_DIR}/bin" ) ]]; then
  if [[ "$CHECK_ONLY" == 1 ]]; then usage; exit 0; fi
  command -v git >/dev/null || die "Install git first."
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
  warn "cargo not found; installing Rust build tools"
  if command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --needed --noconfirm rust || die "Could not install Rust with pacman."
  elif command -v apt >/dev/null 2>&1; then
    sudo apt install -y rustc cargo || die "Could not install Rust with apt."
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y rust cargo || die "Could not install Rust with dnf."
  else
    info "  https://rustup.rs (any distro)"
    die "Install rust/cargo and re-run this script."
  fi
  command -v cargo >/dev/null 2>&1 || die "cargo is still unavailable after installation."
}

# ── Prerequisite: system libs (webkit2gtk-4.1 etc.) ─────────────────────────
ensure_system_libs() {
  local missing=()
  if ! command -v pkg-config >/dev/null 2>&1; then
    missing+=("pkg-config")
  fi
  pkg-config --exists glib-2.0        2>/dev/null || missing+=("glib2")
  pkg-config --exists webkit2gtk-4.1 2>/dev/null || missing+=("webkit2gtk-4.1")
  pkg-config --exists gtk+-3.0         2>/dev/null || missing+=("gtk3")
  pkg-config --exists javascriptcoregtk-4.1 2>/dev/null || missing+=("javascriptcoregtk-4.1")
  pkg-config --exists libsoup-3.0     2>/dev/null || missing+=("libsoup3")

  if [[ ${#missing[@]} -eq 0 ]]; then
    return 0
  fi

  warn "Missing system libraries: ${missing[*]}"
  if command -v pacman >/dev/null 2>&1; then
    sudo pacman -S --needed --noconfirm glib2 webkit2gtk-4.1 gtk3 libsoup3 pkgconf \
      || die "Could not install Arch/CachyOS build libraries."
  elif command -v apt >/dev/null 2>&1; then
    sudo apt install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev pkg-config \
      || die "Could not install Debian/Ubuntu build libraries."
  elif command -v dnf >/dev/null 2>&1; then
    sudo dnf install -y glib2-devel webkit2gtk4.1-devel gtk3-devel libsoup3-devel pkgconf-pkg-config \
      || die "Could not install Fedora build libraries."
  else
    die "Install glib2, GTK3, WebKitGTK 4.1, libsoup3 and pkg-config before building."
  fi
  command -v pkg-config >/dev/null 2>&1 || die "pkg-config is still unavailable."
  for library in glib-2.0 webkit2gtk-4.1 gtk+-3.0 javascriptcoregtk-4.1 libsoup-3.0; do
    pkg-config --exists "${library}" || die "${library} is still unavailable after package installation."
  done
}

# Prebuilt installs need runtime libraries, but no Rust or development headers.
ensure_runtime_libs() {
  command -v ldd >/dev/null || die "Install ldd (glibc tools) before running this package."
  local report
  report="$(ldd "$BINARY_SOURCE/linux-download-manager" 2>&1)" || true
  if [[ "$report" == *"not found"* ]]; then
    log "Installing missing runtime libraries…"
    if command -v pacman >/dev/null 2>&1; then
      sudo pacman -S --needed --noconfirm webkit2gtk-4.1 gtk3
    elif command -v apt >/dev/null 2>&1; then
      sudo apt install -y libwebkit2gtk-4.1-0
    elif command -v dnf >/dev/null 2>&1; then
      sudo dnf install -y webkit2gtk4.1 gtk3
    else
      die "Install GTK3 and WebKitGTK 4.1 runtime libraries, then try again."
    fi
  fi
  for binary in linux-download-manager browser_native_host; do
    report="$(ldd "$BINARY_SOURCE/$binary" 2>&1)" || die "Cannot check package compatibility: $report"
    [[ "$report" != *"not found"* ]] || die "This package is incompatible with your system. Install from source instead. Details: $report"
  done
}

# ── Build ─────────────────────────────────────────────────────────────────────
build_binary() {
  log "Building release binary (this takes 1–2 minutes on first run)…"
  cargo build --release --locked --bins --manifest-path src-tauri/Cargo.toml >/dev/null \
    || die "cargo build failed. Re-run with: cargo build --release --manifest-path src-tauri/Cargo.toml"
}

# ── Install all the pieces ───────────────────────────────────────────────────
install_binary() {
  log "Installing binary → ${BIN_DIR}/linux-download-manager"
  mkdir -p "${BIN_DIR}" "${APP_DATA_DIR}/bin"
  install_atomic 0755 "${BINARY_SOURCE}/linux-download-manager" "${BIN_DIR}/linux-download-manager"
  install_atomic 0755 "${BINARY_SOURCE}/browser_native_host" "${NATIVE_HOST_BIN}"
}

install_extension() {
  log "Copying browser extension → ${EXT_DIR}"
  mkdir -p "${EXT_DIR}"
  for file in manifest.json service-worker.js player-manifest-observer.js youtube-audio-observer.js content-script.js content-style.css popup.html popup.js popup.css; do
    install_atomic 0644 "browser/chromium/$file" "${EXT_DIR}/$file"
  done
  log "Copying Firefox extension → ${FIREFOX_EXT_DIR}"
  mkdir -p "${FIREFOX_EXT_DIR}"
  install_atomic 0644 "browser/firefox/manifest.json" "${FIREFOX_EXT_DIR}/manifest.json"
  for file in service-worker.js player-manifest-observer.js youtube-audio-observer.js content-script.js content-style.css popup.html popup.js popup.css; do
    install_atomic 0644 "browser/chromium/$file" "${FIREFOX_EXT_DIR}/$file"
  done
  local temporary
  temporary="$(mktemp --suffix=.zip "${FIREFOX_ZIP}.new.XXXXXX")"
  # The packager uses output.with_extension("zip.tmp"); retain the unique stem.
  TEMP_FILES+=("$temporary" "$temporary.tmp")
  "${BIN_DIR}/linux-download-manager" --package-firefox-extension "${FIREFOX_EXT_DIR}" "$temporary" \
    || die "Could not create the Firefox extension ZIP file."
  [[ -s "$temporary" ]] || die "Firefox extension ZIP is empty."
  mv -fT -- "$temporary" "$FIREFOX_ZIP"
  info "    Firefox ZIP → ${FIREFOX_ZIP}"
}

install_desktop_entry() {
  log "Installing desktop entry + icon"
  mkdir -p "${DESKTOP_DIR}" "${ICON_DIR}"
  install_atomic 0644 "src-tauri/icons/icon.png" "${ICON_DIR}/linux-download-manager.png"

  write_atomic 0644 "${DESKTOP_DIR}/linux-download-manager.desktop" <<EOF
[Desktop Entry]
Version=1.0
Type=Application
Name=Linux Download Manager
Comment=IDM-inspired download manager with YouTube/Facebook/Twitter/Reddit support
Exec="${BIN_DIR}/linux-download-manager" %U
Icon=linux-download-manager
StartupWMClass=linux-download-manager
Terminal=false
Categories=Network;FileTransfer;
EOF

  command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database "${DESKTOP_DIR}" 2>/dev/null || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -f -t "${INSTALL_HOME}/.local/share/icons/hicolor/" 2>/dev/null || true
}

install_ytdlp_and_deps() {
  log "Installing yt-dlp and its runtime dependencies"

  # yt-dlp itself — drop a single-file binary into ~/.local/bin.
  local ytdlp_path="${BIN_DIR}/yt-dlp" existing_ytdlp
  existing_ytdlp="$(command -v yt-dlp || true)"
  if [[ ! -f "$existing_ytdlp" || ! -x "$existing_ytdlp" ]]; then
    info "    fetching yt-dlp release…"
    local temporary checksums expected actual
    temporary="$(mktemp "${ytdlp_path}.new.XXXXXX")"
    TEMP_FILES+=("$temporary")
    checksums="$(mktemp "${ytdlp_path}.new.XXXXXX")"
    TEMP_FILES+=("$checksums")
    local release="https://github.com/yt-dlp/yt-dlp/releases/latest/download"
    local curl_options=(--fail --location --proto '=https' --proto-redir '=https'
                        --connect-timeout 15 --max-time 180 --retry 2 --retry-max-time 240)
    curl "${curl_options[@]}" --max-filesize 52428800 --output "$temporary" "$release/yt-dlp" \
      || die "yt-dlp download failed; no helper was published."
    curl "${curl_options[@]}" --max-filesize 1048576 --output "$checksums" "$release/SHA2-256SUMS" \
      || die "yt-dlp checksum download failed; no helper was published."
    expected="$(sed -n 's/^\([[:xdigit:]]\{64\}\)  yt-dlp$/\1/p' "$checksums")"
    actual="$(sha256sum "$temporary")"
    actual="${actual%% *}"
    [[ -s "$temporary" && "$expected" =~ ^[[:xdigit:]]{64}$ && "${actual,,}" == "${expected,,}" ]] \
      || die "yt-dlp checksum mismatch; no helper was published."
    # The upstream HTTPS checksum provides integrity, not independent authenticity.
    chmod 0755 "$temporary"
    mv -fT -- "$temporary" "$ytdlp_path"
    info "    yt-dlp → ${ytdlp_path} (upstream HTTPS SHA256 verified)"
  else
    info "    yt-dlp already on PATH — skipping"
  fi

  # ffmpeg is needed for video+audio stream muxing on YouTube.
  if ! command -v ffmpeg >/dev/null 2>&1; then
    warn "ffmpeg not found; installing media support"
    if command -v pacman >/dev/null 2>&1; then
      sudo pacman -S --needed --noconfirm ffmpeg || warn "ffmpeg installation failed."
    elif command -v apt >/dev/null 2>&1; then
      sudo apt install -y ffmpeg || warn "ffmpeg installation failed."
    elif command -v dnf >/dev/null 2>&1; then
      sudo dnf install -y ffmpeg || sudo dnf install -y ffmpeg-free \
        || warn "ffmpeg installation failed."
    fi
  fi

  # yt-dlp needs a JavaScript runtime for YouTube signature extraction (since
  # late 2024). Prefer deno (recommended upstream), fall back to node.
  local js_runtime=""
  if command -v deno >/dev/null 2>&1; then
    js_runtime="deno"
  elif command -v node >/dev/null 2>&1; then
    js_runtime="node"
  else
    warn "No JavaScript runtime found; installing Node.js for yt-dlp"
    if command -v pacman >/dev/null 2>&1; then
      sudo pacman -S --needed --noconfirm nodejs || warn "Node.js installation failed."
    elif command -v apt >/dev/null 2>&1; then
      sudo apt install -y nodejs || warn "Node.js installation failed."
    elif command -v dnf >/dev/null 2>&1; then
      sudo dnf install -y nodejs || warn "Node.js installation failed."
    fi
    if command -v node >/dev/null 2>&1; then
      js_runtime="node"
    else
      warn "YouTube downloads may be degraded until Node.js or Deno is installed."
      return 0
    fi
  fi

  # Runtime options belong to LDM's invocation, not the user's shared config.
  info "    yt-dlp JavaScript runtime available: ${js_runtime}"
}

install_native_host() {
  log "Writing Chromium native messaging manifest"
  local browsers=(
    "${CONFIG_DIR}/google-chrome/NativeMessagingHosts"
    "${CONFIG_DIR}/chromium/NativeMessagingHosts"
    "${CONFIG_DIR}/BraveSoftware/Brave-Browser/NativeMessagingHosts"
    "${CONFIG_DIR}/microsoft-edge-dev/NativeMessagingHosts"
    "${CONFIG_DIR}/microsoft-edge/NativeMessagingHosts"
    "${CONFIG_DIR}/vivaldi/NativeMessagingHosts"
  )

  local payload
  payload=$(cat <<EOF
{
  "name": "${NATIVE_HOST_NAME}",
  "description": "Native messaging bridge for Linux Download Manager",
  "path": "${NATIVE_HOST_BIN}",
  "type": "stdio",
  "allowed_origins": ["chrome-extension://${EXTENSION_ID}/"]
}
EOF
)

  # Only install for browsers the user has actually used (config dir exists).
  for browser_root in "${browsers[@]}"; do
    local parent="$(dirname "${browser_root}")"
    if [[ -d "${parent}" ]]; then
      mkdir -p "${browser_root}"
      write_atomic 0644 "${browser_root}/${NATIVE_HOST_NAME}.json" <<< "$payload"
      info "    → $(basename "${parent}")"
    fi
  done
  if [[ -d "${INSTALL_HOME}/.mozilla" ]] || command -v firefox >/dev/null 2>&1; then
    local firefox_host_dir="${INSTALL_HOME}/.mozilla/native-messaging-hosts"
    mkdir -p "${firefox_host_dir}"
    write_atomic 0644 "${firefox_host_dir}/${NATIVE_HOST_NAME}.json" <<EOF
{
  "name": "${NATIVE_HOST_NAME}",
  "description": "Native messaging bridge for Linux Download Manager",
  "path": "${NATIVE_HOST_BIN}",
  "type": "stdio",
  "allowed_extensions": ["${FIREFOX_EXTENSION_ID}"]
}
EOF
    info "    → Firefox"
  fi
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
  echo "     5. Reload the extension after updating its files"
  echo "  Firefox test: about:debugging → This Firefox → Load Temporary Add-on"
  echo "     Select: ${BOLD}${FIREFOX_ZIP}${RESET}"
  echo "     A signed Firefox add-on is required for installation that survives a restart."
  echo "  Visual guide: ${BOLD}https://github.com/ekremx25/linux-download-manager/blob/main/docs/extension-install-en.md${RESET}"
  echo
  printf '  Uninstall: bash "%s/uninstall.sh"\n' "$APP_DATA_DIR"
  echo
}

# Publish binaries by rename so an already running application is not overwritten.
install_atomic() {
  local mode="$1" source="$2" destination="$3" temporary
  temporary="$(mktemp "${destination}.new.XXXXXX")"
  TEMP_FILES+=("$temporary")
  if ! install -m "$mode" "$source" "$temporary"; then rm -f -- "$temporary"; return 1; fi
  mv -fT -- "$temporary" "$destination"
}

write_atomic() {
  local mode="$1" destination="$2" temporary
  temporary="$(mktemp "${destination}.new.XXXXXX")"
  TEMP_FILES+=("$temporary")
  cat > "$temporary"
  chmod "$mode" "$temporary"
  mv -fT -- "$temporary" "$destination"
}

# Earlier releases kept history under a "-custom" folder; adopt it once.
migrate_legacy_data_dir() {
  local legacy="${APP_DATA_DIR}-custom"
  if [[ ! -e "$APP_DATA_DIR" && -d "$legacy" && ! -L "$legacy" ]]; then
    mv -T -- "$legacy" "$APP_DATA_DIR"
    log "Moved app data to $APP_DATA_DIR"
  fi
}

# Previous files are kept only for rollback and removed once installation succeeds.
backup_installation() {
  BACKUP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/ldm-install.XXXXXX")"
  INSTALL_FILES=("$BIN_DIR/linux-download-manager" "$NATIVE_HOST_BIN" "$BIN_DIR/yt-dlp"
    "$DESKTOP_DIR/linux-download-manager.desktop" "$ICON_DIR/linux-download-manager.png"
    "$APP_DATA_DIR/.setup_done" "$APP_DATA_DIR/uninstall.sh"
    "$FIREFOX_ZIP" "$INSTALL_HOME/.mozilla/native-messaging-hosts/$NATIVE_HOST_NAME.json")
  local file browser
  for file in manifest.json service-worker.js player-manifest-observer.js youtube-audio-observer.js content-script.js content-style.css popup.html popup.js popup.css; do
    INSTALL_FILES+=("$EXT_DIR/$file" "$FIREFOX_EXT_DIR/$file")
  done
  for browser in google-chrome chromium BraveSoftware/Brave-Browser microsoft-edge-dev microsoft-edge vivaldi; do
    INSTALL_FILES+=("$CONFIG_DIR/$browser/NativeMessagingHosts/$NATIVE_HOST_NAME.json")
  done
  for file in "${INSTALL_FILES[@]}"; do
    [[ ! -d "$file" ]] || die "Installation destination is a directory: $file"
    if [[ -e "$file" || -L "$file" ]]; then
      mkdir -p "$BACKUP_DIR$(dirname "$file")"
      cp -a -- "$file" "$BACKUP_DIR$file"
    fi
  done
  # Arm rollback only after every previous file has been saved successfully.
  TRANSACTION_ACTIVE=1
}

rollback_installation() {
  local file failed=0
  for file in "${INSTALL_FILES[@]}"; do
    # Remove only exact owned files, never extension/config directories or user data.
    if ! rm -f -- "$file"; then failed=1; continue; fi
    if [[ -e "$BACKUP_DIR$file" || -L "$BACKUP_DIR$file" ]]; then
      cp -a -- "$BACKUP_DIR$file" "$file" || failed=1
    fi
  done
  return "$failed"
}

[[ "$(uname -s)" == Linux ]] || die "This installer requires Linux."
if [[ "$MODE" == auto && -d "$REPO_DIR/bin" ]]; then MODE=prebuilt; fi
BINARY_SOURCE="${CARGO_TARGET_DIR:-$REPO_DIR/target}/release"
if [[ "$MODE" == prebuilt ]]; then
  [[ "$(uname -m)" == x86_64 ]] || die "The prebuilt package requires x86_64."
  BINARY_SOURCE="$REPO_DIR/bin"
fi
if [[ "$CHECK_ONLY" == 1 ]]; then
  usage
  echo "Source: $REPO_DIR"
  echo "Installation mode: $MODE"
  for tool in cargo pkg-config yt-dlp ffmpeg node deno; do
    if command -v "$tool" >/dev/null; then echo "$tool: available"; else echo "$tool: not found"; fi
  done
  echo "Executable: $BIN_DIR/linux-download-manager"
  exit 0
fi
[[ "$EUID" != 0 ]] || die "Run as your own user, without sudo."
if [[ "$UPDATE" == 1 ]]; then
  [[ "$MODE" != prebuilt ]] || die "Prebuilt packages cannot update through Git; install a newer package."
  git rev-parse --show-toplevel >/dev/null 2>&1 || die "Git repository not found."
  [[ -z "$(git status --porcelain)" ]] || die "Uncommitted changes found. Commit them first; no changes were discarded."
  branch="$(git symbolic-ref --quiet --short HEAD)" || die "Switch to a Git branch first."
  git rev-parse --verify '@{upstream}' >/dev/null 2>&1 || die "This branch has no upstream tracking branch."
  remote="$(git config "branch.$branch.remote")"
  git fetch "$remote"
  git merge --ff-only '@{upstream}' || die "Branches have diverged; automatic update stopped. Local commits were preserved."
  # Re-run the updated installer without --update, preserving the other options.
  args=(); for option in "$@"; do [[ "$option" == --update ]] || args+=("$option"); done
  exec bash "$REPO_DIR/install.sh" "${args[@]}"
fi
if [[ "$MODE" != prebuilt && "$SKIP_BUILD" == 0 ]]; then
  if [[ "$NO_DEPS" == 0 ]]; then ensure_cargo; ensure_system_libs;
  else command -v cargo >/dev/null || die "cargo not found (--no-deps)."; fi
  build_binary
fi
for binary in linux-download-manager browser_native_host; do
  [[ -x "$BINARY_SOURCE/$binary" ]] || die "Missing executable: $BINARY_SOURCE/$binary"
done
for file in manifest.json service-worker.js player-manifest-observer.js youtube-audio-observer.js content-script.js content-style.css popup.html popup.js popup.css; do
  [[ -f "browser/chromium/$file" ]] || die "Missing extension file: $file"
done
[[ -f browser/firefox/manifest.json ]] || die "Missing Firefox extension manifest."
[[ -f src-tauri/icons/icon.png ]] || die "Application icon is missing."
if [[ "$MODE" == prebuilt && -f SHA256SUMS ]]; then
  sha256sum --check --quiet SHA256SUMS || die "Package integrity check failed; nothing was installed."
fi
if [[ "$MODE" == prebuilt && "$NO_DEPS" == 0 ]]; then ensure_runtime_libs; fi
migrate_legacy_data_dir
backup_installation
install_binary
install_extension
install_desktop_entry
install_native_host
if [[ "$NO_DEPS" == 0 ]]; then install_ytdlp_and_deps;
else
  for tool in yt-dlp ffmpeg; do command -v "$tool" >/dev/null || warn "$tool is missing: install it for video downloads."; done
fi
# Match the application's setup marker; avoid overwriting browser settings again on launch.
write_atomic 0644 "$APP_DATA_DIR/.setup_done" <<< "$NATIVE_HOST_BIN"
install_atomic 0755 "$REPO_DIR/uninstall.sh" "$APP_DATA_DIR/uninstall.sh"
final_instructions
echo "Restart the application. Reload the browser extension and refresh the video page."
TRANSACTION_ACTIVE=0
rm -rf -- "$BACKUP_DIR"
