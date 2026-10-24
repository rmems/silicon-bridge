# Security policy

## Reporting a vulnerability

Report a vulnerability in this repository through GitHub private
vulnerability reporting:

<https://github.com/rmems/silicon-bridge/security/advisories/new>

Include the crate version or git commit, the feature set (`default`,
`uart`, `nir`), and enough detail to reproduce the issue.

That form is the reporting route for this repository. GitHub shows it to
reporters after an admin enables private vulnerability reporting under
Settings → Advanced Security.

File non-security bugs as public issues. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## What this covers

Reports should be about the `silicon-bridge` crate in this repository:
Q8.8 export, Vivado timing-report parsing, and the optional `uart` /
`serialport` bridge. The crates.io release is 0.3.0. `main` is the
unpublished 0.3.1 candidate (`[package].version` in `Cargo.toml`).
