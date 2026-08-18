#!/bin/sh

set -eu

TAG="${1:-}"
BINARY="${2:-}"
if [ -z "$TAG" ]; then
    echo "用法：scripts/check-release.sh <vMAJOR.MINOR.PATCH> [binary]" >&2
    exit 2
fi

VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
if [ -z "$VERSION" ]; then
    echo "无法从 Cargo.toml 读取工作区版本。" >&2
    exit 2
fi
if [ "$TAG" != "v${VERSION}" ]; then
    echo "标签 ${TAG} 与 Cargo.toml 版本 ${VERSION} 不一致。" >&2
    exit 2
fi

NOTES="docs/releases/${TAG}.md"
if [ ! -s "$NOTES" ]; then
    echo "缺少发布说明：${NOTES}" >&2
    exit 2
fi
if ! grep -q "^# FlowWatch ${TAG}$" "$NOTES"; then
    echo "${NOTES} 首行必须是 '# FlowWatch ${TAG}'。" >&2
    exit 2
fi

if [ -n "$BINARY" ]; then
    ACTUAL_VERSION="$("$BINARY" --version)"
    if [ "$ACTUAL_VERSION" != "flowwatch ${VERSION}" ]; then
        echo "程序版本 ${ACTUAL_VERSION} 与 ${VERSION} 不一致。" >&2
        exit 2
    fi
fi

echo "发布标签 ${TAG} 的元数据一致。"
