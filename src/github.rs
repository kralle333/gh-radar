//! Data fetching. Everything goes through the `gh` CLI so we reuse its auth,
//! host configuration and proxy settings instead of managing tokens ourselves.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const RUNS_PER_REPO: usize = 40;
const WORKERS: usize = 8;

type RunsResult = Option<Result<Vec<Run>, String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Failure,
    Running,
    Queued,
    Success,
    Neutral,
    Unknown,
}

impl State {
    pub fn is_active(self) -> bool {
        matches!(self, State::Running | State::Queued)
    }
}

fn state_of(status: Option<&str>, conclusion: Option<&str>) -> State {
    match status {
        Some("completed") | None => match conclusion {
            Some("success") => State::Success,
            Some("failure" | "timed_out" | "startup_failure" | "action_required") => State::Failure,
            Some("cancelled" | "skipped" | "neutral" | "stale") => State::Neutral,
            _ => State::Unknown,
        },
        Some("in_progress") => State::Running,
        Some(_) => State::Queued,
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Step {
    pub name: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
}

impl Step {
    pub fn state(&self) -> State {
        state_of(self.status.as_deref(), self.conclusion.as_deref())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Job {
    pub name: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

impl Job {
    pub fn state(&self) -> State {
        state_of(self.status.as_deref(), self.conclusion.as_deref())
    }

    pub fn duration(&self, now: DateTime<Utc>) -> Option<Duration> {
        let start = self.started_at?;
        Some(self.completed_at.unwrap_or(now) - start)
    }
}

/// Jobs of the latest attempt of a run.
pub fn fetch_jobs(repo: &str, run_id: u64) -> Result<Vec<Job>> {
    let v = gh(&[
        "api",
        &format!("repos/{repo}/actions/runs/{run_id}/jobs?per_page=100"),
    ])?;
    let jobs = v.get("jobs").cloned().unwrap_or(Value::Array(vec![]));
    Ok(serde_json::from_value(jobs)?)
}

#[derive(Debug, Clone, Deserialize)]
pub struct Actor {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Run {
    pub id: u64,
    pub name: Option<String>,
    pub display_title: String,
    pub head_branch: Option<String>,
    pub event: String,
    pub status: Option<String>,
    pub conclusion: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub run_started_at: Option<DateTime<Utc>>,
    pub html_url: String,
    #[serde(default)]
    pub path: Option<String>,
    pub run_number: u64,
    #[serde(default)]
    pub run_attempt: Option<u64>,
    pub actor: Option<Actor>,
}

impl Run {
    pub fn state(&self) -> State {
        state_of(self.status.as_deref(), self.conclusion.as_deref())
    }

    pub fn workflow(&self) -> &str {
        self.name.as_deref().unwrap_or("workflow")
    }

    pub fn branch(&self) -> &str {
        self.head_branch.as_deref().unwrap_or("")
    }

    pub fn duration(&self, now: DateTime<Utc>) -> Duration {
        let start = self.run_started_at.unwrap_or(self.created_at);
        let end = if self.state().is_active() {
            now
        } else {
            self.updated_at
        };
        end - start
    }
}

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub description: Option<String>,
    pub url: Option<String>,
    pub default_branch: Option<String>,
    pub pushed_at: Option<DateTime<Utc>>,
    pub archived: bool,
    pub open_prs: Option<u64>,
    pub open_issues: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct RepoStatus {
    pub name: String,
    pub meta: Meta,
    pub runs: Vec<Run>,
    pub error: Option<String>,
    /// False until the first fetch for this repo has completed.
    pub loaded: bool,
}

impl RepoStatus {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..Default::default()
        }
    }

    pub fn latest(&self) -> Option<&Run> {
        self.runs.first()
    }

    /// Latest run on the default branch, i.e. "is main green?".
    pub fn default_branch_run(&self) -> Option<&Run> {
        let branch = self.meta.default_branch.as_deref()?;
        self.runs
            .iter()
            .find(|r| r.head_branch.as_deref() == Some(branch))
    }

    pub fn has_active_runs(&self) -> bool {
        self.runs.iter().any(|r| r.state().is_active())
    }

    pub fn last_activity(&self) -> Option<DateTime<Utc>> {
        let run = self.latest().map(|r| r.updated_at);
        run.max(self.meta.pushed_at)
    }

    pub fn url(&self) -> String {
        self.meta
            .url
            .clone()
            .unwrap_or_else(|| format!("https://github.com/{}", self.name))
    }
}

fn gh(args: &[&str]) -> Result<Value> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .context("failed to run `gh`; is the GitHub CLI installed?")?;
    if out.status.success() {
        return serde_json::from_slice(&out.stdout).context("unexpected response from gh");
    }
    // GraphQL responses with partial errors still carry useful data.
    if let Ok(v) = serde_json::from_slice::<Value>(&out.stdout)
        && v.get("data").is_some_and(|d| !d.is_null())
    {
        return Ok(v);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let msg = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("gh api failed");
    bail!("{}", msg.trim().trim_start_matches("gh: "))
}

pub fn fetch_runs(repo: &str, ignore: &[String]) -> Result<Vec<Run>> {
    let v = gh(&[
        "api",
        &format!("repos/{repo}/actions/runs?per_page={RUNS_PER_REPO}"),
    ])?;
    let runs = v
        .get("workflow_runs")
        .cloned()
        .unwrap_or(Value::Array(vec![]));
    let mut runs: Vec<Run> = serde_json::from_value(runs)?;
    runs.retain(|r| !r.path.as_deref().is_some_and(|p| is_ignored(p, ignore)));
    Ok(runs)
}

fn is_ignored(path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| match pat.strip_suffix('*') {
        Some(prefix) => path.starts_with(prefix),
        None => path == pat,
    })
}

/// Fetches metadata for many repos in a single GraphQL request per 50 repos.
fn fetch_meta(repos: &[String]) -> HashMap<String, Result<Meta, String>> {
    let mut result = HashMap::new();
    for chunk in repos.chunks(50) {
        let mut query = String::from("query {");
        for (i, repo) in chunk.iter().enumerate() {
            // Names are validated by config::parse_repo, so they are safe to inline.
            let (owner, name) = repo.split_once('/').unwrap_or((repo, ""));
            query.push_str(&format!(
                " r{i}: repository(owner: \"{owner}\", name: \"{name}\") {{ \
                   description url pushedAt isArchived defaultBranchRef {{ name }} \
                   pullRequests(states: OPEN) {{ totalCount }} issues(states: OPEN) {{ totalCount }} }}"
            ));
        }
        query.push_str(" }");

        let response = gh(&["api", "graphql", "-f", &format!("query={query}")]);
        for (i, repo) in chunk.iter().enumerate() {
            let entry = match &response {
                Err(e) => Err(e.to_string()),
                Ok(v) => {
                    let node = &v["data"][format!("r{i}")];
                    if node.is_null() {
                        Err(graphql_error(v, &format!("r{i}"))
                            .unwrap_or_else(|| "repository not found".into()))
                    } else {
                        Ok(Meta {
                            description: node["description"].as_str().map(str::to_string),
                            url: node["url"].as_str().map(str::to_string),
                            default_branch: node["defaultBranchRef"]["name"]
                                .as_str()
                                .map(str::to_string),
                            pushed_at: node["pushedAt"].as_str().and_then(|s| s.parse().ok()),
                            archived: node["isArchived"].as_bool().unwrap_or(false),
                            open_prs: node["pullRequests"]["totalCount"].as_u64(),
                            open_issues: node["issues"]["totalCount"].as_u64(),
                        })
                    }
                }
            };
            result.insert(repo.to_lowercase(), entry);
        }
    }
    result
}

fn graphql_error(v: &Value, alias: &str) -> Option<String> {
    v["errors"].as_array()?.iter().find_map(|e| {
        (e["path"].get(0).and_then(Value::as_str) == Some(alias))
            .then(|| e["message"].as_str().unwrap_or("error").to_string())
    })
}

/// Fetches workflow runs for every repo (in parallel) and, if `with_meta`,
/// repository metadata as well. Results keep the order of `repos`.
pub fn fetch(repos: &[String], with_meta: bool, ignore: &[String]) -> Vec<RepoStatus> {
    let next = AtomicUsize::new(0);
    let runs: Mutex<Vec<RunsResult>> = Mutex::new(vec![None; repos.len()]);
    let mut meta = HashMap::new();

    std::thread::scope(|s| {
        let meta_handle = with_meta.then(|| s.spawn(|| fetch_meta(repos)));
        for _ in 0..WORKERS.min(repos.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(repo) = repos.get(i) else { break };
                    let r = fetch_runs(repo, ignore).map_err(|e| e.to_string());
                    runs.lock().unwrap()[i] = Some(r);
                }
            });
        }
        if let Some(h) = meta_handle {
            meta = h.join().unwrap_or_default();
        }
    });

    let runs = runs.into_inner().unwrap();
    repos
        .iter()
        .zip(runs)
        .map(|(name, runs)| {
            let mut status = RepoStatus::new(name);
            status.loaded = true;
            match runs {
                Some(Ok(r)) => status.runs = r,
                Some(Err(e)) => status.error = Some(e),
                None => status.error = Some("not fetched".into()),
            }
            match meta.remove(&name.to_lowercase()) {
                Some(Ok(m)) => status.meta = m,
                // A missing repo makes both calls fail; prefer the clearer GraphQL message.
                Some(Err(e)) => status.error = Some(e),
                None => {}
            }
            status
        })
        .collect()
}

/// A repository the viewer can access, as listed by the repo picker.
#[derive(Debug, Clone, Deserialize)]
pub struct Available {
    pub full_name: String,
    pub description: Option<String>,
    pub pushed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub fork: bool,
}

impl Available {
    pub fn owner(&self) -> &str {
        self.full_name.split_once('/').map_or("", |(o, _)| o)
    }
}

/// Every repo the viewer owns, collaborates on, or can access through an
/// organization or team, most recently pushed first.
pub fn fetch_accessible() -> Result<Vec<Available>> {
    let out = Command::new("gh")
        .args([
            "api",
            "--paginate",
            "user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member",
            "--jq",
            ".[] | {full_name, description, pushed_at, private, archived, fork}",
        ])
        .output()
        .context("failed to run `gh`; is the GitHub CLI installed?")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "{}",
            stderr
                .lines()
                .next()
                .unwrap_or("failed to list repositories")
                .trim()
        );
    }
    let mut repos = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str::<Available>)
        .collect::<Result<Vec<_>, _>>()?;
    repos.sort_by_key(|r| std::cmp::Reverse(r.pushed_at));
    Ok(repos)
}

/// The `owner/repo` of the git repository in the current directory.
pub fn current_repo() -> Result<String> {
    let out = Command::new("gh")
        .args([
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "-q",
            ".nameWithOwner",
        ])
        .output()
        .context("failed to run `gh`")?;
    if !out.status.success() {
        bail!("not inside a GitHub repository; pass owner/repo explicitly");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_workflow_paths() {
        let patterns = vec![
            "dynamic/*".to_string(),
            ".github/workflows/noisy.yml".to_string(),
        ];
        assert!(is_ignored(
            "dynamic/dependabot/dependabot-updates",
            &patterns
        ));
        assert!(is_ignored(
            "dynamic/agents/copilot-pull-request-reviewer",
            &patterns
        ));
        assert!(is_ignored(".github/workflows/noisy.yml", &patterns));
        assert!(!is_ignored(".github/workflows/ci.yml", &patterns));
        assert!(!is_ignored("dynamic/anything", &[]));
    }
}
