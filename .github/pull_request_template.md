## Summary

Describe the user-visible change and which ledger it affects: physical, direct application, Clash, storage, or installation.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --locked --workspace --all-targets -- -D warnings`
- [ ] `cargo test --locked --workspace`
- [ ] Accuracy-sensitive changes include a regression test
- [ ] Documentation reflects permission, privacy, retention, or compatibility changes
