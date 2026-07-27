#!/usr/bin/env bash
#
# spacetimedb-tui installer for Linux & macOS.
#
# Downloads the latest (or a pinned) pre-built release archive from
# GitHub, verifies the binary runs, and installs it to a directory on
# the user's PATH.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/RazieLDG/spacetimedb-tui/main/scripts/install.sh | bash
#
#   # Pin a specific version
#   curl -fsSL .../install.sh | bash -s -- --version v0.1.0
#
#   # Install to a custom directory (default: $HOME/.local/bin)
#   curl -fsSL .../install.sh | bash -s -- --dir /usr/local/bin
#
# Environment overrides (useful inside CI):
#   STDB_TUI_VERSION   — same as --version
#   STDB_TUI_INSTALL_DIR — same as --dir

set -euo pipefail

# ── Configuration ────────────────────────────────────────────────────────────
REPO="RazieLDG/spacetimedb-tui"
BIN_NAME="spacetimedb-tui"
DEFAULT_INSTALL_DIR="${HOME}/.local/bin"

VERSION="${STDB_TUI_VERSION:-latest}"
INSTALL_DIR="${STDB_TUI_INSTALL_DIR:-${DEFAULT_INSTALL_DIR}}"

# ── Colour helpers ───────────────────────────────────────────────────────────
if [ -t 1 ]; then
    BOLD=$(printf '\033[1m')
    RED=$(printf '\033[31m')
    GREEN=$(printf '\033[32m')
    YELLOW=$(printf '\033[33m')
    CYAN=$(printf '\033[36m')
    RESET=$(printf '\033[0m')
else
    BOLD= RED= GREEN= YELLOW= CYAN= RESET=
fi

info()  { printf '%s==>%s %s\n' "${CYAN}"    "${RESET}" "$*"; }
warn()  { printf '%swarn:%s %s\n' "${YELLOW}" "${RESET}" "$*" >&2; }
err()   { printf '%serror:%s %s\n' "${RED}"  "${RESET}" "$*" >&2; exit 1; }
ok()    { printf '%sok:%s %s\n' "${GREEN}"   "${RESET}" "$*"; }

# ── Argument parsing ─────────────────────────────────────────────────────────
while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --dir)     INSTALL_DIR="$2"; shift 2 ;;
        -h|--help)
            sed -n '3,20p' "$0"
            exit 0
            ;;
        *)
            err "Unknown argument: $1 (try --help)"
            ;;
    esac
done

# ── Platform detection ───────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "${OS}" in
    Linux)
        case "${ARCH}" in
            x86_64|amd64) TARGET="x86_64-unknown-linux-gnu" ;;
            *) err "Unsupported Linux architecture: ${ARCH}. Pre-built binaries only ship x86_64 Linux." ;;
        esac
        ;;
    Darwin)
        case "${ARCH}" in
            x86_64) TARGET="x86_64-apple-darwin" ;;
            arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
            *) err "Unsupported macOS architecture: ${ARCH}." ;;
        esac
        ;;
    *)
        err "Unsupported OS: ${OS}. Use scripts/install.ps1 on Windows."
        ;;
esac

# ── Tool sanity check ────────────────────────────────────────────────────────
need() { command -v "$1" >/dev/null 2>&1 || err "missing required tool: $1"; }
need curl
need tar
need mktemp
need install

# ── Resolve version ──────────────────────────────────────────────────────────
if [ "${VERSION}" = "latest" ]; then
    info "Resolving latest release from github.com/${REPO}..."
    # Follow the "latest release" redirect to pick up the tag name without
    # needing the API (which would rate-limit anonymous calls).
    LATEST_URL=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
        "https://github.com/${REPO}/releases/latest") \
        || err "Failed to query latest release."
    VERSION="${LATEST_URL##*/}"
    if [ -z "${VERSION}" ] || [ "${VERSION}" = "latest" ]; then
        err "Could not determine latest version — no releases yet?"
    fi
    ok "Latest version: ${BOLD}${VERSION}${RESET}"
fi

# ── Download + extract ───────────────────────────────────────────────────────
ARCHIVE="${BIN_NAME}-${VERSION}-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

TMP=$(mktemp -d)
trap 'rm -rf "${TMP}"' EXIT

info "Downloading ${ARCHIVE}..."
if ! curl -fsSL --proto '=https' --tlsv1.2 -o "${TMP}/${ARCHIVE}" "${URL}"; then
    err "Download failed. URL: ${URL}"
fi

info "Extracting archive..."
tar -xzf "${TMP}/${ARCHIVE}" -C "${TMP}"

STAGED="${TMP}/${BIN_NAME}-${VERSION}-${TARGET}/${BIN_NAME}"
if [ ! -f "${STAGED}" ]; then
    err "Archive layout unexpected — could not find ${BIN_NAME} inside."
fi

# ── Install ──────────────────────────────────────────────────────────────────
mkdir -p "${INSTALL_DIR}"
install -m 0755 "${STAGED}" "${INSTALL_DIR}/${BIN_NAME}"
ok "Installed ${BIN_NAME} ${VERSION} → ${INSTALL_DIR}/${BIN_NAME}"

# ── PATH hint ────────────────────────────────────────────────────────────────
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        warn "${INSTALL_DIR} is not on your PATH."
        case "$(basename "${SHELL:-}")" in
            bash) rc="~/.bashrc" ;;
            zsh)  rc="~/.zshrc"  ;;
            fish) rc="~/.config/fish/config.fish" ;;
            *)    rc="your shell rc file" ;;
        esac
        printf '       add this line to %s and restart your shell:\n' "${rc}"
        printf '         export PATH="%s:$PATH"\n' "${INSTALL_DIR}"
        ;;
esac

# ── Smoke test ───────────────────────────────────────────────────────────────
if "${INSTALL_DIR}/${BIN_NAME}" --version >/dev/null 2>&1; then
    ok "$(${INSTALL_DIR}/${BIN_NAME} --version 2>&1 || echo "${BIN_NAME} ${VERSION}")"
fi

ok "Done. Run ${BOLD}${BIN_NAME} --help${RESET} to get started."
