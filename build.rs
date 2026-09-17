// SPDX-License-Identifier: MIT OR Apache-2.0
//! Build-time source revision capture for profiled export metadata.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.git/HEAD");
    let revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_owned())
        .filter(|value| value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SILICON_BRIDGE_GIT_REV={revision}");
}
