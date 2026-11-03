#!/usr/bin/env bash
# Usage: repack-with-ytdlp.sh INPUT.AppImage OUTPUT.AppImage
# Reuses the released app binary; no Rust rebuild. Needs curl, python3 and bash.
set -euo pipefail
[[ $# == 2 ]] || { echo "Usage: $0 INPUT.AppImage OUTPUT.AppImage" >&2; exit 2; }
source_root=$(cd -- "$(dirname -- "$0")/.." && pwd)
input=$(realpath -- "$1")
output=$(realpath -m -- "$2")
[[ "$input" != "$output" ]] || { echo 'Use a new output filename.' >&2; exit 2; }
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
mkdir -p -- "$(dirname -- "$output")"
cp -- "$input" "$work/original.AppImage"
chmod +x "$work/original.AppImage"
cd "$work"
./original.AppImage --appimage-extract >/dev/null
bash "$source_root/scripts/fetch-bundled-ytdlp.sh" "$work/yt-dlp"
mkdir -p squashfs-root/usr/lib/ldm-tools
install -m 755 yt-dlp squashfs-root/usr/lib/ldm-tools/yt-dlp
install -m 644 "$source_root/packaging/appimage/install-bundled-ytdlp.sh" squashfs-root/apprun-hooks/install-bundled-ytdlp.sh
python3 - <<'PY'
from pathlib import Path
p = Path('squashfs-root/AppRun')
s = p.read_text()
anchor = 'source "$this_dir"/apprun-hooks/"linuxdeploy-plugin-gtk.sh"'
if s.count(anchor) != 1 or 'install-bundled-ytdlp.sh' in s:
    raise SystemExit('Unexpected AppRun layout; refusing to patch')
hook = '''source "$this_dir/apprun-hooks/install-bundled-ytdlp.sh"
if [[ "${1:-}" == "--ldm-check-tools" ]]; then
    exec "$HOME/.local/bin/yt-dlp" --ignore-config --version
fi

'''
p.write_text(s.replace(anchor, hook + anchor))
PY
chmod -R a+rX squashfs-root
curl -fL --retry 2 https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage -o appimagetool.AppImage
chmod +x appimagetool.AppImage
ARCH=x86_64 ./appimagetool.AppImage --appimage-extract-and-run --comp zstd squashfs-root "$output"
chmod +x "$output"
sha256sum "$output" > "$output.sha256"
