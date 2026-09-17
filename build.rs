// SPDX-License-Identifier: MIT OR Apache-2.0
//! Build-time source revision capture for profiled export metadata.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    watch_git_revision_inputs();
    let revision = git_stdout(["rev-parse", "HEAD"])
        .filter(|value| value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SILICON_BRIDGE_GIT_REV={revision}");
}

fn watch_git_revision_inputs() {
    println!("cargo:rerun-if-changed=.git");
    if let Some(git_dir) = git_dir() {
        let head_path = git_dir.join("HEAD");
        println!("cargo:rerun-if-changed={}", head_path.display());
        watch_path(git_dir.join("packed-refs"));
        if let Some(common_git_dir) = git_path(["rev-parse", "--git-common-dir"]) {
            watch_path(common_git_dir.join("packed-refs"));
            watch_head_reference(&head_path, &git_dir, Some(&common_git_dir));
        } else {
            watch_head_reference(&head_path, &git_dir, None);
        }
    }
}

fn watch_head_reference(head_path: &Path, git_dir: &Path, common_git_dir: Option<&Path>) {
    if let Ok(head) = fs::read_to_string(head_path)
        && let Some(reference) = head.strip_prefix("ref: ")
    {
        let reference = reference.trim();
        watch_path(git_dir.join(reference));
        if let Some(common_git_dir) = common_git_dir {
            watch_path(common_git_dir.join(reference));
        }
    }
}

fn watch_path(path: PathBuf) {
    println!("cargo:rerun-if-changed={}", path.display());
}

fn git_path<const N: usize>(args: [&str; N]) -> Option<PathBuf> {
    git_stdout(args).map(PathBuf::from)
}

fn git_stdout<const N: usize>(args: [&str; N]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn git_dir() -> Option<PathBuf> {
    if let Some(path) = git_path(["rev-parse", "--git-dir"]) {
        return Some(path);
    }
    git_dir_from_dot_git()
}

fn git_dir_from_dot_git() -> Option<PathBuf> {
    let dot_git = PathBuf::from(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let link = fs::read_to_string(&dot_git).ok()?;
    let path = link.trim().strip_prefix("gitdir: ")?.trim();
    let path = PathBuf::from(path);
    Some(if path.is_absolute() {
        path
    } else {
        dot_git
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    })
}
