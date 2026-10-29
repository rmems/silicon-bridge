## Summary

<!-- What changed, and why. -->

## Linked work

- GitHub issue: <!-- `Closes #N` if this PR resolves it, otherwise `Refs #N` -->
- Linear key: <!-- for example `RM-####`; write "none" if there is no Linear issue -->

## Checklist

Quality bar from `CLAUDE.md`, plus the example-target tests the `uart`
CI job runs. On Linux, the `uart` / `--all-features` lines need
`pkg-config`, `libudev-dev`, and `libhdf5-dev` (for `nir-io`). `cargo test --all-features` does not run
inline tests inside examples.

- [ ] `cargo fmt --check`
- [ ] `cargo fmt --check --manifest-path fuzz/Cargo.toml`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-features`
- [ ] `cargo test --all-features --examples`
- [ ] `cargo check --features uart`

## Notes

<!-- Hardware smoke, the `nir` feature, or anything CI does not run. -->
