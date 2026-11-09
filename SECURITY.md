# Security policy

## Reporting a vulnerability

Report a vulnerability in this repository through GitHub private
vulnerability reporting:

<https://github.com/rmems/silicon-bridge/security/advisories/new>

GitHub accepts a report on that form only after an admin enables private
vulnerability reporting (Settings → Advanced Security → Private
vulnerability reporting). That is the reporting route for this
repository.

Include the crate version or git commit, the feature set (`default`,
`uart`, `nir`, `nir-io`), and enough detail to reproduce the issue.

File non-security bugs as public issues. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## What this covers

Reports should be about the `silicon-bridge` crate in this repository:
Q8.8 export, Vivado timing-report parsing, and the optional `uart` /
`serialport` bridge. The crates.io release is 0.4.0 (`[package].version` in
`Cargo.toml`).
