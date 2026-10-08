//! Non-interactive one-shot summary (`gh radar status`).

use crate::fmt::{ago, truncate};
use crate::github::{self, RepoStatus, State};
use crossterm::style::{Color, Stylize};
use std::io::IsTerminal;

pub fn icon(state: State) -> (&'static str, Color) {
    match state {
        State::Success => ("✓", Color::Green),
        State::Failure => ("✗", Color::Red),
        State::Running => ("●", Color::Yellow),
        State::Queued => ("○", Color::Cyan),
        State::Neutral => ("⊘", Color::DarkGrey),
        State::Unknown => ("·", Color::Grey),
    }
}

pub fn print(repos: &[String], ignore: &[String], only_failing: bool) -> bool {
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let paint = |s: String, c: Color| if color { s.with(c).to_string() } else { s };
    let dim = |s: String| if color { s.dim().to_string() } else { s };

    let statuses = github::fetch(repos, true, ignore);
    let now = chrono::Utc::now();
    let width = statuses.iter().map(|s| s.name.len()).max().unwrap_or(0);
    let failing = |s: &RepoStatus| {
        s.error.is_some() || s.latest().is_some_and(|r| r.state() == State::Failure)
    };
    let mut any_failing = false;

    for s in &statuses {
        any_failing |= failing(s);
        if only_failing && !failing(s) {
            continue;
        }
        let name = format!("{:<width$}", s.name);
        if let Some(err) = &s.error {
            println!(
                "{} {}  {}",
                paint("!".into(), Color::Red),
                name,
                paint(err.clone(), Color::Red)
            );
            continue;
        }
        let Some(run) = s.latest() else {
            println!(
                "{} {}  {}",
                dim("·".into()),
                name,
                dim("no workflow runs".into())
            );
            continue;
        };
        let (glyph, c) = icon(run.state());
        let main = match s.default_branch_run() {
            Some(m) => {
                let (g, c) = icon(m.state());
                format!(
                    "{} {}",
                    dim(s.meta.default_branch.clone().unwrap_or_default()),
                    paint(g.into(), c)
                )
            }
            None => String::new(),
        };
        println!(
            "{} {}  {} {:<40} {}  {}",
            paint(glyph.into(), c),
            name,
            dim(format!("{:<5}", ago(run.created_at, now))),
            truncate(&format!("{} · {}", run.workflow(), run.display_title), 40),
            dim(format!("{:<24}", truncate(run.branch(), 24))),
            main,
        );
    }
    any_failing
}
