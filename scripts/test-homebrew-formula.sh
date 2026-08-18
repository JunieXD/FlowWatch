#!/bin/sh

set -eu

TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/flowwatch-formula-test.XXXXXX")"
cleanup() {
    if [ -n "${TEST_ROOT:-}" ] && [ -d "$TEST_ROOT" ]; then
        rm -rf "$TEST_ROOT"
    fi
}
trap cleanup EXIT HUP INT TERM

CHECKSUMS="${TEST_ROOT}/SHA256SUMS"
cat > "$CHECKSUMS" <<'SUMS'
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  flowwatch-aarch64-apple-darwin.tar.gz
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  flowwatch-x86_64-apple-darwin.tar.gz
SUMS

OUTPUT="${TEST_ROOT}/Formula/flowwatch.rb"
scripts/render-homebrew-formula.sh v9.8.7 "$CHECKSUMS" "$OUTPUT" >/dev/null
grep -q 'releases/download/v9.8.7/' "$OUTPUT"
grep -q 'sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"' "$OUTPUT"
grep -q 'sha256 "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"' "$OUTPUT"

printf '%s\n' \
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  flowwatch-aarch64-apple-darwin.tar.gz' \
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  flowwatch-aarch64-apple-darwin.tar.gz' \
    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  flowwatch-x86_64-apple-darwin.tar.gz' \
    > "$CHECKSUMS"
if scripts/render-homebrew-formula.sh v9.8.7 "$CHECKSUMS" "$OUTPUT" >/dev/null 2>&1; then
    echo "Formula 生成器接受了重复的架构条目" >&2
    exit 1
fi

if scripts/render-homebrew-formula.sh v9.8.7-beta "$CHECKSUMS" "$OUTPUT" >/dev/null 2>&1; then
    echo "Formula 生成器接受了预发布版本" >&2
    exit 1
fi

echo "Homebrew Formula 生成测试通过。"
