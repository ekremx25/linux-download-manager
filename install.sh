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

Installation backs up existing files and preserves history and downloads.
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
export PATH="${INSTALL_HOME}/.cargo/bin:${BIN_DIR}:${PATH}"
APP_DATA_DIR="${XDG_DATA_HOME:-${INSTALL_HOME}/.local/share}/linux-download-manager-custom"
DESKTOP_DIR="${XDG_DATA_HOME:-${INSTALL_HOME}/.local/share}/applications"
ICON_DIR="${XDG_DATA_HOME:-${INSTALL_HOME}/.local/share}/icons/hicolor/256x256/apps"
EXT_DIR="${INSTALL_HOME}/Documents/Linux Download Manager Extension"

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
trap 'if [[ -n "$tmp_dir" && -d "$tmp_dir" ]]; then rm -rf -- "$tmp_dir"; fi' EXIT
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
  install_atomic 0755 "${BINARY_SOURCE}/linux-download-manager-custom" "${BIN_DIR}/linux-download-manager"
  install_atomic 0755 "${BINARY_SOURCE}/browser_native_host" "${NATIVE_HOST_BIN}"
}

install_extension() {
  log "Copying browser extension → ${EXT_DIR}"
  mkdir -p "${EXT_DIR}"
  for file in manifest.json service-worker.js player-manifest-observer.js content-script.js content-style.css; do
    install_atomic 0644 "browser/chromium/$file" "${EXT_DIR}/$file"
  done
}

install_desktop_entry() {
  log "Installing desktop entry + icon"
  mkdir -p "${DESKTOP_DIR}" "${ICON_DIR}"
  install_atomic 0644 "src-tauri/icons/icon.png" "${ICON_DIR}/linux-download-manager.png"

  cat > "${DESKTOP_DIR}/linux-download-manager.desktop" <<EOF
[Desktop Entry]
Version=1.0
Type=Application
Name=Linux Download Manager
Comment=IDM-inspired download manager with YouTube/Facebook/Twitter/Reddit support
Exec="${BIN_DIR}/linux-download-manager" %U
Icon=linux-download-manager
StartupWMClass=linux-download-manager-custom
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
  local ytdlp_path="${BIN_DIR}/yt-dlp"
  if ! command -v yt-dlp >/dev/null 2>&1 && [[ ! -x "${ytdlp_path}" ]]; then
    info "    fetching yt-dlp release…"
    if curl -fL --progress-bar \
        -o "${ytdlp_path}" \
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp"; then
      chmod +x "${ytdlp_path}"
      info "    yt-dlp → ${ytdlp_path} ($(${ytdlp_path} --version 2>/dev/null || echo "??"))"
    else
      warn "yt-dlp download failed. YouTube/social media downloads will be disabled."
    fi
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

  # Persistent yt-dlp config so every invocation (including the one from the
  # LDM binary) uses the detected runtime without needing extra flags.
  local config_dir="${INSTALL_HOME}/.config/yt-dlp"
  local config_file="${config_dir}/config"
  mkdir -p "${config_dir}"
  # Preserve any existing config the user may have hand-tuned, but make sure
  # the js-runtimes line is present and correct.
  # Keep user configuration intact and do not append duplicate options.
  touch "${config_file}"
  if ! grep -q '^--js-runtimes' "${config_file}"; then
    printf '\n--js-runtimes %s\n' "${js_runtime}" >> "${config_file}"
  fi
  if ! grep -q '^--no-mtime' "${config_file}"; then
    printf '\n--no-mtime\n' >> "${config_file}"
  fi
  info "    yt-dlp config → ${config_file} (js-runtime: ${js_runtime})"
}

install_native_host() {
  log "Writing Chromium native messaging manifest"
  local browsers=(
    "${INSTALL_HOME}/.config/google-chrome/NativeMessagingHosts"
    "${INSTALL_HOME}/.config/chromium/NativeMessagingHosts"
    "${INSTALL_HOME}/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts"
    "${INSTALL_HOME}/.config/microsoft-edge-dev/NativeMessagingHosts"
    "${INSTALL_HOME}/.config/microsoft-edge/NativeMessagingHosts"
    "${INSTALL_HOME}/.config/vivaldi/NativeMessagingHosts"
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
  echo "     5. Reload the extension after updating its files"
  echo
  printf '  Uninstall: bash "%s/uninstall.sh"\n' "$APP_DATA_DIR"
  echo
}

# Publish binaries by rename so an already running application is not overwritten.
install_atomic() {
  local mode="$1" source="$2" destination="$3" temporary
  temporary="$(mktemp "${destination}.new.XXXXXX")"
  if ! install -m "$mode" "$source" "$temporary"; then rm -f -- "$temporary"; return 1; fi
  mv -f -- "$temporary" "$destination"
}

backup_installation() {
  BACKUP_DIR="${APP_DATA_DIR}/backups/$(date +%Y%m%d-%H%M%S)-$$"
  mkdir -p "$BACKUP_DIR"
  local file
  for file in "$BIN_DIR/linux-download-manager" "$NATIVE_HOST_BIN" \
      "$DESKTOP_DIR/linux-download-manager.desktop" "$ICON_DIR/linux-download-manager.png" \
      "${INSTALL_HOME}/.config/yt-dlp/config" "$APP_DATA_DIR/.setup_done"; do
    if [[ -f "$file" ]]; then
      mkdir -p "$BACKUP_DIR$(dirname "$file")"
      cp -p -- "$file" "$BACKUP_DIR$file"
    fi
  done
  if [[ -d "$EXT_DIR" ]]; then mkdir -p "$BACKUP_DIR$(dirname "$EXT_DIR")"; cp -a -- "$EXT_DIR" "$BACKUP_DIR$EXT_DIR"; fi
  while IFS= read -r file; do
    mkdir -p "$BACKUP_DIR$(dirname "$file")"; cp -p -- "$file" "$BACKUP_DIR$file"
  done < <(find "${INSTALL_HOME}/.config" -maxdepth 4 -name "${NATIVE_HOST_NAME}.json" -type f 2>/dev/null || true)
  log "Backup: $BACKUP_DIR"
}

[[ "$(uname -s)" == Linux ]] || die "This installer requires Linux."
if [[ "$MODE" == auto && -d "$REPO_DIR/bin" ]]; then MODE=prebuilt; fi
BINARY_SOURCE="$REPO_DIR/target/release"
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
  git tag "backup-ldm-$(date +%Y%m%d-%H%M%S)-$$"
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
for binary in linux-download-manager-custom browser_native_host; do
  [[ -x "$BINARY_SOURCE/$binary" ]] || die "Missing executable: $BINARY_SOURCE/$binary"
done
for file in manifest.json service-worker.js player-manifest-observer.js content-script.js content-style.css; do
  [[ -f "browser/chromium/$file" ]] || die "Missing extension file: $file"
done
[[ -f src-tauri/icons/icon.png ]] || die "Application icon is missing."
if [[ "$MODE" == prebuilt && -f SHA256SUMS ]]; then
  sha256sum --check --quiet SHA256SUMS || die "Package integrity check failed; nothing was installed."
fi
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
printf '%s\n' "$NATIVE_HOST_BIN" > "$APP_DATA_DIR/.setup_done"
install_atomic 0755 "$REPO_DIR/uninstall.sh" "$APP_DATA_DIR/uninstall.sh"
final_instructions
echo "Backup: $BACKUP_DIR"
echo "Restart the application. Reload the browser extension and refresh the video page."
