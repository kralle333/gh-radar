use crate::config::parse_remote;
use std::path::{Path, PathBuf};
use std::process::Command;

const SKIP: [&str; 4] = ["node_modules", "target", "vendor", "dist"];

/// Finds git repositories under `root` (up to `depth` levels deep) that have a
/// github.com remote. Returns `(path, owner/repo)` pairs.
pub fn find_repos(root: &Path, depth: usize) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    walk(root, depth, &mut out);
    out
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, String)>) {
    if dir.join(".git").exists() {
        if let Some(repo) = github_remote(dir) {
            out.push((dir.to_path_buf(), repo));
        }
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && !SKIP.contains(&name.as_ref())
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for d in dirs {
        walk(&d, depth - 1, out);
    }
}

/// Prefers the `origin` remote, falling back to any other github.com remote.
fn github_remote(dir: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["remote", "-v"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut fallback = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(url)) = (parts.next(), parts.next()) else {
            continue;
        };
        if let Some(repo) = parse_remote(url) {
            if name == "origin" {
                return Some(repo);
            }
            fallback.get_or_insert(repo);
        }
    }
    fallback
}
