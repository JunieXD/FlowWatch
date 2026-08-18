#!/bin/sh

set -eu

TAG="${1:-}"
BINARY="${2:-}"
if [ -z "$TAG" ]; then
    echo "usage: scripts/check-release.sh <vMAJOR.MINOR.PATCH> [binary]" >&2
    exit 2
fi

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
if [ -z "$VERSION" ]; then
    echo "Could not read workspace version from Cargo.toml." >&2
    exit 2
fi
if [ "$TAG" != "v${VERSION}" ]; then
    echo "Tag ${TAG} does not match Cargo.toml version ${VERSION}." >&2
    exit 2
fi

NOTES="docs/releases/${TAG}.md"
if [ ! -s "$NOTES" ]; then
    echo "Missing release announcement: ${NOTES}" >&2
    exit 2
fi
if ! grep -q "^# FlowWatch ${TAG}$" "$NOTES"; then
    echo "${NOTES} must start with '# FlowWatch ${TAG}'." >&2
    exit 2
fi

if [ -n "$BINARY" ]; then
    ACTUAL_VERSION="$("$BINARY" --version)"
    if [ "$ACTUAL_VERSION" != "flowwatch ${VERSION}" ]; then
        echo "Binary version ${ACTUAL_VERSION} does not match ${VERSION}." >&2
        exit 2
    fi
fi

echo "Release metadata for ${TAG} is consistent."
