// SPDX-License-Identifier: MIT OR Apache-2.0
//! Build-time source revision capture for profiled export metadata.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let git_dir = git_dir_from_dot_git(&manifest_dir);
    watch_git_revision_inputs(git_dir.as_deref());
    let revision = git_dir
        .as_ref()
        .and_then(|_| git_stdout(&manifest_dir, ["rev-parse", "HEAD"]))
        .filter(|value| value.len() == 40 && value.chars().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SILICON_BRIDGE_GIT_REV={revision}");
}

fn watch_git_revision_inputs(git_dir: Option<&Path>) {
    let Some(git_dir) = git_dir else {
        return;
    };
    watch_path(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".git"));
    let head_path = git_dir.join("HEAD");
    watch_path(head_path.clone());
    watch_path(git_dir.join("packed-refs"));
    if let Some(common_git_dir) = git_path(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        ["rev-parse", "--git-common-dir"],
    ) {
        watch_path(common_git_dir.join("packed-refs"));
        watch_head_reference(&head_path, git_dir, Some(&common_git_dir));
    } else {
        watch_head_reference(&head_path, git_dir, None);
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
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

fn git_path<const N: usize>(cwd: &Path, args: [&str; N]) -> Option<PathBuf> {
    git_stdout(cwd, args).map(|path| absolute_path(cwd, path))
}

fn git_stdout<const N: usize>(cwd: &Path, args: [&str; N]) -> Option<String> {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|stdout| stdout.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn git_dir_from_dot_git(manifest_dir: &Path) -> Option<PathBuf> {
    let dot_git = manifest_dir.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let link = fs::read_to_string(&dot_git).ok()?;
    let path = link.trim().strip_prefix("gitdir: ")?.trim();
    let path = PathBuf::from(path);
    Some(if path.is_absolute() {
        path
    } else {
        manifest_dir.join(path)
    })
}

fn absolute_path(cwd: &Path, path: String) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}
