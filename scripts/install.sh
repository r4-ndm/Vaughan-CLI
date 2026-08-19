#!/usr/bin/env sh
# Install the Vaughan wallet CLI/TUI into ~/.local/bin (or $VAUGHAN_INSTALL_DIR).
#
# Downloads a release tarball from GitHub, verifies SHA256, and installs the
# `vaughan` binary. Falls back to `cargo install` when no release exists yet.
#
# Quick install:
#   curl -fsSL https://raw.githubusercontent.com/r4-ndm/Vaughan-CLI/main/scripts/install.sh | sh
#
# Pin a version:
#   VAUGHAN_VERSION=v0.1.0 curl -fsSL …/install.sh | sh

set -eu

REPO="${VAUGHAN_REPO:-r4-ndm/Vaughan-CLI}"
INSTALL_DIR="${VAUGHAN_INSTALL_DIR:-${HOME}/.local/bin}"
BASE_URL="https://github.com/${REPO}"

say() {
    printf 'vaughan-install: %s\n' "$1"
}

err() {
    say "error: $1" >&2
    exit 1
}

detect_platform() {
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)
    case "$arch" in
        x86_64 | amd64) arch=x86_64 ;;
        aarch64 | arm64) arch=aarch64 ;;
        *) err "unsupported CPU architecture: $arch (need x86_64 or aarch64)" ;;
    esac
    case "$os" in
        linux) printf 'linux-%s' "$arch" ;;
        darwin) printf 'macos-%s' "$arch" ;;
        *) err "unsupported OS: $os (need Linux or macOS)" ;;
    esac
}

sha256_verify() {
    file=$1
    expected=$2
    if command -v sha256sum >/dev/null 2>&1; then
        printf '%s  %s\n' "$expected" "$file" | sha256sum -c - >/dev/null 2>&1
    elif command -v shasum >/dev/null 2>&1; then
        printf '%s  %s\n' "$expected" "$file" | shasum -a 256 -c - >/dev/null 2>&1
    else
        err "need sha256sum or shasum to verify downloads"
    fi
}

resolve_version() {
    if [ -n "${VAUGHAN_VERSION:-}" ]; then
        printf '%s' "$VAUGHAN_VERSION"
        return 0
    fi
    tag=$(
        curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
            | grep '"tag_name":' \
            | head -1 \
            | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/' \
            || true
    )
    if [ -z "$tag" ]; then
        return 1
    fi
    printf '%s' "$tag"
}

install_from_tarball() {
    platform=$1
    version=$2
    archive="vaughan-${platform}.tar.gz"
    url="${BASE_URL}/releases/download/${version}/${archive}"
    sums_url="${BASE_URL}/releases/download/${version}/SHA256SUMS"
    tmpdir=$(mktemp -d)
    # shellcheck disable=SC2064
    trap 'rm -rf "$tmpdir"' EXIT INT HUP TERM

    say "downloading ${archive} (${version})"
    if ! curl -fsSL -o "${tmpdir}/${archive}" "$url"; then
        return 1
    fi

    say "fetching checksums"
    if ! curl -fsSL -o "${tmpdir}/SHA256SUMS" "$sums_url"; then
        return 1
    fi
    expected=$(
        grep " ${archive}\$" "${tmpdir}/SHA256SUMS" | awk '{print $1}' | head -1
    )
    if [ -z "$expected" ]; then
        say "checksum entry for ${archive} not found in SHA256SUMS"
        return 1
    fi

    say "verifying SHA256"
    if ! sha256_verify "${tmpdir}/${archive}" "$expected"; then
        err "checksum mismatch for ${archive} — aborting (refusing to install)"
    fi

    say "extracting"
    tar -xzf "${tmpdir}/${archive}" -C "$tmpdir"

    mkdir -p "$INSTALL_DIR"
    install -m 755 "${tmpdir}/vaughan" "${INSTALL_DIR}/vaughan"
    say "installed ${INSTALL_DIR}/vaughan (${version}, ${platform})"
    return 0
}

install_from_cargo() {
    if ! command -v cargo >/dev/null 2>&1; then
        err "no GitHub release found and cargo is not installed — build from source or install Rust"
    fi
    say "no release tarball available; compiling with cargo (this may take several minutes)"
    tag_args=""
    if [ -n "${VAUGHAN_VERSION:-}" ]; then
        tag_args="--tag ${VAUGHAN_VERSION}"
    fi
    # shellcheck disable=SC2086
    cargo install --locked --git "https://github.com/${REPO}.git" $tag_args --bin vaughan vaughan-cli
    say "installed via cargo (ensure $(cargo home 2>/dev/null || printf '%s' "${HOME}/.cargo")/bin is on PATH)"
}

platform=$(detect_platform)
say "platform: ${platform}"

if version=$(resolve_version); then
    if ! install_from_tarball "$platform" "$version"; then
        say "release download failed for ${platform} (${version})"
        install_from_cargo
    fi
else
    say "no published release yet"
    install_from_cargo
fi

case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        say "add ${INSTALL_DIR} to your PATH, e.g.:"
        say "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        ;;
esac

say "done — run: vaughan"
