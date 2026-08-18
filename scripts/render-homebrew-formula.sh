#!/bin/sh

set -eu

TAG="${1:-}"
CHECKSUMS="${2:-}"
OUTPUT="${3:-}"
if [ -z "$TAG" ] || [ -z "$CHECKSUMS" ] || [ -z "$OUTPUT" ]; then
    echo "用法：scripts/render-homebrew-formula.sh <vMAJOR.MINOR.PATCH> <SHA256SUMS> <输出文件>" >&2
    exit 2
fi
case "$TAG" in
    v*) VERSION="${TAG#v}" ;;
    *) echo "版本标签无效：${TAG}" >&2; exit 2 ;;
esac
case "$VERSION" in
    "" | *[!0-9.]*) echo "版本标签无效：${TAG}" >&2; exit 2 ;;
esac
OLD_IFS="$IFS"
IFS=.
set -- $VERSION
IFS="$OLD_IFS"
if [ "$#" -ne 3 ] || [ -z "$1" ] || [ -z "$2" ] || [ -z "$3" ]; then
    echo "版本标签无效：${TAG}" >&2
    exit 2
fi
if [ ! -f "$CHECKSUMS" ]; then
    echo "找不到校验和文件：${CHECKSUMS}" >&2
    exit 2
fi

read_hash() {
    asset="$1"
    value="$(awk -v asset="$asset" '
        NF == 2 {
            name = $2
            sub(/^\*/, "", name)
            if (name == asset) print tolower($1)
        }
    ' "$CHECKSUMS")"
    case "$value" in
        *"
"* | *[!0-9a-f]* | "")
            echo "SHA256SUMS 中必须且只能包含一个 ${asset} 条目。" >&2
            exit 2
            ;;
    esac
    if [ "${#value}" -ne 64 ]; then
        echo "${asset} 的 SHA-256 无效。" >&2
        exit 2
    fi
    printf '%s' "$value"
}

ARM64_SHA256="$(read_hash flowwatch-aarch64-apple-darwin.tar.gz)"
X86_64_SHA256="$(read_hash flowwatch-x86_64-apple-darwin.tar.gz)"
TEMPLATE="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)/packaging/homebrew/flowwatch.rb.in"
if [ ! -f "$TEMPLATE" ]; then
    echo "找不到 Formula 模板：${TEMPLATE}" >&2
    exit 2
fi

OUTPUT_DIR="$(dirname -- "$OUTPUT")"
mkdir -p "$OUTPUT_DIR"
TEMPORARY="${OUTPUT}.new.$$"
cleanup() {
    rm -f "$TEMPORARY"
}
trap cleanup EXIT HUP INT TERM
sed \
    -e "s/@VERSION@/${VERSION}/g" \
    -e "s/@ARM64_SHA256@/${ARM64_SHA256}/g" \
    -e "s/@X86_64_SHA256@/${X86_64_SHA256}/g" \
    "$TEMPLATE" > "$TEMPORARY"
mv "$TEMPORARY" "$OUTPUT"
trap - EXIT HUP INT TERM
echo "已生成 FlowWatch ${TAG} 的 Homebrew Formula：${OUTPUT}"
