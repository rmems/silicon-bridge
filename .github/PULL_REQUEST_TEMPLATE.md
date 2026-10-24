## Summary

<!-- What changed, and why. -->

## Linked work

- GitHub issue: <!-- `Closes #N` if this PR resolves it, otherwise `Refs #N` -->
- Linear key: <!-- for example `RM-####`; write "none" if there is no Linear issue -->

## Checklist

Quality bar from `CLAUDE.md`. On Linux, the `uart` / `--all-features` lines
need `pkg-config` and `libudev-dev`.

- [ ] `cargo fmt --check`
- [ ] `cargo fmt --check --manifest-path fuzz/Cargo.toml`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --all-features`
- [ ] `cargo check --features uart`

## Notes

<!-- Hardware smoke, the `nir` feature, or anything CI does not run. -->
