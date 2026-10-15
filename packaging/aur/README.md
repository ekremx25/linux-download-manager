# Arch Linux / AUR package

The `linux-download-manager-git` recipe builds the current `main` branch with
Cargo.lock. It installs into `/usr`, managed by pacman, without running
`install.sh`, sudo, or any user setup during packaging.

## Local installation

From a checkout containing the package integration changes:

```bash
cd packaging/aur/linux-download-manager-git
makepkg -si
```

Build as a normal user. `base-devel` and Git must be installed. Dependencies
are resolved by pacman; the first Rust build can take several minutes.

Launch `/usr/bin/linux-download-manager`. First launch registers the native
bridge for the current user and copies the browser extensions to Documents.
Load the Chromium extension from `~/Documents/Linux Download Manager Extension`.
Firefox instructions and signing limitations are documented in the main README.
FFmpeg, yt-dlp and Node are package dependencies and are updated by pacman.
No video tools are downloaded by the packaged application's first-run setup.

If migrating from the source installer, `~/.local/bin/linux-download-manager`
and its desktop entry can still take precedence. Keep downloads and history;
use the absolute `/usr/bin/linux-download-manager` path until you have backed up
and removed the old launcher. Launch the packaged app once and reload the browser
extension so its user-level native messaging manifest points to the packaged host.
Pacman removal leaves user data and copied extensions intact.

## Validation

Run `./scripts/test-aur-package.sh` from the repository root. It checks shell
syntax, desktop metadata, generated SRCINFO and staged package layout using
placeholder executables; it does not replace a full release build or clean-chroot
test. `makepkg --verifysource` verifies source retrieval and desktop-file checksum.

## Publishing to AUR

The GitHub directory is a packaging recipe, not an AUR publication. The maintainer
needs an AUR account with an authorized SSH public key. Never share the private key.
After the application changes have reached GitHub main:

```bash
git clone ssh://aur@aur.archlinux.org/linux-download-manager-git.git aur-publish
cp packaging/aur/linux-download-manager-git/{PKGBUILD,linux-download-manager.desktop} aur-publish/
cd aur-publish
makepkg --printsrcinfo > .SRCINFO
makepkg -si
# Review package contents and run namcap if installed.
git add PKGBUILD .SRCINFO linux-download-manager.desktop
git commit -m 'Initial Linux Download Manager Git package'
git push origin master
```

Only after publication:

```bash
yay -S linux-download-manager-git
```

For VCS update checks use `yay -Syu --devel`. Regenerate `.SRCINFO` after every
PKGBUILD metadata change. Do not commit binaries or the application source tree
to the AUR repository. Source fetching happens in `prepare()`; compilation is
frozen/offline in `build()`. Validate dependency completeness in a clean Arch
chroot before publishing a release of this recipe.
