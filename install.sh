#!/usr/bin/env bash
#
# Spotify DX — cross-platform installer.
#
# Downloads the latest (or a pinned-version) Spotify DX binary/APK from
# GitHub Releases and installs it on the current machine without requiring
# a package manager or sudo for the default user-level install.
#
#   curl -fsSL https://raw.githubusercontent.com/MihneaMoso/spotify-dx/master/install.sh | bash
#
# Options (via environment variables):
#   SPOTIFY_DX_VERSION   Install a specific release tag, e.g. SPOTIFY_DX_VERSION=v0.1.9
#                        (default: latest release)
#   SPOTIFY_DX_PREFIX    Install prefix. Linux/macOS default: ~/.local
#   SPOTIFY_DX_DRYRUN    Print what would happen without writing anything.
#
# GitHub Release assets are named (see .github/workflows/release.yml):
#   spotify-dx-<target>.tar.gz               Linux / macOS (unversioned alias)
#   spotify-dx-<target>.zip                  Windows (unversioned alias)
#   app-release-unsigned-signed.apk          Android APK (stable name)

set -euo pipefail

# ---------------------------------------------------------------------------
# Config / constants
# ---------------------------------------------------------------------------
REPO="${SPOTIFY_DX_REPO:-MihneaMoso/spotify-dx}"
API="https://api.github.com/repos/${REPO}"
RAW_VERSION="${SPOTIFY_DX_VERSION:-}"

die() { echo "error: $*" >&2; exit 1; }
# `log` goes to stderr so that a function's *stdout* carries only its real
# return value (e.g. `installed="$(install_desktop ...)"`).
log() { echo "spotify-dx-install: $*" >&2; }

have() { command -v "$1" >/dev/null 2>&1; }

# Extract a value from a GitHub API release body (JSON) with python3, which is
# more robust than sed/grep across minified and pretty-printed payloads.
# Usage: json_get <expr>
#   <expr> is a python expression evaluated against `d` (the parsed dict) read
#   from stdin. e.g. 'd["tag_name"]' or 'next(...)'.
json_get() {
    local expr="$1"
    python3 -c '
import json, sys
d = json.loads(sys.stdin.read())
r = eval(sys.argv[1], {"d": d})
if r is None:
    sys.exit(4)
print(r)
' "$expr"
}

# ---------------------------------------------------------------------------
# Detect OS + architecture + asset suffix for this machine.
# Returns (os, arch).
# ---------------------------------------------------------------------------
detect_target() {
    local os arch
    case "$(uname -s)" in
        Linux)  os=linux ;;
        Darwin) os=macos ;;
        MINGW*|MSYS*|CYGWIN*) os=windows ;;
        *) die "unsupported OS: $(uname -s)" ;;
    esac

    # Termux / Android under Linux.
    if [ -n "${TERMUX_VERSION:-}" ] || grep -qiE 'android' /proc/version 2>/dev/null; then
        os=android
    fi

    case "$(uname -m)" in
        x86_64|amd64)     arch=x86_64 ;;
        aarch64|arm64)    arch=aarch64 ;;
        armv7*)           arch=armv7 ;;
        *)                arch="$(uname -m)" ;;
    esac

    echo "$os|$arch"
}

# ---------------------------------------------------------------------------
# Resolve the release version (tag) to install.
# ---------------------------------------------------------------------------
resolve_version() {
    if [ -n "$RAW_VERSION" ]; then
        case "$RAW_VERSION" in
            v*) echo "$RAW_VERSION" ;;
            *)  echo "v${RAW_VERSION}" ;;
        esac
        return
    fi
    local body
    curl -fsSL -H "Accept: application/vnd.github+json" \
        -H "User-Agent: spotify-dx-install" \
        "$API/releases/latest" \
    | json_get 'd["tag_name"]'
}

# ---------------------------------------------------------------------------
# Install a desktop (Linux/macOS/Windows) binary.
# ---------------------------------------------------------------------------
install_desktop() {
    local os="$1" url="$2" fname="$3" digest="$4" tmp bin src
    log "downloading ${fname} …"

    tmp="$(mktemp -d)"
    # No EXIT trap referencing `$tmp`: after the function returns its `local`
    # vars are unset, so `set -u` would choke. We clean up explicitly below.
    curl -fsSL -o "$tmp/$fname" "$url"

    if [ -n "$digest" ] && have shasum; then
        echo "$digest  $tmp/$fname" | shasum -a 256 -c - >/dev/null \
            || { rm -rf "$tmp"; die "SHA-256 verification failed for ${fname}"; }
        log "checksum verified"
    elif [ -n "$digest" ] && have sha256sum; then
        echo "$digest  $tmp/$fname" | sha256sum -c - >/dev/null \
            || { rm -rf "$tmp"; die "SHA-256 verification failed for ${fname}"; }
        log "checksum verified"
    else
        log "warning: no checksum tool found; skipping verification"
    fi

    case "$os" in
        linux|macos)
            bin="$HOME/.local/bin/spotify-dx"
            [ -n "${SPOTIFY_DX_PREFIX:-}" ] && bin="${SPOTIFY_DX_PREFIX%/}/bin/spotify-dx"
            mkdir -p "$(dirname "$bin")"
            if have tar; then
                tar -xzf "$tmp/$fname" -C "$tmp"
                src="$(find "$tmp" -maxdepth 2 -type f -name 'spotify-dx' | head -n1)"
            else
                src="$tmp/$fname"
            fi
            [ -n "$src" ] && [ -f "$src" ] || { rm -rf "$tmp"; die "could not locate spotify-dx binary in archive"; }
            cp "$src" "$bin"
            chmod +x "$bin"
            rm -rf "$tmp"
            echo "$bin"
            ;;
        windows)
            local dest
            dest="$(cygpath -u "${LOCALAPPDATA:-$HOME/AppData/Local}/Programs/SpotifyDX" 2>/dev/null \
                    || printf '%s' "$HOME/AppData/Local/Programs/SpotifyDX")"
            mkdir -p "$dest"
            # .zip is not gzip: `tar -xzf` fails obscurely on systems whose
            # tar can't auto-detect zip, and the no-tar fallback would copy
            # the zip over as a corrupt "exe". Require unzip explicitly.
            have unzip || { rm -rf "$tmp"; die "unzip is required to install the Windows .zip (found neither a zip-capable tar nor unzip)"; }
            unzip -q -o "$tmp/$fname" -d "$tmp/unzipped"
            src="$(find "$tmp/unzipped" -maxdepth 2 -type f -name 'spotify-dx.exe' | head -n1)"
            [ -n "$src" ] && [ -f "$src" ] || { rm -rf "$tmp"; die "could not locate spotify-dx.exe in archive"; }
            cp "$src" "$dest/spotify-dx.exe"
            rm -rf "$tmp"
            echo "$dest/spotify-dx.exe"
            ;;
    esac
}

# ---------------------------------------------------------------------------
# Android: download the APK (SHA-256 verified like desktop) and tell the
# user how to install it.
# ---------------------------------------------------------------------------
install_android() {
    local url="$2" fname="$3" digest="$4"
    local dest="$HOME/Download"
    [ -d "$dest" ] || mkdir -p "$dest"
    log "downloading ${fname} …"
    curl -fsSL -o "$dest/$fname" "$url"
    # The APK is the privileged artifact here — verify it exactly like the
    # desktop binary (fail closed when no digest or no tool, not open).
    if [ -z "$digest" ]; then
        rm -f "$dest/$fname"
        die "no SHA-256 digest published for ${fname}; refusing unverified APK"
    fi
    if have shasum; then
        echo "$digest  $dest/$fname" | shasum -a 256 -c - >/dev/null \
            || { rm -f "$dest/$fname"; die "SHA-256 verification failed for ${fname}"; }
    elif have sha256sum; then
        echo "$digest  $dest/$fname" | sha256sum -c - >/dev/null \
            || { rm -f "$dest/$fname"; die "SHA-256 verification failed for ${fname}"; }
    else
        rm -f "$dest/$fname"
        die "no checksum tool (shasum/sha256sum) to verify ${fname}; refusing unverified APK"
    fi
    log "checksum verified"
    echo "$dest/$fname"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    have curl || die "curl is required (macOS/Linux usually include it)"
    ! have uname || true

    local target os arch
    target="$(detect_target)"
    os="${target%%|*}"
    arch="${target#*|}"

    log "platform: ${os} (${arch})"

    local tag
    tag="$(resolve_version)"
    log "release: ${tag}"

    local body
    body="$(curl -fsSL -H "Accept: application/vnd.github+json" \
        -H "User-Agent: spotify-dx-install" \
        "$API/releases/tags/$tag")"

    # Pick the platform asset by its name token and resolve its digest.
    local fname url digest
    case "$os" in
        linux)
            # Only x86_64 Linux is published (see release.yml matrix): an
            # ARM machine must fail loudly here instead of downloading an
            # x86_64 binary that can never exec.
            case "$arch" in
                x86_64) fname="spotify-dx-x86_64-unknown-linux-gnu.tar.gz" ;;
                *) die "no Linux ${arch} build published (only x86_64); refusing to install the wrong arch" ;;
            esac
            ;;
        macos)
            case "$arch" in
                aarch64) fname="spotify-dx-aarch64-apple-darwin.tar.gz" ;;
                *)       fname="spotify-dx-x86_64-apple-darwin.tar.gz" ;;
            esac
            ;;
        windows) fname="spotify-dx-x86_64-pc-windows-msvc.zip" ;;
        android) fname="app-release-unsigned-signed.apk" ;;
    esac
    url="$(
        printf '%s' "$body" \
        | json_get "next(a['browser_download_url'] for a in d['assets'] if '$fname' in a['name'])" \
        || true
    )"
    [ -n "$url" ] || die "no release asset matching '${fname}' on ${tag}"
    # The published basename may differ from our token; fetch the digest by the
    # exact URL.
    digest="$(
        printf '%s' "$body" \
        | json_get "next(a['digest'] for a in d['assets'] if a['browser_download_url'] == '$url')" \
        || true
    )"
    # The GitHub `digest` field for a release asset is `sha256:<hex>`; de-prefix
    # it so it can feed `shasum -a 256 -c` / `sha256sum -c`.
    case "$digest" in
        sha256:*) digest="${digest#sha256:}" ;;
    esac

    if [ "${SPOTIFY_DX_DRYRUN:-}" = "1" ]; then
        echo "would download: $url"
        echo "would verify SHA-256: ${digest:-<unknown>}"
        return 0
    fi

    local installed
    if [ "$os" = "android" ]; then
        installed="$(install_android "$os" "$url" "$(basename "$url")" "$digest")"
        log "APK downloaded to: ${installed}"
        log "open it on your device to install (enable 'Install from unknown sources' if prompted)"
        return 0
    fi

    installed="$(install_desktop "$os" "$url" "$(basename "$url")" "$digest")"
    log "installed: ${installed}"

    case "$os" in
        linux|macos)
            case ":$PATH:" in
                *":$HOME/.local/bin:"*) : ;;
                *) log "add ~/.local/bin to your PATH to run 'spotify-dx'" ;;
            esac
            ;;
        windows)
            log "add the program directory to your PATH to run 'spotify-dx'"
            ;;
    esac
}

main "$@"
