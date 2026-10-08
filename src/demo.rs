//! Fictional data for recording the README demo (`GH_RADAR_DEMO=1`).
//!
//! Nothing here talks to GitHub: the dashboard, jobs and repo picker are fed
//! from made-up accounts so a recording never shows anyone's real repos. The
//! data is generated once, relative to the time it is first requested, so ages
//! keep ticking naturally while recording.

use crate::config::Config;
use crate::github::{Actor, Available, Job, Meta, RepoStatus, Run, Step};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::OnceLock;

pub fn enabled() -> bool {
    std::env::var_os("GH_RADAR_DEMO").is_some_and(|v| !v.is_empty() && v != "0")
}

pub fn config() -> Config {
    Config {
        desktop_notifications: false,
        repos: TRACKED.iter().map(|r| r.to_string()).collect(),
        ..Config::default()
    }
}

pub fn statuses(repos: &[String]) -> Vec<RepoStatus> {
    std::thread::sleep(std::time::Duration::from_millis(300));
    repos
        .iter()
        .map(|name| match data().repos.get(&name.to_lowercase()) {
            Some(status) => status.clone(),
            None => RepoStatus {
                name: name.clone(),
                error: Some("Could not resolve to a Repository".into()),
                loaded: true,
                ..Default::default()
            },
        })
        .collect()
}

pub fn jobs(run_id: u64) -> Vec<Job> {
    std::thread::sleep(std::time::Duration::from_millis(250));
    data().jobs.get(&run_id).cloned().unwrap_or_default()
}

pub fn accessible() -> Vec<Available> {
    std::thread::sleep(std::time::Duration::from_millis(900));
    data().accessible.clone()
}

/// Repos tracked when the demo starts.
const TRACKED: &[&str] = &[
    "acme-corp/payments-api",
    "acme-corp/checkout-web",
    "alex-dev/dotfiles",
    "alex-dev/raytracer",
    "octo-labs/api-gateway",
    "octo-labs/billing-service",
    "octo-labs/infra",
    "octo-labs/web-app",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Rust,
    Node,
    Go,
    Terraform,
    Docs,
}

struct Spec {
    name: &'static str,
    description: &'static str,
    private: bool,
    archived: bool,
    fork: bool,
    kind: Kind,
    /// Run results, newest first: S success, F failure, R running, Q queued, C cancelled.
    history: &'static str,
    /// Minutes since the newest run started.
    age: i64,
    prs: u64,
    issues: u64,
}

const fn spec(
    name: &'static str,
    description: &'static str,
    kind: Kind,
    history: &'static str,
    age: i64,
) -> Spec {
    Spec {
        name,
        description,
        private: true,
        archived: false,
        fork: false,
        kind,
        history,
        age,
        prs: 0,
        issues: 0,
    }
}

const SPECS: &[Spec] = &[
    // Tracked from the start.
    Spec {
        prs: 4,
        issues: 9,
        ..spec(
            "acme-corp/payments-api",
            "Payment processing and refunds for all storefronts",
            Kind::Rust,
            "FFSSSSSSSSSS",
            12,
        )
    },
    Spec {
        prs: 7,
        issues: 21,
        ..spec(
            "acme-corp/checkout-web",
            "Checkout flow for web and in-app browsers",
            Kind::Node,
            "QSSSSCSSSSSS",
            1,
        )
    },
    Spec {
        private: false,
        ..spec(
            "alex-dev/dotfiles",
            "zsh, neovim and tmux setup",
            Kind::Docs,
            "SSSSSS",
            3 * 24 * 60,
        )
    },
    Spec {
        private: false,
        issues: 2,
        ..spec(
            "alex-dev/raytracer",
            "A weekend ray tracer in Rust",
            Kind::Rust,
            "SSSFSSSSSS",
            26 * 60,
        )
    },
    Spec {
        prs: 3,
        issues: 12,
        ..spec(
            "octo-labs/api-gateway",
            "Edge gateway: routing, auth and rate limiting",
            Kind::Go,
            "FSSSSSSSSFSS",
            34,
        )
    },
    Spec {
        prs: 2,
        issues: 5,
        ..spec(
            "octo-labs/billing-service",
            "Invoices, subscriptions and usage metering",
            Kind::Rust,
            "SSSSSFSSSSSS",
            95,
        )
    },
    Spec {
        prs: 1,
        ..spec(
            "octo-labs/infra",
            "Terraform for all environments",
            Kind::Terraform,
            "SSSSSSSSSSSS",
            5 * 60,
        )
    },
    Spec {
        prs: 11,
        issues: 38,
        ..spec(
            "octo-labs/web-app",
            "Customer dashboard (Next.js)",
            Kind::Node,
            "RSSSSSSSSSSS",
            2,
        )
    },
    // Only in the picker until added.
    Spec {
        prs: 2,
        issues: 3,
        ..spec(
            "octo-labs/auth-service",
            "OAuth2 / OIDC provider and session management",
            Kind::Go,
            "SSSSSFSSSS",
            50,
        )
    },
    Spec {
        prs: 1,
        issues: 4,
        ..spec(
            "acme-corp/inventory",
            "Stock levels and warehouse sync",
            Kind::Rust,
            "RSSSSSSSS",
            4,
        )
    },
    Spec {
        prs: 6,
        issues: 17,
        ..spec(
            "octo-labs/mobile-app",
            "iOS and Android app (React Native)",
            Kind::Node,
            "SSFSSSSSSS",
            3 * 60,
        )
    },
    Spec {
        prs: 3,
        issues: 6,
        ..spec(
            "octo-labs/notifications",
            "Email, push and webhook delivery",
            Kind::Go,
            "SSSSSSSS",
            7 * 60,
        )
    },
    Spec {
        prs: 1,
        issues: 2,
        ..spec(
            "octo-labs/design-system",
            "Shared React components and tokens",
            Kind::Node,
            "SSSSSSS",
            20 * 60,
        )
    },
    Spec {
        private: false,
        prs: 2,
        ..spec(
            "octo-labs/docs",
            "Public developer documentation",
            Kind::Docs,
            "SSSSFSSS",
            2 * 24 * 60,
        )
    },
    Spec {
        ..spec(
            "octo-labs/data-pipeline",
            "Nightly exports to the warehouse",
            Kind::Rust,
            "SSSSSSSS",
            9 * 60,
        )
    },
    Spec {
        archived: true,
        ..spec(
            "octo-labs/legacy-admin",
            "Old admin panel (replaced by web-app)",
            Kind::Node,
            "SSSS",
            200 * 24 * 60,
        )
    },
    Spec {
        ..spec(
            "octo-labs/feature-flags",
            "Feature flag service and SDKs",
            Kind::Go,
            "SSSSSS",
            4 * 24 * 60,
        )
    },
    Spec {
        ..spec(
            "octo-labs/search",
            "Product search indexer",
            Kind::Rust,
            "SSSSSSS",
            11 * 60,
        )
    },
    Spec {
        prs: 2,
        issues: 8,
        ..spec(
            "acme-corp/storefront",
            "Public storefront",
            Kind::Node,
            "SSSSSSS",
            6 * 60,
        )
    },
    Spec {
        ..spec(
            "acme-corp/warehouse-sync",
            "Syncs orders to warehouse partners",
            Kind::Go,
            "SSSSSS",
            30 * 60,
        )
    },
    Spec {
        ..spec(
            "acme-corp/terraform-modules",
            "Shared Terraform modules",
            Kind::Terraform,
            "SSSSS",
            3 * 24 * 60,
        )
    },
    Spec {
        ..spec(
            "acme-corp/order-events",
            "Order event stream consumers",
            Kind::Rust,
            "SSSSSS",
            13 * 60,
        )
    },
    Spec {
        ..spec(
            "acme-corp/support-bot",
            "Answers the easy support tickets",
            Kind::Node,
            "SSSS",
            9 * 24 * 60,
        )
    },
    Spec {
        archived: true,
        ..spec(
            "acme-corp/old-checkout",
            "Previous checkout (kept for reference)",
            Kind::Node,
            "SSS",
            400 * 24 * 60,
        )
    },
    Spec {
        private: false,
        ..spec(
            "alex-dev/advent-of-code",
            "Solutions in Rust",
            Kind::Rust,
            "SSSSS",
            40 * 24 * 60,
        )
    },
    Spec {
        ..spec(
            "alex-dev/homelab",
            "Ansible for the home server",
            Kind::Terraform,
            "SSSS",
            6 * 24 * 60,
        )
    },
    Spec {
        private: false,
        ..spec(
            "alex-dev/blog",
            "Personal blog",
            Kind::Docs,
            "SSSSS",
            12 * 24 * 60,
        )
    },
    Spec {
        private: false,
        fork: true,
        ..spec(
            "alex-dev/ratatui",
            "Fork for a PR",
            Kind::Rust,
            "SSS",
            60 * 24 * 60,
        )
    },
];

const APP_TITLES: &[&str] = &[
    "feat: add rate limiting to public endpoints",
    "fix: handle expired refresh tokens",
    "refactor: split worker into smaller jobs",
    "feat: export invoices as CSV",
    "fix: race condition in webhook delivery",
    "perf: cache tenant lookups",
    "docs: document retry configuration",
    "feat: add OpenTelemetry tracing",
    "fix: flaky integration test",
    "ci: run tests on macOS",
    "feat: dark mode for settings",
    "fix: off-by-one in pagination",
];
const INFRA_TITLES: &[&str] = &[
    "Add staging bucket for exports",
    "Bump AWS provider to 5.80",
    "Restrict security group ingress",
    "Scale workers to 6 replicas",
    "Rotate database credentials",
];
const DOCS_TITLES: &[&str] = &[
    "Update README",
    "Add tmux config",
    "Fix broken links",
    "New post: debugging flaky CI",
    "Tweak prompt colors",
];
const BRANCHES: &[&str] = &[
    "feat/rate-limiting",
    "fix/token-refresh",
    "refactor/worker-split",
    "feat/csv-export",
    "fix/webhook-race",
    "perf/tenant-cache",
];
const ACTORS: &[&str] = &["alex-dev", "mira-k", "tobias-n", "sana-r", "jon-b"];

impl Kind {
    fn titles(self) -> &'static [&'static str] {
        match self {
            Kind::Terraform => INFRA_TITLES,
            Kind::Docs => DOCS_TITLES,
            _ => APP_TITLES,
        }
    }

    fn workflows(self) -> &'static [&'static str] {
        match self {
            Kind::Rust => &["CI", "CI", "Release", "CI", "Security Audit"],
            Kind::Node => &[
                "CI",
                "Deploy to Staging",
                "CI",
                "E2E",
                "Deploy to Production",
            ],
            Kind::Go => &["CI", "CI", "Docker", "Deploy"],
            Kind::Terraform => &["terraform plan", "terraform apply"],
            Kind::Docs => &["Pages", "Link check"],
        }
    }

    fn jobs(self, workflow: &str) -> (&'static [&'static str], &'static str) {
        match (self, workflow) {
            (_, w) if w.starts_with("Deploy") => (
                &["build image", "push", "deploy"],
                "Run kubectl rollout status",
            ),
            (_, "Docker" | "Release") => (&["build", "publish"], "Build and push"),
            (_, "Security Audit") => (&["cargo audit"], "Run cargo audit"),
            (_, "E2E") => (&["e2e (chromium)", "e2e (webkit)"], "Run playwright test"),
            (Kind::Rust, _) => (
                &[
                    "fmt",
                    "clippy",
                    "test (ubuntu-latest)",
                    "test (macos-latest)",
                ],
                "Run cargo test --all-features",
            ),
            (Kind::Node, _) => (&["lint", "unit tests", "build"], "Run npm test"),
            (Kind::Go, _) => (&["golangci-lint", "test", "build"], "Run go test ./..."),
            (Kind::Terraform, _) => (
                &["fmt", "validate", "plan (staging)", "plan (production)"],
                "Run terraform plan",
            ),
            (Kind::Docs, _) => (&["build", "deploy"], "Run lychee"),
        }
    }
}

struct Data {
    repos: HashMap<String, RepoStatus>,
    jobs: HashMap<u64, Vec<Job>>,
    accessible: Vec<Available>,
}

fn data() -> &'static Data {
    static DATA: OnceLock<Data> = OnceLock::new();
    DATA.get_or_init(|| generate(Utc::now()))
}

fn generate(now: DateTime<Utc>) -> Data {
    let mut repos = HashMap::new();
    let mut jobs = HashMap::new();
    let mut accessible = Vec::new();

    for (index, spec) in SPECS.iter().enumerate() {
        let seed = index * 7 + spec.name.len();
        let default_branch = if spec.kind == Kind::Docs && index % 2 == 0 {
            "master"
        } else {
            "main"
        };
        let workflows = spec.kind.workflows();
        let gap = 41 + (seed % 5) as i64 * 23;
        let mut runs = Vec::new();

        for (i, result) in spec.history.chars().enumerate() {
            // Failures come from CI, which is what you usually have to go and fix.
            let workflow = match (result, spec.kind) {
                ('F', Kind::Terraform) => "terraform plan",
                ('F', Kind::Docs) => "Link check",
                ('F', _) => "CI",
                _ => workflows[(seed + i) % workflows.len()],
            };
            let (branch, event) = if workflow == "Release" {
                (format!("v1.{}.{}", 9 - i.min(9), seed % 7), "release")
            } else if matches!(workflow, "Security Audit" | "Link check") {
                (default_branch.to_string(), "schedule")
            } else if workflow.starts_with("Deploy")
                || workflow.contains("apply")
                || workflow == "Pages"
                || i % 2 == 1
            {
                (default_branch.to_string(), "push")
            } else {
                (
                    BRANCHES[(seed + i) % BRANCHES.len()].to_string(),
                    "pull_request",
                )
            };
            let titles = spec.kind.titles();
            let title = match event {
                "release" => branch.clone(),
                "schedule" => workflow.to_string(),
                // BRANCHES line up with the first APP_TITLES, so a PR's title matches its branch.
                "pull_request" if titles == APP_TITLES => {
                    APP_TITLES[(seed + i) % BRANCHES.len()].to_string()
                }
                _ => titles[(seed + i * 5) % titles.len()].to_string(),
            };
            let (status, conclusion) = match result {
                'F' => ("completed", Some("failure")),
                'R' => ("in_progress", None),
                'Q' => ("queued", None),
                'C' => ("completed", Some("cancelled")),
                _ => ("completed", Some("success")),
            };
            let started = now
                - Duration::minutes(spec.age + i as i64 * gap)
                - Duration::seconds((seed * 13 % 50) as i64);
            let took = Duration::seconds(45 + ((seed + i * 31) % 9) as i64 * 37);
            let id = (index as u64 + 1) * 10_000 + i as u64;
            let actor = if event == "schedule" {
                "github-actions"
            } else {
                ACTORS[(seed + i) % ACTORS.len()]
            };

            let run = Run {
                id,
                name: Some(workflow.to_string()),
                display_title: title,
                head_branch: Some(branch),
                event: event.to_string(),
                status: Some(status.to_string()),
                conclusion: conclusion.map(str::to_string),
                created_at: started,
                updated_at: if status == "completed" {
                    started + took
                } else {
                    now
                },
                run_started_at: Some(started),
                html_url: format!("https://github.com/{}/actions/runs/{id}", spec.name),
                path: Some(".github/workflows/ci.yml".into()),
                run_number: 400 - (index as u64 * 13 % 300) - i as u64,
                run_attempt: Some(1),
                actor: Some(Actor {
                    login: actor.to_string(),
                }),
            };
            jobs.insert(id, make_jobs(spec.kind, &run, seed + i, took));
            runs.push(run);
        }

        let pushed_at = runs.first().map(|r| r.created_at - Duration::seconds(20));
        repos.insert(
            spec.name.to_lowercase(),
            RepoStatus {
                name: spec.name.to_string(),
                meta: Meta {
                    description: Some(spec.description.to_string()),
                    url: Some(format!("https://github.com/{}", spec.name)),
                    default_branch: Some(default_branch.to_string()),
                    pushed_at,
                    archived: spec.archived,
                    open_prs: Some(spec.prs),
                    open_issues: Some(spec.issues),
                },
                runs,
                error: None,
                loaded: true,
            },
        );
        accessible.push(Available {
            full_name: spec.name.to_string(),
            description: Some(spec.description.to_string()),
            pushed_at,
            private: spec.private,
            archived: spec.archived,
            fork: spec.fork,
        });
    }
    accessible.sort_by_key(|r| std::cmp::Reverse(r.pushed_at));
    Data {
        repos,
        jobs,
        accessible,
    }
}

fn make_jobs(kind: Kind, run: &Run, seed: usize, took: Duration) -> Vec<Job> {
    let (names, main_step) = kind.jobs(run.workflow());
    let failing = seed % names.len();
    let started = run.run_started_at.unwrap_or(run.created_at);
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let length = took * (60 + (i as i32 * 17) % 40) / 100;
            let (status, conclusion, step_status, step_conclusion) =
                match (run.status.as_deref(), run.conclusion.as_deref()) {
                    (Some("queued"), _) => ("queued", None, "queued", None),
                    (Some("in_progress"), _) if i == failing => {
                        ("in_progress", None, "in_progress", None)
                    }
                    (Some("in_progress"), _) if i > failing => ("queued", None, "queued", None),
                    (_, Some("failure")) if i == failing => {
                        ("completed", Some("failure"), "completed", Some("failure"))
                    }
                    (_, Some("cancelled")) => (
                        "completed",
                        Some("cancelled"),
                        "completed",
                        Some("cancelled"),
                    ),
                    _ => ("completed", Some("success"), "completed", Some("success")),
                };
            let done = status == "completed";
            Job {
                name: name.to_string(),
                status: Some(status.into()),
                conclusion: conclusion.map(Into::into),
                started_at: (status != "queued").then_some(started),
                completed_at: done.then_some(started + length),
                steps: vec![
                    step("Set up job", "completed", Some("success")),
                    step("Checkout", "completed", Some("success")),
                    step(main_step, step_status, step_conclusion),
                ],
            }
        })
        .collect()
}

fn step(name: &str, status: &str, conclusion: Option<&str>) -> Step {
    Step {
        name: name.into(),
        status: Some(status.into()),
        conclusion: conclusion.map(Into::into),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_repos_have_data() {
        let data = generate(Utc::now());
        for repo in TRACKED {
            let status = &data.repos[&repo.to_lowercase()];
            assert!(!status.runs.is_empty(), "{repo} has no runs");
            assert!(
                status.runs.iter().all(|r| data.jobs.contains_key(&r.id)),
                "{repo} is missing jobs"
            );
        }
        assert_eq!(data.accessible.len(), SPECS.len());
    }
}
