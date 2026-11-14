#!/bin/sh
# Beamlogin installer for Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/UnoSee/beamlogin/main/install.sh | sh
#
# Detects the system, downloads the matching binary from GitHub Releases, checks it against
# SHA256SUMS and installs it as `beamlogin`.
#
# Settings (environment variables):
#   BEAMLOGIN_VERSION       release tag to install, such as v0.4.0-beta.1 (default: newest)
#   BEAMLOGIN_INSTALL_DIR   folder to install into (default: /usr/local/bin as root,
#                           otherwise ~/.local/bin)
#
# Written for plain POSIX sh, so it also runs on busybox and minimal live systems.
set -eu

REPO="UnoSee/beamlogin"

# Everything is inside main(), so a download cut off halfway runs nothing.
main() {
    if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
        BOLD="$(printf '\033[1m')"; DIM="$(printf '\033[2m')"; RED="$(printf '\033[31m')"
        GREEN="$(printf '\033[32m')"; RESET="$(printf '\033[0m')"
    else
        BOLD=""; DIM=""; RED=""; GREEN=""; RESET=""
    fi
    # The Linux text console and non-UTF-8 locales can't show check marks.
    OK="✓"; BAD="✗"
    case "${LC_ALL:-}${LC_CTYPE:-}${LANG:-}" in *[Uu][Tt][Ff]-8* | *[Uu][Tt][Ff]8*) ;; *) OK="+"; BAD="x" ;; esac
    [ "${TERM:-}" = "linux" ] && { OK="+"; BAD="x"; }

    # --- what system is this? ---------------------------------------------------------------
    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os" in
        Linux) os="linux" ;;
        Darwin) os="macos" ;;
        MINGW* | MSYS* | CYGWIN*)
            fail "This is Windows. In PowerShell, run instead:" \
                "  irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
        *) fail "Beamlogin has no build for $os yet. Builds exist for Linux, macOS and Windows." ;;
    esac
    case "$arch" in
        x86_64 | amd64) arch="x86_64" ;;
        aarch64 | arm64 | armv8*) arch="aarch64" ;;
        *) fail "Beamlogin has no build for the $arch processor yet." \
                "Builds exist for x86_64 and 64-bit ARM (aarch64)." ;;
    esac
    # A Mac with Apple Silicon reports x86_64 when this shell runs under Rosetta.
    if [ "$os" = "macos" ] && [ "$arch" = "x86_64" ] \
        && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then
        arch="aarch64"
    fi
    # A 64-bit ARM processor running a 32-bit Linux can't run the 64-bit build.
    if [ "$os" = "linux" ] && [ "$arch" = "aarch64" ] && command -v getconf >/dev/null 2>&1 \
        && [ "$(getconf LONG_BIT 2>/dev/null || echo 64)" = "32" ]; then
        fail "This is a 32-bit ARM system. Beamlogin needs a 64-bit OS on ARM."
    fi
    asset="beamlogin-$arch-$os"

    # --- tools ------------------------------------------------------------------------------
    if command -v curl >/dev/null 2>&1; then
        fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
        fetch_text() { curl -fsSL --retry 3 "$1"; }
    elif command -v wget >/dev/null 2>&1; then
        fetch() { wget -q -O "$2" "$1"; }
        fetch_text() { wget -q -O - "$1"; }
    else
        fail "Neither curl nor wget is installed. Install one of them and try again."
    fi

    # --- which release? ---------------------------------------------------------------------
    version="${BEAMLOGIN_VERSION:-}"
    if [ -z "$version" ]; then
        # The newest release, including pre-releases (GitHub's "latest" skips those).
        version="$(fetch_text "https://api.github.com/repos/$REPO/releases?per_page=1" 2>/dev/null \
            | grep '"tag_name"' | head -n 1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' || true)"
        [ -n "$version" ] || fail "Couldn't find the newest release on GitHub." \
            "Check the internet connection, or set BEAMLOGIN_VERSION, for example:" \
            "  curl -fsSL https://raw.githubusercontent.com/$REPO/main/install.sh | BEAMLOGIN_VERSION=v0.4.0-beta.1 sh"
    fi
    base="https://github.com/$REPO/releases/download/$version"

    # --- where to? --------------------------------------------------------------------------
    if [ -n "${BEAMLOGIN_INSTALL_DIR:-}" ]; then
        dir="$BEAMLOGIN_INSTALL_DIR"
    elif [ "$(id -u)" = "0" ]; then
        dir="/usr/local/bin"
    else
        [ -n "${HOME:-}" ] || fail "HOME isn't set. Set BEAMLOGIN_INSTALL_DIR to choose a folder."
        dir="$HOME/.local/bin"
    fi

    say ""
    say "  ${BOLD}Beamlogin installer${RESET}"
    say "  ${DIM}System${RESET}   $os $arch"
    say "  ${DIM}Release${RESET}  $version"
    say "  ${DIM}Into${RESET}     $dir"
    say ""

    # --- download and check -----------------------------------------------------------------
    tmp="$(mktemp -d 2>/dev/null || mktemp -d -t beamlogin)"
    trap 'rm -rf "$tmp"' EXIT INT TERM
    step "Downloading $asset"
    fetch "$base/$asset" "$tmp/$asset" \
        || fail "Download failed: $base/$asset" \
            "Check the internet connection, and that release $version exists:" \
            "  https://github.com/$REPO/releases"

    if fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" 2>/dev/null; then
        want="$(grep " $asset\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)"
        if command -v sha256sum >/dev/null 2>&1; then
            got="$(sha256sum "$tmp/$asset" | cut -d' ' -f1)"
        elif command -v shasum >/dev/null 2>&1; then
            got="$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)"
        else
            got=""
        fi
        if [ -z "$want" ]; then
            warn "SHA256SUMS has no entry for $asset, so the download wasn't checked."
        elif [ -z "$got" ]; then
            warn "No sha256sum or shasum here, so the download wasn't checked."
        elif [ "$want" != "$got" ]; then
            fail "The download doesn't match its checksum, so it wasn't installed." \
                "Try again. If it keeps happening, please report it."
        else
            step "Checksum verified"
        fi
    else
        warn "This release has no SHA256SUMS file, so the download wasn't checked."
    fi

    chmod +x "$tmp/$asset"
    "$tmp/$asset" --version >/dev/null 2>&1 \
        || fail "The downloaded program doesn't run on this system ($os $arch)."

    # --- install ----------------------------------------------------------------------------
    mkdir -p "$dir" 2>/dev/null || fail "Couldn't create $dir." \
        "Run as root, or choose a folder with BEAMLOGIN_INSTALL_DIR."
    [ -w "$dir" ] || fail "You can't write to $dir." \
        "Run as root, or choose a folder with BEAMLOGIN_INSTALL_DIR."
    # Move into place atomically, so a running copy is never half-written.
    cp "$tmp/$asset" "$dir/.beamlogin.new"
    mv -f "$dir/.beamlogin.new" "$dir/beamlogin"
    installed="$("$dir/beamlogin" --version 2>/dev/null || echo beamlogin)"
    step "Installed ${BOLD}$installed${RESET} to $dir/beamlogin"

    # --- PATH -------------------------------------------------------------------------------
    case ":${PATH:-}:" in
        *":$dir:"*)
            say ""
            say "  ${GREEN}Done.${RESET} Run ${BOLD}beamlogin${RESET} to sign in to Claude Code with your phone."
            ;;
        *)
            # These are shown for the user to paste, so ~ stays literal here.
            # shellcheck disable=SC2088
            case "${SHELL:-}" in
                */zsh) rc="~/.zshrc" ;;
                */bash) if [ "$os" = "macos" ]; then rc="~/.bash_profile"; else rc="~/.bashrc"; fi ;;
                *) rc="~/.profile" ;;
            esac
            pdir="$dir"
            [ -n "${HOME:-}" ] && case "$dir" in "$HOME"/*) pdir="\$HOME${dir#"$HOME"}" ;; esac
            say ""
            say "  ${GREEN}Done.${RESET} $dir isn't on your PATH yet. Add it with:"
            say ""
            say "    ${BOLD}echo 'export PATH=\"$pdir:\$PATH\"' >> $rc && export PATH=\"$pdir:\$PATH\"${RESET}"
            say ""
            say "  Or run it directly: ${BOLD}$dir/beamlogin${RESET}"
            ;;
    esac
    say ""
}

say() { printf '%s\n' "$*"; }
step() { printf '  %s%s%s %s\n' "$GREEN" "$OK" "$RESET" "$*"; }
warn() { printf '  %s!%s %s\n' "$BOLD" "$RESET" "$*" >&2; }
fail() {
    printf '\n  %s%s %s%s\n' "${RED:-}" "${BAD:-x}" "$1" "${RESET:-}" >&2
    shift
    for line in "$@"; do printf '    %s\n' "$line" >&2; done
    printf '\n' >&2
    exit 1
}

main "$@"
