mod config;
mod fmt;
mod github;
mod scan;
mod status;
mod tui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::{Config, parse_repo};
use std::path::PathBuf;

/// Keep an eye on all your GitHub repos and their workflow runs.
///
/// Run without arguments to open the dashboard.
#[derive(Parser)]
#[command(name = "gh-radar", bin_name = "gh radar", version)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Track repositories (owner/repo or URL); defaults to the repo in the current directory
    Add {
        repos: Vec<String>,
        /// Pick from all repos you have access to (personal and organizations)
        #[arg(short, long, conflicts_with = "repos")]
        interactive: bool,
    },
    /// Stop tracking repositories
    #[command(alias = "remove")]
    Rm {
        #[arg(required = true)]
        repos: Vec<String>,
    },
    /// List tracked repositories
    Ls,
    /// Find local git clones with GitHub remotes and track them
    Scan {
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// How many directory levels to descend
        #[arg(long, default_value_t = 2)]
        depth: usize,
        /// Only show what would be added
        #[arg(long)]
        dry_run: bool,
    },
    /// Print a one-shot summary instead of opening the dashboard
    Status {
        /// Only show repos whose latest run failed
        #[arg(long)]
        failing: bool,
        /// Exit with status 1 if any repo's latest run failed
        #[arg(long)]
        exit_status: bool,
    },
    /// Print the path of the config file
    Config,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut cfg = Config::load()?;

    match cli.cmd {
        None => {
            let empty = cfg.repos.is_empty();
            tui::run(cfg, empty)?;
        }
        Some(Cmd::Add {
            interactive: true, ..
        }) => tui::run(cfg, true)?,
        Some(Cmd::Add { repos, .. }) => {
            let repos = if repos.is_empty() {
                vec![github::current_repo()?]
            } else {
                repos
            };
            for input in repos {
                match parse_repo(&input) {
                    None => {
                        eprintln!("skipping {input:?}: expected owner/repo or a github.com URL")
                    }
                    Some(r) if cfg.add(&r) => println!("+ {r}"),
                    Some(r) => println!("  {r} (already tracked)"),
                }
            }
            cfg.save()?;
        }
        Some(Cmd::Rm { repos }) => {
            for input in repos {
                let r = parse_repo(&input).unwrap_or(input);
                if cfg.remove(&r) {
                    println!("- {r}");
                } else {
                    eprintln!("{r} is not tracked");
                }
            }
            cfg.save()?;
        }
        Some(Cmd::Ls) => {
            for r in &cfg.repos {
                println!("{r}");
            }
        }
        Some(Cmd::Scan {
            dir,
            depth,
            dry_run,
        }) => {
            let found = scan::find_repos(&dir, depth);
            if found.is_empty() {
                println!(
                    "No git repos with GitHub remotes found under {}",
                    dir.display()
                );
            }
            let mut added = 0;
            for (path, repo) in found {
                let new = if dry_run {
                    !cfg.repos.iter().any(|r| r.eq_ignore_ascii_case(&repo))
                } else {
                    cfg.add(&repo)
                };
                added += new as usize;
                let mark = if new { "+" } else { " " };
                println!("{mark} {repo:<45} {}", path.display());
            }
            if dry_run {
                println!("\n{added} new repos would be tracked (dry run)");
            } else {
                cfg.save()?;
                println!("\n{added} new repos tracked; remove any with `gh radar rm owner/repo`");
            }
        }
        Some(Cmd::Status {
            failing,
            exit_status,
        }) => {
            let any_failing = status::print(&cfg.repos, &cfg.ignore_workflows, failing);
            if exit_status && any_failing {
                std::process::exit(1);
            }
        }
        Some(Cmd::Config) => println!("{}", Config::path().display()),
    }
    Ok(())
}
