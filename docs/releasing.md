# Release Process

FlowWatch releases are built by GitHub Actions from version tags. The workflow publishes separate Apple Silicon and Intel archives plus a `SHA256SUMS` file. No Apple Developer certificate is required because `0.1.x` ships an unsigned CLI and per-user LaunchAgent rather than a Network Extension.

## Prepare A Release

1. Update `[workspace.package].version` in `Cargo.toml` and refresh `Cargo.lock` if needed.
2. Add `docs/releases/v<version>.md`. Its first line must be `# FlowWatch v<version>`.
3. Update both READMEs when commands, storage, compatibility, or security behavior changes.
4. Run the local release checks:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
scripts/check-release.sh v0.1.0 target/release/flowwatch
scripts/test-installer.sh
```

5. Commit the release preparation and let CI pass on `main`.

## Publish

Create and push an annotated tag that exactly matches the Cargo version:

```sh
git tag -a v0.1.0 -m "FlowWatch v0.1.0"
git push origin v0.1.0
```

The `Release` workflow then:

1. validates the tag, Cargo version, binary version, and release announcement;
2. builds `aarch64-apple-darwin` and `x86_64-apple-darwin` binaries with the minimum supported Rust toolchain;
3. packages each binary with the English and Chinese READMEs and MIT license;
4. generates SHA-256 checksums; and
5. creates the GitHub Release from `docs/releases/<tag>.md`.

Use `workflow_dispatch` to test both build jobs without publishing a GitHub Release. Never replace a published tag; prepare a new patch version instead.
