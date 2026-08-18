#!/bin/sh

set -eu

REPOSITORY="${FLOWWATCH_REPOSITORY:-JunieXD/FlowWatch}"
VERSION="${FLOWWATCH_VERSION:-latest}"
DOWNLOAD_BASE_OVERRIDE="${FLOWWATCH_DOWNLOAD_BASE:-}"

SYSTEM="$(uname -s)"
ARCHITECTURE="$(uname -m)"
case "${SYSTEM}:${ARCHITECTURE}" in
    Darwin:arm64)
        TARGET="aarch64-apple-darwin"
        ;;
    Darwin:x86_64)
        TARGET="x86_64-apple-darwin"
        ;;
    Darwin:*)
        echo "FlowWatch does not yet support macOS architecture ${ARCHITECTURE}." >&2
        exit 2
        ;;
    *)
        echo "FlowWatch 0.1 currently supports macOS only." >&2
        exit 2
        ;;
esac

ASSET="flowwatch-${TARGET}.tar.gz"
if [ -n "$DOWNLOAD_BASE_OVERRIDE" ]; then
    DOWNLOAD_BASE="${DOWNLOAD_BASE_OVERRIDE%/}"
elif [ "$VERSION" = "latest" ]; then
    DOWNLOAD_BASE="https://github.com/${REPOSITORY}/releases/latest/download"
else
    case "$VERSION" in
        v*) RELEASE_TAG="$VERSION" ;;
        *) RELEASE_TAG="v$VERSION" ;;
    esac
    DOWNLOAD_BASE="https://github.com/${REPOSITORY}/releases/download/${RELEASE_TAG}"
fi

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/flowwatch-install.XXXXXX")"
cleanup() {
    if [ -n "${TEMP_DIR:-}" ] && [ -d "$TEMP_DIR" ]; then
        rm -rf "$TEMP_DIR"
    fi
}
trap cleanup EXIT HUP INT TERM

echo "Downloading FlowWatch for ${TARGET}..."
curl -fsSL --retry 3 --retry-delay 1 \
    "${DOWNLOAD_BASE}/${ASSET}" -o "${TEMP_DIR}/${ASSET}"
curl -fsSL --retry 3 --retry-delay 1 \
    "${DOWNLOAD_BASE}/SHA256SUMS" -o "${TEMP_DIR}/SHA256SUMS"

CHECKSUMS_SIZE="$(wc -c < "${TEMP_DIR}/SHA256SUMS" | tr -d ' ')"
ARCHIVE_SIZE="$(wc -c < "${TEMP_DIR}/${ASSET}" | tr -d ' ')"
if [ "$CHECKSUMS_SIZE" -gt 1048576 ] || [ "$ARCHIVE_SIZE" -gt 67108864 ]; then
    echo "The downloaded release exceeds FlowWatch's size limit." >&2
    exit 2
fi

EXPECTED_HASH="$(awk -v asset="$ASSET" '
    NF == 2 {
        name = $2
        sub(/^\*/, "", name)
        if (name == asset) print tolower($1)
    }
' "${TEMP_DIR}/SHA256SUMS")"
case "$EXPECTED_HASH" in
    *"
"*)
        echo "SHA256SUMS contains multiple entries for ${ASSET}." >&2
        exit 2
        ;;
    *[!0-9a-f]* | "")
        echo "SHA256SUMS has no valid entry for ${ASSET}." >&2
        exit 2
        ;;
esac
if [ "${#EXPECTED_HASH}" -ne 64 ]; then
    echo "SHA256SUMS has no valid entry for ${ASSET}." >&2
    exit 2
fi

ACTUAL_HASH="$(shasum -a 256 "${TEMP_DIR}/${ASSET}" | awk '{print tolower($1)}')"
if [ "$ACTUAL_HASH" != "$EXPECTED_HASH" ]; then
    echo "SHA-256 verification failed for ${ASSET}; installation stopped." >&2
    exit 2
fi
echo "SHA-256 verified."

tar -xzf "${TEMP_DIR}/${ASSET}" -C "$TEMP_DIR"
if [ ! -f "${TEMP_DIR}/flowwatch" ]; then
    echo "The release archive does not contain the flowwatch executable." >&2
    exit 2
fi
chmod 755 "${TEMP_DIR}/flowwatch"
"${TEMP_DIR}/flowwatch" install "$@"

case ":${PATH}:" in
    *":${HOME}/.local/bin:"*) ;;
    *)
        echo "Add FlowWatch to PATH before using the short command:"
        echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac
