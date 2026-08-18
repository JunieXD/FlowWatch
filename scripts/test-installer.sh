#!/bin/sh

set -eu

TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/flowwatch-installer-test.XXXXXX")"
cleanup() {
    if [ -n "${TEST_ROOT:-}" ] && [ -d "$TEST_ROOT" ]; then
        rm -rf "$TEST_ROOT"
    fi
}
trap cleanup EXIT HUP INT TERM

case "$(uname -m)" in
    arm64) TARGET="aarch64-apple-darwin" ;;
    x86_64) TARGET="x86_64-apple-darwin" ;;
    *) echo "unsupported test architecture" >&2; exit 2 ;;
esac

RELEASE_DIR="${TEST_ROOT}/release"
FIXTURE_DIR="${TEST_ROOT}/fixture"
MARKER="${TEST_ROOT}/arguments"
mkdir -p "$RELEASE_DIR" "$FIXTURE_DIR" "${TEST_ROOT}/home"

cat > "${FIXTURE_DIR}/flowwatch" <<'FIXTURE'
#!/bin/sh
printf '%s\n' "$@" > "$FLOWWATCH_TEST_MARKER"
FIXTURE
chmod 755 "${FIXTURE_DIR}/flowwatch"

ASSET="flowwatch-${TARGET}.tar.gz"
tar -czf "${RELEASE_DIR}/${ASSET}" -C "$FIXTURE_DIR" flowwatch
(
    cd "$RELEASE_DIR"
    shasum -a 256 "$ASSET" > SHA256SUMS
)

HOME="${TEST_ROOT}/home" \
FLOWWATCH_DOWNLOAD_BASE="file://${RELEASE_DIR}" \
FLOWWATCH_TEST_MARKER="$MARKER" \
sh scripts/install.sh --app-granularity 1m >/dev/null

EXPECTED_ARGUMENTS="${TEST_ROOT}/expected-arguments"
printf '%s\n' install --app-granularity 1m > "$EXPECTED_ARGUMENTS"
cmp "$EXPECTED_ARGUMENTS" "$MARKER"

printf 'corrupt' >> "${RELEASE_DIR}/${ASSET}"
rm -f "$MARKER"
if HOME="${TEST_ROOT}/home" \
    FLOWWATCH_DOWNLOAD_BASE="file://${RELEASE_DIR}" \
    FLOWWATCH_TEST_MARKER="$MARKER" \
    sh scripts/install.sh >/dev/null 2>&1; then
    echo "installer accepted an archive with an invalid checksum" >&2
    exit 1
fi
if [ -e "$MARKER" ]; then
    echo "installer executed an archive before checksum verification" >&2
    exit 1
fi

echo "Installer tests passed."
