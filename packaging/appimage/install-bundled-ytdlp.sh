#!/usr/bin/env bash
# Sourced by AppRun before loading bundled GTK libraries.
# Install the verified, bundled helper offline; preserve existing user tools.
ldm_install_bundled_ytdlp() {
    local bundle="$1" destination="${HOME:?HOME is required}/.local/bin/yt-dlp" temporary
    if [[ ! -e "$destination" && ! -L "$destination" ]]; then
        mkdir -p -- "$(dirname "$destination")" || return 1
        temporary=$(mktemp "${destination}.install.XXXXXXXX") || return 1
        if ! cp -- "$bundle/usr/lib/ldm-tools/yt-dlp" "$temporary" || ! chmod 755 "$temporary"; then
            rm -f -- "$temporary"
            return 1
        fi
        # A hard link publishes the complete file atomically without replacing
        # a user tool or another instance's installation.
        if ! ln -- "$temporary" "$destination" 2>/dev/null; then
            if [[ ! -x "$destination" ]]; then
                rm -f -- "$temporary"
                return 1
            fi
        fi
        rm -f -- "$temporary"
    fi
    [[ -x "$destination" ]] || {
        printf 'LDM: %s exists but is not executable; repair or remove it.\n' "$destination" >&2
        return 1
    }
    export PATH="$HOME/.local/bin:${PATH:-/usr/local/bin:/usr/bin:/bin}"
}

ldm_install_bundled_ytdlp "$this_dir" || {
    printf 'LDM: bundled yt-dlp could not be installed in ~/.local/bin.\n' >&2
    exit 1
}
