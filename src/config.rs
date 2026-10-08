use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Seconds between full refreshes of every tracked repo.
    pub refresh_secs: u64,
    /// Seconds between refreshes of repos that have a queued or running workflow.
    pub active_refresh_secs: u64,
    /// Show a desktop notification when a run you were watching finishes.
    pub desktop_notifications: bool,
    /// Workflow paths to leave out. A trailing `*` matches a prefix. The default
    /// hides GitHub-managed runs (Dependabot updates, Copilot reviews), which you
    /// can't fix or re-run from the repo and which would otherwise mask its CI status.
    pub ignore_workflows: Vec<String>,
    pub repos: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            refresh_secs: 120,
            active_refresh_secs: 15,
            desktop_notifications: true,
            ignore_workflows: vec!["dynamic/*".into()],
            repos: Vec::new(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        if let Some(p) = std::env::var_os("GH_RADAR_CONFIG") {
            return PathBuf::from(p);
        }
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        base.join("gh-radar").join("config.toml")
    }

    pub fn load() -> Result<Self> {
        if crate::demo::enabled() {
            return Ok(crate::demo::config());
        }
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("invalid config in {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        if crate::demo::enabled() {
            return Ok(());
        }
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("failed to write {}", path.display()))
    }

    /// Returns false if the repo was already tracked.
    pub fn add(&mut self, repo: &str) -> bool {
        if self.repos.iter().any(|r| r.eq_ignore_ascii_case(repo)) {
            return false;
        }
        self.repos.push(repo.to_string());
        self.repos.sort_by_key(|r| r.to_lowercase());
        true
    }

    /// Returns false if the repo wasn't tracked.
    pub fn remove(&mut self, repo: &str) -> bool {
        let before = self.repos.len();
        self.repos.retain(|r| !r.eq_ignore_ascii_case(repo));
        self.repos.len() != before
    }
}

const GITHUB_PREFIXES: [&str; 5] = [
    "https://github.com/",
    "http://github.com/",
    "git@github.com:",
    "ssh://git@github.com/",
    "github.com/",
];

/// Accepts `owner/repo` or any github.com URL pointing into a repo.
pub fn parse_repo(input: &str) -> Option<String> {
    let s = input.trim();
    let rest = GITHUB_PREFIXES
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .unwrap_or(s);
    if rest.contains(':') || rest.contains('@') {
        return None;
    }
    let rest = rest.trim_end_matches('/');
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let valid = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    (valid(owner) && valid(name)).then(|| format!("{owner}/{name}"))
}

/// Like `parse_repo`, but only for git remote URLs that point at github.com.
pub fn parse_remote(url: &str) -> Option<String> {
    GITHUB_PREFIXES
        .iter()
        .any(|p| url.starts_with(p))
        .then(|| parse_repo(url))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_forms() {
        assert_eq!(parse_repo("cli/cli").as_deref(), Some("cli/cli"));
        assert_eq!(
            parse_repo("https://github.com/cli/cli.git").as_deref(),
            Some("cli/cli")
        );
        assert_eq!(
            parse_repo("https://github.com/cli/cli/actions/runs/1").as_deref(),
            Some("cli/cli")
        );
        assert_eq!(
            parse_repo("git@github.com:cli/cli.git").as_deref(),
            Some("cli/cli")
        );
        assert_eq!(parse_repo("cli"), None);
        assert_eq!(parse_repo("a/b\"){x}"), None);
        assert_eq!(parse_remote("git@gitlab.com:a/b.git"), None);
        assert_eq!(
            parse_remote("ssh://git@github.com/a/b").as_deref(),
            Some("a/b")
        );
    }
}
