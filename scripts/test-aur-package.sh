#!/usr/bin/env bash
# Packaging layout smoke test; placeholder executables are not build validation.
set -euo pipefail
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
recipe="$root/packaging/aur/linux-download-manager-git"
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
bash -n "$recipe/PKGBUILD"
desktop-file-validate "$recipe/linux-download-manager.desktop"
(cd "$recipe" && makepkg --printsrcinfo) > "$tmp/.SRCINFO"
cmp "$recipe/.SRCINFO" "$tmp/.SRCINFO"
source "$recipe/PKGBUILD"
export srcdir="$tmp/src" pkgdir="$tmp/pkg"
mkdir -p "$srcdir/target/release" "$pkgdir"
ln -s "$root" "$srcdir/linux-download-manager"
cp "$recipe/linux-download-manager.desktop" "$srcdir/"
for bin in linux-download-manager-custom browser_native_host; do
  printf '#!/bin/sh\nexit 0\n' > "$srcdir/target/release/$bin"
done
package
[[ $(readlink "$pkgdir/usr/bin/linux-download-manager") == /usr/lib/linux-download-manager/linux-download-manager-custom ]]
[[ $(readlink "$pkgdir/usr/lib/linux-download-manager/_up_/browser") == /usr/share/linux-download-manager/browser ]]
for bin in linux-download-manager-custom browser_native_host; do
  [[ -x "$pkgdir/usr/lib/linux-download-manager/$bin" ]]
done
for file in browser/chromium/manifest.json browser/firefox/manifest.json; do
  cmp "$root/$file" "$pkgdir/usr/share/linux-download-manager/$file"
done
cmp "$root/LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
[[ ! -e "$pkgdir/home" && ! -e "$pkgdir/root" ]]
printf 'PASS: PKGBUILD syntax, desktop metadata, SRCINFO, package layout, licenses and browser resources\n'
