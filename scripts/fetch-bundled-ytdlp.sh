#!/usr/bin/env bash
# Fetch the pinned standalone x86_64 helper at package-build time only.
set -euo pipefail
[[ $# == 1 ]] || { echo "Usage: $0 DESTINATION" >&2; exit 2; }
destination=$(realpath -m -- "$1")
expected=767ba9dce7052aa196f8b12652f433d76de128ba168966fa84b6394d5e6c6795
if [[ -f "$destination" ]] && [[ "$(sha256sum "$destination" | cut -d ' ' -f 1)" == "$expected" ]]; then
  chmod 755 "$destination"
  exit 0
fi
mkdir -p -- "$(dirname -- "$destination")"
temporary=$(mktemp "${destination}.XXXXXXXX")
trap 'rm -f -- "$temporary"' EXIT
curl --fail --location --proto '=https' --proto-redir '=https' --connect-timeout 15 --max-time 180 --retry 2 --max-filesize 52428800 \
  https://github.com/yt-dlp/yt-dlp/releases/download/2026.10.26/yt-dlp_linux -o "$temporary"
actual=$(sha256sum "$temporary")
[[ "${actual%% *}" == "$expected" ]] || { echo 'Bundled yt-dlp checksum mismatch' >&2; exit 1; }
chmod 755 "$temporary"
mv -fT -- "$temporary" "$destination"
