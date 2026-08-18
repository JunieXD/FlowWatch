<div align="center">

# FlowWatch

**See where your Mac's network traffic went, without packet capture or administrator privileges.**

[![Release](https://img.shields.io/github/v/release/JunieXD/FlowWatch?display_name=tag&style=flat-square)](https://github.com/JunieXD/FlowWatch/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/JunieXD/FlowWatch/ci.yml?branch=main&label=CI&style=flat-square)](https://github.com/JunieXD/FlowWatch/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/platform-macOS%2013%2B-2f6f5e?style=flat-square)
[![License](https://img.shields.io/github/license/JunieXD/FlowWatch?style=flat-square)](LICENSE)

[简体中文](README.zh-CN.md) · [Quick start](#quick-start) · [How accounting works](#how-accounting-works) · [Accuracy limits](#accuracy-limits)

</div>

FlowWatch is a lightweight, local-first network accounting tool for macOS. It keeps an authoritative total from physical interfaces, attributes as much traffic as it can to applications, and shows the unexplained remainder instead of forcing every byte into a guessed process.

The current `0.1.0` release is an unsigned CLI and per-user LaunchAgent. It supports Apple Silicon and Intel Macs, starts automatically after login, and does not require `sudo`, packet capture, a system extension, or an Apple Developer account.

## Why FlowWatch

- **Authoritative totals:** native 64-bit macOS interface counters answer how much the Mac actually transferred.
- **Per-app attribution:** structured `nettop` snapshots identify long-lived direct connections without parsing display text.
- **Proxy-aware accounting:** optional Clash/Mihomo integration restores application identity behind a system proxy while excluding the proxy carrier from direct totals.
- **Honest coverage:** physical totals, attributed apps, observed proxy actors, internal traffic, and attribution gaps stay separate.
- **Small local footprint:** aggregate SQLite storage, no packet payloads, no domains or remote IP history, and roughly 10 MiB resident memory in current testing.
- **Useful history:** application rankings, interface totals, high-traffic minutes, gap reports, exact time ranges, and JSON output.

## Quick Start

### 1. Install

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/FlowWatch/main/scripts/install.sh | sh
```

The installer selects the correct Apple Silicon or Intel archive, verifies its SHA-256 checksum, installs the CLI under `~/.local/bin`, and starts a per-user LaunchAgent. If `~/.local/bin` is not already in `PATH`, the installer prints the exact export command to add.

### 2. Check the collector

```sh
flowwatch doctor
flowwatch status
```

### 3. Find the applications using traffic

```sh
flowwatch apps --period today
flowwatch apps --period 24h --sort download --limit 50
flowwatch gaps --period 24h --limit 20
```

A healthy status report separates the accounting layers:

```text
Today
  Physical:   up 888.0 MiB  down 1.9 GiB
  Attributed: up 782.4 MiB  down 1.3 GiB  (77.7% coverage)
  Clash:
    Total:        up 834.6 MiB  down 1.5 GiB
    Attributed:   up 726.4 MiB  down 1.2 GiB
    Classification:
      Observed actor:       up 2.9 MiB  down 33.3 MiB
      App-attributed actor: up 2.9 MiB  down 33.3 MiB  (100.0% coverage)
      Non-actor/unobserved: up 28.1 KiB  down 2.9 MiB
```

## How Accounting Works

| Ledger | Source | What it answers |
| --- | --- | --- |
| Physical | Native macOS interface counters | How many bytes crossed hardware `enN` interfaces? |
| Direct applications | Periodic structured `nettop` snapshots | Which applications used direct physical connections? |
| Proxied applications | Optional Clash/Mihomo controller and local socket matching | Which applications generated traffic carried by the proxy? |
| Coverage and gaps | Comparison of independent ledgers | How much traffic is attributed, and when was attribution incomplete? |

Physical and application data are deliberately independent. Protocol overhead, short connections, missing process metadata, sleep, and sampling boundaries mean application rows do not always add up to the physical total. FlowWatch reports that difference; it does not normalize it away.

See [Architecture](docs/architecture.md) for database invariants, deduplication rules, and the future platform-backend design.

## Optional Clash/Mihomo Integration

Import a Clash Verge or Mihomo configuration after the controller is listening on localhost:

```sh
flowwatch config import-clash \
  "$HOME/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev/config.yaml"
flowwatch config show
```

FlowWatch reads the controller address and secret but never changes the proxy configuration. `find-process-mode: strict` can improve controller-provided process identities. When process fields are missing, FlowWatch can temporarily match a loopback source port to a locally observed application.

The secret is stored as plain text in the SQLite database by design. The data directory is mode `0700`, database files are mode `0600`, normal command output redacts the secret, and only local HTTP controllers are accepted.

## Commands

| Command | Purpose |
| --- | --- |
| `flowwatch status` | Collector health, totals, coverage, and current Clash classification |
| `flowwatch apps` | Rank applications by upload, download, or total bytes |
| `flowwatch interfaces` | Query authoritative physical-interface totals |
| `flowwatch spikes` | Show the highest-traffic physical minutes |
| `flowwatch gaps` | Rank time buckets by unattributed traffic |
| `flowwatch doctor` | Check storage, collectors, permissions, and LaunchAgent state |
| `flowwatch config` | Manage Clash and application-detail settings |
| `flowwatch install` | Install or refresh the current-user service |
| `flowwatch uninstall` | Remove the service while preserving history by default |

Queries accept `today`, `yesterday`, `24h`, `7d`, `30d`, `all`, another positive `h`/`d` period, or an explicit `--from` and `--to` range. Query commands support `--json`.

Five-minute application detail is the lightweight default. Temporarily use one-minute detail when investigating a spike:

```sh
flowwatch config set-app-granularity 1m
flowwatch config set-app-granularity 5m
```

## Storage And Privacy

FlowWatch stores its database at:

```text
~/Library/Application Support/io.github.FlowWatch.FlowWatch/traffic.sqlite3
```

Defaults retain application and minute aggregates for 30 days and daily totals for 365 days. Raw packets, payloads, remote domains, remote IP addresses, local ports, raw `nettop` output, and raw controller responses are not persisted. The local socket-owner map lives only in memory for about 15 seconds. FlowWatch has no telemetry or cloud service.

## Accuracy Limits

- Standard mode is best-effort application attribution, not packet-level process accounting.
- A connection that starts and ends entirely between snapshots can appear in the physical ledger without an application identity.
- The first observation of a socket establishes a baseline; recording its historical cumulative counter would create false spikes.
- Clash TUN/INNER traffic, non-actor traffic, unknown applications, and controller sampling gaps remain unattributed.
- Physical counters can bridge collector restarts, but interface resets and sleep/network transitions can create boundaries.
- Completed direct-attribution buckets are checked against physical bounds; suspicious overcount is reported rather than silently scaled.

Use `flowwatch gaps` to identify the exact periods where attribution was weakest. The full list of known limitations is maintained in [Architecture](docs/architecture.md).

## Upgrade And Uninstall

Run the install command again to upgrade. The verified release replaces the executable and restarts the LaunchAgent while preserving the SQLite database, Clash configuration, retention settings, and selected application granularity.

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/FlowWatch/main/scripts/install.sh | sh
```

Remove the service and CLI while retaining history:

```sh
flowwatch uninstall
```

Add `--purge-data` only when the traffic database should also be deleted.

## Development

Requirements are macOS 13 or newer, Xcode Command Line Tools, and Rust 1.88 or newer.

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
```

Accounting changes should include regression tests and identify which ledger they affect. See [Contributing](CONTRIBUTING.md), [Security](SECURITY.md), and [Release Process](docs/releasing.md).

## License

[MIT](LICENSE) © JunieXD and FlowWatch contributors.
