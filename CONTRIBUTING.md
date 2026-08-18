# Contributing

FlowWatch is early-stage accounting software, so correctness and explicit uncertainty matter more than maximizing attributed bytes. Bug reports should include the macOS version, Mac architecture, FlowWatch version, `flowwatch doctor` output, and whether traffic was direct, proxied, or TUN-based. Do not include Clash secrets, full configuration files, production databases, raw controller responses, or private IP/domain data.

Open an issue before a large architectural change. Keep operating-system APIs inside platform backend crates; shared models and SQLite code must remain portable enough for a future Windows backend.

Before opening a pull request, run:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
scripts/test-installer.sh
```

Changes to accounting logic must include a regression test and explain which ledger is affected: physical, direct application, Clash, storage, or installation. Never improve apparent coverage by assigning unknown bytes to a guessed application or silently scaling an overcount.

Use conventional, focused commit messages. Pull requests should update the English and Chinese documentation together when commands, compatibility, privacy, permissions, retention, or accuracy behavior changes. Maintainers follow the documented [release process](docs/releasing.md).
