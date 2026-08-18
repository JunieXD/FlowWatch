# Security

FlowWatch processes local network accounting metadata and may store a Clash/Mihomo controller secret.

## Supported Versions

Only the latest published release receives security fixes during the `0.1.x` series.

## Reporting A Vulnerability

Use [GitHub's private vulnerability reporting](https://github.com/JunieXD/FlowWatch/security/advisories/new). Do not open a public issue for a vulnerability and do not attach a production database or Clash configuration. Provide a minimal reproduction with synthetic credentials and data.

## Current Security Model

- Controllers are restricted to local HTTP addresses in `0.1`.
- The Clash secret is stored as plain text in SQLite by design.
- App-created data directories use mode `0700`; database and LaunchAgent files use mode `0600`.
- Secrets are redacted from normal CLI output.
- Raw packets, remote domains, remote IPs, and raw controller responses are not persisted.
- LaunchAgent stdout and stderr go to `/dev/null`; operational health is stored as bounded metadata in SQLite.
- The release installer verifies the selected archive against the release `SHA256SUMS` file before executing it.

These controls do not protect secrets from a process already running as the same macOS user. Users who require credential-store protection should not configure the Clash provider until a keychain integration is available.

FlowWatch `0.1.x` is distributed without Apple code signing or notarization. Release checksums detect transfer corruption and mismatched assets, but they are hosted with the release and are not a substitute for a separately trusted signature. Users who require signed and notarized software should build from reviewed source or wait for a future signed distribution.
