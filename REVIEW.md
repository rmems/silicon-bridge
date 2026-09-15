# REVIEW.md

Code review guidelines for silicon-bridge.

## General principles

- Code must compile and pass all tests before merge
- No secrets, credentials, or private telemetry in diffs
- Keep changes minimal and focused — one concern per PR
- Follow existing patterns in the codebase

## Review checklist

### Correctness

- [ ] Logic matches the stated intent
- [ ] Edge cases handled (empty vecs, zero values, overflow)
- [ ] Q8.8 conversions are correct and bounded:
      signed **parameter** `.mem` path: `-128..=127.99609375` via
      `encode_q88_signed_full`; legacy **UART** helper: `-127.99..=127.99`
      via `encode_q88_signed`; unsigned-magnitude pair: `0.0..=255.99609375`
- [ ] Hardware `.mem` images encode through `encode_q88_signed_full`, never
      `encode_q88_unsigned` and never the UART helper `encode_q88_signed`.
      UART frames keep `encode_q88_signed`.
- [ ] No off-by-one errors in array indexing

### API design

- [ ] Public types use `pub` appropriately — don't expose internals
- [ ] Builder pattern (e.g., `FpgaParameterExporter`) is ergonomic
- [ ] Public API parameters use `f32` consistently (matching existing types)
- [ ] Breaking changes are documented in CHANGELOG.md

### Documentation

- [ ] SPDX license header on all `.rs` files
- [ ] Doc comments on all public items (`///` for items, `//!` for modules)
- [ ] Examples in doc comments compile and pass as doctests
- [ ] Relative links to license files avoided in rustdoc (use plain text)

### Testing

- [ ] New code has corresponding tests
- [ ] Tests cover happy path and edge cases
- [ ] Doctests are accurate and runnable
- [ ] `cargo test` passes with no warnings

### Security

- [ ] No hardcoded secrets or API keys
- [ ] No `unsafe` code without safety justification
- [ ] Feature-gated code properly uses `#[cfg(feature = "...")]`
- [ ] No unnecessary dependencies added

## Common review comments

### Edition 2024 is valid

Do not suggest downgrading to edition 2021. This project uses edition 2024 intentionally.

### Relative license links break rustdoc

Links like `[MIT](../LICENSE-MIT)` won't resolve on docs.rs. Use plain text:
```rust
//! Licensed under either of MIT or Apache-2.0 at your option.
```

### Changelog markdownlint

Lists in CHANGELOG.md need a blank line before them:
```markdown
### Changed

- Item here (not directly after heading)
```

### Unused dependencies

Check if new dependencies are actually imported. Do not reintroduce unused crates (e.g. the former unused `rand` dependency).

## AI reviewer notes

### Codacy

- Flags markdown lint issues — check CHANGELOG formatting
- May flag `println!` — prefer `eprintln!` or logging in library code

### Gemini

- May flag relative links in doc comments — already addressed in lib.rs
- Generally accurate on code quality suggestions

### CodeRabbit

- Auto-resolves threads when fix is pushed — verify thread status before merging
- Sometimes flags pre-existing issues — confirm the issue was introduced by the PR

### Amazon Q

- May suggest edition downgrade — ignore if edition 2024 is intentional
- Variable naming suggestions are usually reasonable

### Kilo Code Review

- Reply `@kilocode-bot fix it` to any suggestion thread to auto-fix the issue
- Kilo runs `cargo test`, `cargo check`, `cargo clippy` when applying fixes
- Flags unused dependencies, type mismatches, missing tests
- Suggestions are generally correct — fix unless there's a specific reason not to
- Review suggestions before approving — Kilo can make mistakes

### Devin Review

- Auto-reviews PRs on push — no manual trigger needed
- With Auto-Fix enabled, Devin auto-responds to review comments and fixes flagged bugs
- Checks: correctness, coding standards, style guides, security requirements
- Use Devin Review chat to ask specific questions about the code
- Runs as a CI check — verify it passes before merging

## CI/CD reference

| Check | Trigger | Behavior |
|-------|---------|----------|
| CodeRabbit | Push to PR | Auto-reviews, auto-resolves when fix is pushed |
| Devin Review | Push to PR | Auto-reviews, can auto-fix with Auto-Fix enabled |
| Kilo Code Review | Push to PR | Suggests fixes via review threads |
| Codacy | Push to PR | Markdown lint, configured to skip docs via `.codacy.yml` |

Run locally before pushing:

```bash
cargo check                 # compile check
cargo test                  # all unit + doc tests
cargo clippy                # lint
cargo build --features uart # verify UART feature compiles
```
