use crate::config::Config;
use crate::fmt::{ago, duration};
use crate::github::{self, Available, Job, RepoStatus, Run, State};
use anyhow::Result;
use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{cursor, execute};
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Padding, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const FLASH_FOR: Duration = Duration::from_secs(8);
const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

pub fn run(config: Config, open_picker: bool) -> Result<()> {
    ignore_sigint();
    let mut terminal = ratatui::init();
    let mut app = App::new(config);
    if open_picker {
        app.open_picker();
    }
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}

/// In raw mode Ctrl-C arrives as a key event, but while a child such as
/// `gh run watch` owns the terminal it raises SIGINT for the whole process
/// group. A no-op handler keeps the dashboard alive; children still get the
/// default handler since handlers are reset on exec.
fn ignore_sigint() {
    #[cfg(unix)]
    {
        extern "C" fn noop(_: libc::c_int) {}
        unsafe {
            libc::signal(libc::SIGINT, noop as *const () as libc::sighandler_t);
        }
    }
}

enum Msg {
    Fetched {
        statuses: Vec<RepoStatus>,
        with_meta: bool,
    },
    Action {
        repo: String,
        text: String,
        ok: bool,
    },
    Accessible(Result<Vec<Available>, String>),
    Jobs {
        run_id: u64,
        updated_at: chrono::DateTime<Utc>,
        jobs: Result<Vec<Job>, String>,
    },
}

struct JobsEntry {
    /// `updated_at` of the run when the jobs were fetched; a change means they're stale.
    updated_at: chrono::DateTime<Utc>,
    fetched: Instant,
    jobs: Result<Vec<Job>, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Repos,
    Runs,
    Picker,
}

#[derive(Clone, Copy)]
enum Sort {
    Activity,
    Attention,
    Name,
}

impl Sort {
    fn label(self) -> &'static str {
        match self {
            Sort::Activity => "activity",
            Sort::Attention => "attention",
            Sort::Name => "name",
        }
    }
    fn next(self) -> Self {
        match self {
            Sort::Activity => Sort::Attention,
            Sort::Attention => Sort::Name,
            Sort::Name => Sort::Activity,
        }
    }
}

struct Confirm {
    prompt: String,
    action: ConfirmAction,
}

enum ConfirmAction {
    Gh {
        repo: String,
        args: Vec<String>,
        done: String,
    },
    Untrack(String),
}

/// State of the "add repositories" picker.
struct Picker {
    query: String,
    owner: Option<String>,
    show_archived: bool,
    /// Lowercased names of the repos that will be tracked when saved.
    checked: HashSet<String>,
    table: TableState,
}

struct App {
    config: Config,
    repos: Vec<RepoStatus>,
    view: View,
    sort: Sort,
    table: TableState,
    runs_table: TableState,
    detail: Option<String>,
    filter: String,
    filtering: bool,
    confirm: Option<Confirm>,
    show_help: bool,
    flash: Option<(String, Color, Instant)>,
    pending: usize,
    last_full: Instant,
    last_active: Instant,
    last_updated: Option<chrono::DateTime<Utc>>,
    picker: Option<Picker>,
    accessible: Option<Result<Vec<Available>, String>>,
    accessible_loading: bool,
    show_details: bool,
    jobs: HashMap<u64, JobsEntry>,
    jobs_loading: HashSet<u64>,
    /// The run whose jobs the details pane wants, and since when (to debounce scrolling).
    jobs_wanted: Option<(u64, Instant)>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    quit: bool,
}

impl App {
    fn new(config: Config) -> Self {
        let (tx, rx) = mpsc::channel();
        let repos = config.repos.iter().map(|r| RepoStatus::new(r)).collect();
        Self {
            config,
            repos,
            view: View::Repos,
            sort: Sort::Activity,
            table: TableState::default().with_selected(0),
            runs_table: TableState::default().with_selected(0),
            detail: None,
            filter: String::new(),
            filtering: false,
            confirm: None,
            show_help: false,
            flash: None,
            pending: 0,
            last_full: Instant::now(),
            last_active: Instant::now(),
            last_updated: None,
            picker: None,
            accessible: None,
            accessible_loading: false,
            show_details: true,
            jobs: HashMap::new(),
            jobs_loading: HashSet::new(),
            jobs_wanted: None,
            tx,
            rx,
            quit: false,
        }
    }

    fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        self.refresh_all();
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            if event::poll(Duration::from_millis(200))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.on_key(key, terminal)?;
            }
            while let Ok(msg) = self.rx.try_recv() {
                self.apply(msg);
            }
            self.schedule();
            self.ensure_jobs();
        }
        Ok(())
    }

    // ---------------------------------------------------------------- data

    fn spawn_fetch(&mut self, repos: Vec<String>, with_meta: bool) {
        if repos.is_empty() {
            return;
        }
        self.pending += 1;
        let tx = self.tx.clone();
        let ignore = self.config.ignore_workflows.clone();
        std::thread::spawn(move || {
            let statuses = github::fetch(&repos, with_meta, &ignore);
            let _ = tx.send(Msg::Fetched {
                statuses,
                with_meta,
            });
        });
    }

    fn refresh_all(&mut self) {
        self.last_full = Instant::now();
        self.last_active = Instant::now();
        self.spawn_fetch(self.config.repos.clone(), true);
    }

    /// Full refresh every `refresh_secs`; repos with queued/running workflows
    /// are polled every `active_refresh_secs` so you hear about results quickly.
    fn schedule(&mut self) {
        if self.pending > 0 {
            return;
        }
        if self.last_full.elapsed() >= Duration::from_secs(self.config.refresh_secs.max(10)) {
            self.refresh_all();
        } else if self.last_active.elapsed()
            >= Duration::from_secs(self.config.active_refresh_secs.max(5))
        {
            self.last_active = Instant::now();
            let active = self
                .repos
                .iter()
                .filter(|r| r.has_active_runs())
                .map(|r| r.name.clone())
                .collect();
            self.spawn_fetch(active, false);
        }
    }

    /// Fetches jobs for the run shown in the details pane once the selection
    /// has settled, and keeps them fresh while the run is in progress.
    fn ensure_jobs(&mut self) {
        if !self.show_details || self.view == View::Picker {
            return;
        }
        let Some((repo, run)) = self.target_run() else {
            return;
        };
        match self.jobs_wanted {
            Some((id, since)) if id == run.id => {
                if since.elapsed() < Duration::from_millis(250) {
                    return;
                }
            }
            _ => {
                self.jobs_wanted = Some((run.id, Instant::now()));
                return;
            }
        }
        if self.jobs_loading.contains(&run.id) {
            return;
        }
        let stale = match self.jobs.get(&run.id) {
            None => true,
            Some(e) => {
                e.updated_at != run.updated_at
                    || (run.state().is_active() && e.fetched.elapsed() > Duration::from_secs(10))
            }
        };
        if !stale {
            return;
        }
        self.jobs_loading.insert(run.id);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let jobs = github::fetch_jobs(&repo, run.id).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Jobs {
                run_id: run.id,
                updated_at: run.updated_at,
                jobs,
            });
        });
    }

    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Jobs {
                run_id,
                updated_at,
                jobs,
            } => {
                self.jobs_loading.remove(&run_id);
                self.jobs.insert(
                    run_id,
                    JobsEntry {
                        updated_at,
                        fetched: Instant::now(),
                        jobs,
                    },
                );
            }
            Msg::Accessible(result) => {
                self.accessible_loading = false;
                self.accessible = Some(result);
                if let Some(p) = &mut self.picker {
                    p.table.select(Some(0));
                }
            }
            Msg::Action { repo, text, ok } => {
                self.set_flash(text, if ok { Color::Green } else { Color::Red });
                if ok {
                    self.spawn_fetch(vec![repo], false);
                }
            }
            Msg::Fetched {
                statuses,
                with_meta,
            } => {
                self.pending = self.pending.saturating_sub(1);
                self.last_updated = Some(Utc::now());
                // Before the first load the order is meaningless, so start at the top.
                let first_load = !self.repos.iter().any(|r| r.loaded);
                let selected_repo = self
                    .selected_repo()
                    .filter(|_| !first_load)
                    .map(|r| r.name.clone());
                let selected_run = self.selected_detail_run().map(|r| r.id);
                let mut finished = Vec::new();

                for new in statuses {
                    let Some(old) = self
                        .repos
                        .iter_mut()
                        .find(|r| r.name.eq_ignore_ascii_case(&new.name))
                    else {
                        continue;
                    };
                    if old.loaded {
                        for run in new.runs.iter().filter(|r| !r.state().is_active()) {
                            if old
                                .runs
                                .iter()
                                .any(|o| o.id == run.id && o.state().is_active())
                            {
                                finished.push((new.name.clone(), run.clone()));
                            }
                        }
                    }
                    if with_meta {
                        *old = new;
                    } else {
                        old.runs = new.runs;
                        old.error = new.error;
                        old.loaded = true;
                    }
                }

                if first_load {
                    self.table.select(Some(0));
                } else if let Some(name) = selected_repo
                    && let Some(pos) = self
                        .visible()
                        .iter()
                        .position(|&i| self.repos[i].name == name)
                {
                    self.table.select(Some(pos));
                }
                if let Some(id) = selected_run
                    && let Some(pos) = self
                        .detail_repo()
                        .and_then(|r| r.runs.iter().position(|r| r.id == id))
                {
                    self.runs_table.select(Some(pos));
                }
                self.notify(finished);
            }
        }
    }

    fn notify(&mut self, finished: Vec<(String, Run)>) {
        let Some((repo, run)) = finished.first() else {
            return;
        };
        let failed = finished
            .iter()
            .filter(|(_, r)| r.state() == State::Failure)
            .count();
        let text = if finished.len() == 1 {
            let outcome = match run.state() {
                State::Success => "succeeded".to_string(),
                State::Failure => "failed".to_string(),
                _ => run.conclusion.clone().unwrap_or_else(|| "finished".into()),
            };
            format!("{repo} · {} #{} {outcome}", run.workflow(), run.run_number)
        } else {
            format!("{} runs finished, {failed} failed", finished.len())
        };
        let (glyph, color) = if failed > 0 {
            ("✗", Color::Red)
        } else {
            ("✓", Color::Green)
        };
        self.set_flash(format!("{glyph} {text}"), color);
        if failed > 0 {
            print!("\x07");
            let _ = std::io::stdout().flush();
        }
        if self.config.desktop_notifications {
            desktop_notify(&format!("{glyph} {text}"));
        }
    }

    fn set_flash(&mut self, text: impl Into<String>, color: Color) {
        self.flash = Some((text.into(), color, Instant::now()));
    }

    // ----------------------------------------------------------- selection

    fn visible(&self) -> Vec<usize> {
        let filter = self.filter.to_lowercase();
        let mut idx: Vec<usize> = (0..self.repos.len())
            .filter(|&i| filter.is_empty() || self.repos[i].name.to_lowercase().contains(&filter))
            .collect();
        let r = &self.repos;
        match self.sort {
            Sort::Activity => idx.sort_by_key(|&i| Reverse(r[i].last_activity())),
            Sort::Attention => {
                idx.sort_by_key(|&i| (attention(&r[i]), Reverse(r[i].last_activity())))
            }
            Sort::Name => idx.sort_by_key(|&i| r[i].name.to_lowercase()),
        }
        idx
    }

    fn selected_repo(&self) -> Option<&RepoStatus> {
        let i = *self.visible().get(self.table.selected()?)?;
        self.repos.get(i)
    }

    fn detail_repo(&self) -> Option<&RepoStatus> {
        let name = self.detail.as_deref()?;
        self.repos.iter().find(|r| r.name == name)
    }

    fn selected_detail_run(&self) -> Option<&Run> {
        self.detail_repo()?.runs.get(self.runs_table.selected()?)
    }

    /// The run that run-actions apply to: the selected run in the runs view,
    /// or the latest run of the selected repo in the repo list.
    fn target_run(&self) -> Option<(String, Run)> {
        match self.view {
            View::Repos => self
                .selected_repo()
                .and_then(|r| Some((r.name.clone(), r.latest()?.clone()))),
            View::Runs => Some((self.detail.clone()?, self.selected_detail_run()?.clone())),
            View::Picker => None,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let (len, state) = match self.view {
            View::Repos => (self.visible().len(), &mut self.table),
            View::Runs => (
                self.detail_repo().map_or(0, |r| r.runs.len()),
                &mut self.runs_table,
            ),
            View::Picker => {
                let len = self.picker_rows().len();
                let Some(p) = &mut self.picker else { return };
                (len, &mut p.table)
            }
        };
        if len == 0 {
            state.select(None);
            return;
        }
        let cur = state.selected().unwrap_or(0) as isize;
        state.select(Some((cur + delta).clamp(0, len as isize - 1) as usize));
    }

    // ---------------------------------------------------------------- input

    fn on_key(&mut self, key: KeyEvent, terminal: &mut DefaultTerminal) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return Ok(());
        }
        if self.show_help {
            self.show_help = false;
            return Ok(());
        }
        if let Some(confirm) = self.confirm.take() {
            if matches!(key.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter) {
                match confirm.action {
                    ConfirmAction::Gh { repo, args, done } => self.run_gh_action(repo, args, done),
                    ConfirmAction::Untrack(repo) => self.untrack(&repo),
                }
            }
            return Ok(());
        }
        if self.view == View::Picker {
            self.on_picker_key(key);
            return Ok(());
        }
        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filtering = false;
                }
                KeyCode::Enter | KeyCode::Down | KeyCode::Up => self.filtering = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.table.select(Some(0));
            return Ok(());
        }

        use KeyCode::*;
        match (self.view, key.code) {
            (_, Char('q')) => self.quit = true,
            (_, Char('?')) => self.show_help = true,
            (_, Char('v')) => self.show_details = !self.show_details,
            (_, Char('r')) => {
                if self.pending == 0 {
                    self.refresh_all();
                } else {
                    self.set_flash("already refreshing…", Color::Yellow);
                }
            }
            (_, Down | Char('j')) => self.move_selection(1),
            (_, Up | Char('k')) => self.move_selection(-1),
            (_, PageDown) => self.move_selection(10),
            (_, PageUp) => self.move_selection(-10),
            (_, Home | Char('g')) => self.move_selection(isize::MIN / 2),
            (_, End | Char('G')) => self.move_selection(isize::MAX / 2),
            (_, Char('l')) => self.logs(terminal)?,
            (_, Char('w')) => self.watch(terminal)?,
            (_, Char('R')) => self.ask_rerun(),
            (_, Char('C')) => self.ask_cancel(),
            (_, Char('O')) => {
                if let Some((_, run)) = self.target_run() {
                    open_url(&run.html_url);
                }
            }

            (View::Repos, Esc) => {
                if self.filter.is_empty() {
                    self.quit = true;
                } else {
                    self.filter.clear();
                }
            }
            (View::Repos, Char('/')) => {
                self.filtering = true;
                self.table.select(Some(0));
            }
            (View::Repos, Char('s')) => {
                let name = self.selected_repo().map(|r| r.name.clone());
                self.sort = self.sort.next();
                let pos =
                    name.and_then(|n| self.visible().iter().position(|&i| self.repos[i].name == n));
                self.table.select(Some(pos.unwrap_or(0)));
            }
            (View::Repos, Enter | Right) => {
                if let Some(name) = self.selected_repo().map(|r| r.name.clone()) {
                    self.detail = Some(name);
                    self.view = View::Runs;
                    self.runs_table.select(Some(0));
                }
            }
            (View::Repos, Char('+')) => self.open_picker(),
            (View::Repos, Char('d')) => {
                if let Some(name) = self.selected_repo().map(|r| r.name.clone()) {
                    self.confirm = Some(Confirm {
                        prompt: format!("Stop tracking {name}? (y/n)"),
                        action: ConfirmAction::Untrack(name),
                    });
                }
            }
            (View::Repos, Char('o')) => self.open_repo_page(""),
            (View::Repos, Char('a')) => self.open_repo_page("/actions"),
            (View::Repos, Char('p')) => self.open_repo_page("/pulls"),
            (View::Repos, Char('i')) => self.open_repo_page("/issues"),

            (View::Runs, Esc | Backspace | Left | Char('h')) => {
                self.view = View::Repos;
                self.detail = None;
            }
            (View::Runs, Char('o')) => {
                if let Some(run) = self.selected_detail_run() {
                    open_url(&run.html_url);
                }
            }
            (View::Runs, Enter) => {
                if let Some((repo, run)) = self.target_run() {
                    let mut cmd = Command::new("gh");
                    cmd.args(["run", "view", &run.id.to_string(), "-R", &repo]);
                    self.suspend(terminal, true, || cmd.status().map(drop))?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn open_repo_page(&self, suffix: &str) {
        if let Some(repo) = self.selected_repo() {
            open_url(&format!("{}{suffix}", repo.url()));
        }
    }

    fn logs(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some((repo, run)) = self.target_run() else {
            self.set_flash("no workflow run selected", Color::Yellow);
            return Ok(());
        };
        if run.state().is_active() {
            return self.watch(terminal);
        }
        let flag = if run.state() == State::Failure {
            "--log-failed"
        } else {
            "--log"
        };
        self.suspend(terminal, false, || page_logs(&repo, run.id, flag))
    }

    fn watch(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let Some((repo, run)) = self.target_run() else {
            self.set_flash("no workflow run selected", Color::Yellow);
            return Ok(());
        };
        let mut cmd = Command::new("gh");
        cmd.args(["run", "watch", &run.id.to_string(), "-R", &repo]);
        self.suspend(terminal, true, || cmd.status().map(drop))?;
        self.spawn_fetch(vec![repo], false);
        Ok(())
    }

    fn ask_rerun(&mut self) {
        let Some((repo, run)) = self.target_run() else {
            return;
        };
        if run.state().is_active() {
            self.set_flash("run is still in progress", Color::Yellow);
            return;
        }
        let failed_only = run.state() == State::Failure;
        let mut args = vec![
            "run".into(),
            "rerun".into(),
            run.id.to_string(),
            "-R".into(),
            repo.clone(),
        ];
        if failed_only {
            args.push("--failed".into());
        }
        let what = if failed_only { "failed jobs of " } else { "" };
        self.confirm = Some(Confirm {
            prompt: format!(
                "Re-run {what}{} #{} in {repo}? (y/n)",
                run.workflow(),
                run.run_number
            ),
            action: ConfirmAction::Gh {
                done: format!("re-run of {} #{} requested", run.workflow(), run.run_number),
                repo,
                args,
            },
        });
    }

    fn ask_cancel(&mut self) {
        let Some((repo, run)) = self.target_run() else {
            return;
        };
        if !run.state().is_active() {
            self.set_flash("run is not in progress", Color::Yellow);
            return;
        }
        self.confirm = Some(Confirm {
            prompt: format!(
                "Cancel {} #{} in {repo}? (y/n)",
                run.workflow(),
                run.run_number
            ),
            action: ConfirmAction::Gh {
                done: format!("cancelled {} #{}", run.workflow(), run.run_number),
                args: vec![
                    "run".into(),
                    "cancel".into(),
                    run.id.to_string(),
                    "-R".into(),
                    repo.clone(),
                ],
                repo,
            },
        });
    }

    fn run_gh_action(&mut self, repo: String, args: Vec<String>, done: String) {
        let tx = self.tx.clone();
        self.set_flash("working…", Color::Yellow);
        std::thread::spawn(move || {
            let (ok, text) = match Command::new("gh").args(&args).stdin(Stdio::null()).output() {
                Ok(o) if o.status.success() => (true, done),
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr);
                    (false, err.lines().next().unwrap_or("gh failed").to_string())
                }
                Err(e) => (false, e.to_string()),
            };
            let _ = tx.send(Msg::Action { repo, text, ok });
        });
    }

    /// Hands the terminal to a child process, then takes it back.
    fn suspend(
        &mut self,
        terminal: &mut DefaultTerminal,
        pause: bool,
        run: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<()> {
        disable_raw_mode()?;
        execute!(std::io::stdout(), LeaveAlternateScreen, cursor::Show)?;
        let status = run();
        if pause {
            print!("\n\x1b[2m[press Enter to return to gh radar]\x1b[0m");
            let _ = std::io::stdout().flush();
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        enable_raw_mode()?;
        execute!(std::io::stdout(), EnterAlternateScreen, cursor::Hide)?;
        // Force a full redraw. `Terminal::clear` would query the cursor position,
        // which fails on terminals that don't answer that query.
        let size = terminal.size()?;
        terminal.resize(Rect::new(0, 0, size.width, size.height))?;
        if let Err(e) = status {
            self.set_flash(format!("failed to run command: {e}"), Color::Red);
        }
        Ok(())
    }

    // --------------------------------------------------------------- picker

    fn open_picker(&mut self) {
        self.picker = Some(Picker {
            query: String::new(),
            owner: None,
            show_archived: false,
            checked: self.config.repos.iter().map(|r| r.to_lowercase()).collect(),
            table: TableState::default().with_selected(0),
        });
        self.view = View::Picker;
        if self.accessible.is_none() {
            self.load_accessible();
        }
    }

    fn load_accessible(&mut self) {
        if self.accessible_loading {
            return;
        }
        self.accessible_loading = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Accessible(
                github::fetch_accessible().map_err(|e| e.to_string()),
            ));
        });
    }

    fn accessible_list(&self) -> &[Available] {
        match &self.accessible {
            Some(Ok(list)) => list,
            _ => &[],
        }
    }

    /// Indices into the accessible list that match the picker's filters.
    fn picker_rows(&self) -> Vec<usize> {
        let Some(p) = &self.picker else {
            return Vec::new();
        };
        let query = p.query.to_lowercase();
        self.accessible_list()
            .iter()
            .enumerate()
            .filter(|(_, r)| p.show_archived || !r.archived)
            .filter(|(_, r)| {
                p.owner
                    .as_deref()
                    .is_none_or(|o| r.owner().eq_ignore_ascii_case(o))
            })
            .filter(|(_, r)| query.is_empty() || r.full_name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect()
    }

    /// Owners in the accessible list, most repos first.
    fn picker_owners(&self) -> Vec<String> {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for r in self.accessible_list() {
            match counts.iter_mut().find(|(o, _)| o == r.owner()) {
                Some((_, n)) => *n += 1,
                None => counts.push((r.owner().to_string(), 1)),
            }
        }
        counts.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        counts.into_iter().map(|(o, _)| o).collect()
    }

    fn on_picker_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let rows = self.picker_rows();
        let owners = self.picker_owners();
        let current = self
            .picker
            .as_ref()
            .and_then(|p| p.table.selected())
            .and_then(|i| rows.get(i))
            .map(|&i| self.accessible_list()[i].full_name.to_lowercase());
        let visible_names: Vec<String> = rows
            .iter()
            .map(|&i| self.accessible_list()[i].full_name.to_lowercase())
            .collect();
        let Some(p) = &mut self.picker else { return };

        match key.code {
            KeyCode::Esc if !p.query.is_empty() => p.query.clear(),
            KeyCode::Esc => {
                self.picker = None;
                self.view = View::Repos;
                return;
            }
            KeyCode::Enter => {
                self.save_picker();
                return;
            }
            KeyCode::Down => return self.move_selection(1),
            KeyCode::Up => return self.move_selection(-1),
            KeyCode::Char('n') if ctrl => return self.move_selection(1),
            KeyCode::Char('p') if ctrl => return self.move_selection(-1),
            KeyCode::PageDown => return self.move_selection(10),
            KeyCode::PageUp => return self.move_selection(-10),
            KeyCode::Char(' ') => {
                if let Some(name) = current
                    && !p.checked.remove(&name)
                {
                    p.checked.insert(name);
                }
                return self.move_selection(1);
            }
            KeyCode::Char('a') if ctrl => {
                if visible_names.iter().all(|n| p.checked.contains(n)) {
                    for n in &visible_names {
                        p.checked.remove(n);
                    }
                } else {
                    p.checked.extend(visible_names);
                }
                return;
            }
            KeyCode::Char('x') if ctrl => p.show_archived = !p.show_archived,
            KeyCode::Char('r') if ctrl => {
                self.accessible = None;
                self.load_accessible();
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let pos = p
                    .owner
                    .as_ref()
                    .and_then(|o| owners.iter().position(|x| x == o));
                // Cycle through "all" (None) followed by each owner.
                let n = owners.len() + 1;
                let cur = pos.map_or(0, |i| i + 1);
                let next = if key.code == KeyCode::Tab {
                    (cur + 1) % n
                } else {
                    (cur + n - 1) % n
                };
                p.owner = next.checked_sub(1).map(|i| owners[i].clone());
            }
            KeyCode::Backspace => {
                p.query.pop();
            }
            KeyCode::Char(c) if !ctrl => p.query.push(c),
            _ => return,
        }
        p.table.select(Some(0));
    }

    fn save_picker(&mut self) {
        let Some(p) = self.picker.take() else { return };
        self.view = View::Repos;
        let tracked: HashSet<String> = self.config.repos.iter().map(|r| r.to_lowercase()).collect();
        let to_remove: Vec<String> = self
            .config
            .repos
            .iter()
            .filter(|r| !p.checked.contains(&r.to_lowercase()))
            .cloned()
            .collect();
        let to_add: Vec<String> = self
            .accessible_list()
            .iter()
            .filter(|r| {
                let n = r.full_name.to_lowercase();
                p.checked.contains(&n) && !tracked.contains(&n)
            })
            .map(|r| r.full_name.clone())
            .collect();
        if to_add.is_empty() && to_remove.is_empty() {
            return;
        }

        for name in &to_remove {
            self.config.remove(name);
            self.repos.retain(|r| !r.name.eq_ignore_ascii_case(name));
        }
        for name in &to_add {
            self.config.add(name);
            self.repos.push(RepoStatus::new(name));
        }
        match self.config.save() {
            Ok(()) => self.set_flash(
                format!(
                    "now tracking {} (+{} −{})",
                    self.config.repos.len(),
                    to_add.len(),
                    to_remove.len()
                ),
                Color::Green,
            ),
            Err(e) => self.set_flash(format!("failed to save config: {e}"), Color::Red),
        }
        self.table.select(Some(0));
        self.spawn_fetch(to_add, true);
    }

    fn untrack(&mut self, name: &str) {
        self.config.remove(name);
        self.repos.retain(|r| !r.name.eq_ignore_ascii_case(name));
        match self.config.save() {
            Ok(()) => self.set_flash(format!("stopped tracking {name}"), Color::Green),
            Err(e) => self.set_flash(format!("failed to save config: {e}"), Color::Red),
        }
        self.move_selection(0);
    }

    // -------------------------------------------------------------- drawing

    fn draw(&mut self, f: &mut Frame) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(f.area());
        self.draw_header(f, header);
        match self.view {
            View::Repos => self.draw_repos(f, body),
            View::Runs => self.draw_runs(f, body),
            View::Picker => self.draw_picker(f, body),
        }
        self.draw_footer(f, footer);
        if self.show_help {
            draw_help(f);
        }
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let failing = self
            .repos
            .iter()
            .filter(|r| r.latest().is_some_and(|r| r.state() == State::Failure))
            .count();
        let running = self.repos.iter().filter(|r| r.has_active_runs()).count();
        let mut left = vec![
            Span::styled(
                " gh radar ",
                Style::new().bold().fg(Color::Black).bg(Color::Cyan),
            ),
            Span::raw(format!(
                "  {} repo{}",
                self.repos.len(),
                if self.repos.len() == 1 { "" } else { "s" }
            )),
        ];
        if failing > 0 {
            left.push(Span::styled(
                format!("  ✗ {failing} failing"),
                Style::new().fg(Color::Red).bold(),
            ));
        }
        if running > 0 {
            left.push(Span::styled(
                format!("  ● {running} running"),
                Style::new().fg(Color::Yellow),
            ));
        }
        f.render_widget(Line::from(left), area);

        let status = if self.pending > 0 {
            format!("{} refreshing", spinner())
        } else if let Some(t) = self.last_updated {
            match ago(t, Utc::now()).as_str() {
                "now" => "updated just now".to_string(),
                a => format!("updated {a} ago"),
            }
        } else {
            String::new()
        };
        let mut right = format!("sort: {} ", self.sort.label());
        if !status.is_empty() {
            right.push_str(&format!("· {status} "));
        }
        let right = Line::from(right).dark_gray();
        f.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
    }

    fn draw_repos(&mut self, f: &mut Frame, area: Rect) {
        let visible = self.visible();
        if visible.is_empty() {
            let msg = if self.repos.is_empty() {
                "No repos tracked yet. Press + to pick repos you have access to."
            } else {
                "No repos match the filter."
            };
            f.render_widget(
                Paragraph::new(msg).dark_gray().alignment(Alignment::Center),
                area.inner_y(2),
            );
            return;
        }

        let (table_area, preview_area) = self.split_for_details(area, 210, 64, 11);
        if let Some(preview) = preview_area
            && let Some(repo) = self.selected_repo()
        {
            self.draw_repo_details(f, preview, repo);
        }

        let now = Utc::now();
        let show_recent = table_area.width >= 100;
        let show_issues = table_area.width >= 140;
        let latest: Vec<Option<&Run>> = visible.iter().map(|&i| self.repos[i].latest()).collect();
        let branch_w = latest
            .iter()
            .flatten()
            .map(|r| (r.branch().chars().count() + 2).max(r.event.len()))
            .max()
            .unwrap_or(8)
            .clamp(8, 26) as u16;
        let main_w = visible
            .iter()
            .filter_map(|&i| self.repos[i].meta.default_branch.as_ref())
            .map(|b| b.chars().count() + 2)
            .max()
            .unwrap_or(6)
            .clamp(6, 16) as u16;

        let mut widths = vec![
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(branch_w),
        ];
        let mut header = vec!["", "REPOSITORY · LATEST RUN", "BRANCH"];
        if show_recent {
            widths.push(Constraint::Length(HISTORY as u16));
            header.push("RECENT");
        }
        widths.extend([
            Constraint::Length(6),
            Constraint::Length(main_w),
            Constraint::Length(3),
        ]);
        header.extend(["AGE", "MAIN", "PRS"]);
        if show_issues {
            widths.push(Constraint::Length(6));
            header.push("ISSUES");
        }
        let fill_w = fill_width(table_area.width, &widths);

        let rows: Vec<Row> = visible
            .iter()
            .map(|&i| {
                let r = &self.repos[i];
                let (owner, name) = r.name.split_once('/').unwrap_or(("", &r.name));
                let mut title = vec![
                    Span::raw(format!("{owner}/")).dark_gray(),
                    Span::raw(name.to_string()).bold(),
                ];
                if r.meta.archived {
                    title.push(Span::raw("  archived").dark_gray().italic());
                }

                let empty = || Line::raw("");
                let (icon, subtitle, branch, trigger, age, took) = if !r.loaded {
                    let loading = Line::from("loading…").dark_gray();
                    (
                        Span::raw("…").dark_gray(),
                        loading,
                        empty(),
                        empty(),
                        empty(),
                        empty(),
                    )
                } else if let Some(err) = &r.error {
                    (
                        Span::raw("!").red().bold(),
                        fit(vec![Span::raw(err.clone()).red()], fill_w),
                        empty(),
                        empty(),
                        empty(),
                        empty(),
                    )
                } else if let Some(run) = r.latest() {
                    (
                        state_span(run.state()),
                        fit(
                            vec![
                                Span::raw(run.workflow().to_string()).fg(state_color(run.state())),
                                Span::raw(" · ").dark_gray(),
                                Span::raw(run.display_title.clone()),
                            ],
                            fill_w,
                        ),
                        fit(
                            vec![Span::raw(format!("⎇ {}", run.branch())).magenta()],
                            branch_w as usize,
                        ),
                        Line::from(run.event.clone()).dark_gray(),
                        Line::from(ago(run.created_at, now)),
                        Line::from(duration(run.duration(now))).dark_gray(),
                    )
                } else {
                    (
                        Span::raw("·").dark_gray(),
                        Line::from("no workflow runs").dark_gray(),
                        empty(),
                        empty(),
                        empty(),
                        empty(),
                    )
                };
                let main = match (r.default_branch_run(), &r.meta.default_branch) {
                    (Some(run), Some(b)) => Line::from(vec![
                        state_span(run.state()),
                        Span::raw(format!(" {b}")).dark_gray(),
                    ]),
                    _ => empty(),
                };
                let count = |n: Option<u64>| match n {
                    Some(0) => Line::from("0").dark_gray().right_aligned(),
                    Some(n) => Line::from(n.to_string()).right_aligned(),
                    None => empty(),
                };

                let mut cells = vec![
                    Cell::from(Line::from(icon)),
                    Cell::from(Text::from(vec![fit(title, fill_w), subtitle])),
                    Cell::from(Text::from(vec![branch, trigger])),
                ];
                if show_recent {
                    cells.push(Cell::from(history(&r.runs)));
                }
                cells.extend([
                    Cell::from(Text::from(vec![age, took])),
                    Cell::from(main),
                    Cell::from(count(r.meta.open_prs)),
                ]);
                if show_issues {
                    cells.push(Cell::from(count(r.meta.open_issues)));
                }
                Row::new(cells).height(2)
            })
            .collect();

        let table = Table::new(rows, widths)
            .header(Row::new(header).style(Style::new().dark_gray().bold()))
            .column_spacing(COLUMN_SPACING)
            .row_highlight_style(highlight_style())
            .highlight_symbol(Text::from(vec![
                Line::from("▌ ").cyan(),
                Line::from("▌ ").cyan(),
            ]));
        f.render_stateful_widget(table, table_area, &mut self.table);
    }

    fn draw_repo_details(&self, f: &mut Frame, area: Rect, repo: &RepoStatus) {
        let now = Utc::now();
        let block = Block::bordered()
            .padding(Padding::horizontal(1))
            .border_style(Style::new().dark_gray())
            .title(Line::from(format!(" {} ", repo.name)).bold().white());
        let inner = block.inner(area);
        f.render_widget(block, area);

        let run = repo.latest();

        let mut lines = Vec::new();
        if let Some(d) = &repo.meta.description {
            lines.push(Line::from(d.clone()).dark_gray().italic());
        }
        if let Some(e) = &repo.error {
            lines.push(Line::from(e.clone()).red());
        }
        if let Some(run) = run {
            if !lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.extend(run_summary(run, now));
            if let (Some(main), Some(b)) = (repo.default_branch_run(), &repo.meta.default_branch)
                && main.id != run.id
            {
                lines.push(Line::from(vec![
                    Span::raw(format!("latest on {b}: ")).dark_gray(),
                    state_span(main.state()),
                    Span::raw(format!(
                        " {} · {} ago",
                        main.workflow(),
                        ago(main.created_at, now)
                    ))
                    .dark_gray(),
                ]));
            }
        } else if repo.loaded && repo.error.is_none() {
            lines.push(Line::from("No workflow runs.").dark_gray());
        }
        let Some(run) = run else {
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
            return;
        };
        let jobs = self.jobs_lines(run, now);
        if inner.width >= 90 {
            // Wide bottom pane: summary and jobs side by side.
            let [a, b] =
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .spacing(3)
                    .areas(inner);
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), a);
            f.render_widget(Paragraph::new(jobs), b);
        } else {
            // Narrow side pane: jobs below the summary.
            lines.push(Line::raw(""));
            lines.extend(jobs);
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
    }

    fn draw_runs(&mut self, f: &mut Frame, area: Rect) {
        let Some(repo) = self.detail_repo() else {
            return;
        };
        let now = Utc::now();
        let [info, body] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

        let mut title = vec![Span::raw(format!(" {}", repo.name)).bold().cyan()];
        if let Some(d) = &repo.meta.description {
            title.push(Span::raw(format!("  {d}")).dark_gray());
        }
        let mut facts = Vec::new();
        if let Some(b) = &repo.meta.default_branch {
            facts.push(format!("default branch {b}"));
        }
        if let Some(n) = repo.meta.open_prs {
            facts.push(format!("{n} open PRs"));
        }
        if let Some(n) = repo.meta.open_issues {
            facts.push(format!("{n} open issues"));
        }
        if let Some(t) = repo.meta.pushed_at {
            facts.push(format!("pushed {} ago", ago(t, now)));
        }
        let second = match &repo.error {
            Some(e) => Line::from(format!(" {e}")).red(),
            None => Line::from(format!(" {}", facts.join(" · "))).dark_gray(),
        };
        f.render_widget(
            Paragraph::new(vec![fit(title, area.width as usize), second]),
            info,
        );

        if repo.runs.is_empty() {
            f.render_widget(
                Paragraph::new("No workflow runs.")
                    .dark_gray()
                    .alignment(Alignment::Center),
                body,
            );
            return;
        }

        let (table_area, jobs_area) = self.split_for_details(body, 110, 56, 12);
        if let Some(jobs_area) = jobs_area
            && let Some(run) = self.selected_detail_run()
        {
            let block = Block::bordered()
                .padding(Padding::horizontal(1))
                .border_style(Style::new().dark_gray())
                .title(
                    Line::from(format!(" {} #{} ", run.workflow(), run.run_number))
                        .bold()
                        .white(),
                );
            let inner = block.inner(jobs_area);
            f.render_widget(block, jobs_area);
            let mut lines = run_summary(run, now);
            lines.push(Line::raw(""));
            lines.extend(self.jobs_lines(run, now));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }

        let Some(repo) = self.detail_repo() else {
            return;
        };
        let meta_w = repo
            .runs
            .iter()
            .map(|r| {
                let trigger = r.event.len() + r.actor.as_ref().map_or(0, |a| a.login.len() + 3);
                (r.branch().chars().count() + 2).max(trigger)
            })
            .max()
            .unwrap_or(10)
            .clamp(10, 32) as u16;
        let widths = vec![
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Fill(1),
            Constraint::Length(meta_w),
            Constraint::Length(6),
        ];
        let fill_w = fill_width(table_area.width, &widths);

        let rows: Vec<Row> = repo
            .runs
            .iter()
            .map(|run| {
                let attempt = match run.run_attempt {
                    Some(a) if a > 1 => Line::from(format!("try {a}")).dark_gray(),
                    _ => Line::raw(""),
                };
                let trigger = match &run.actor {
                    Some(a) => format!("{} · {}", run.event, a.login),
                    None => run.event.clone(),
                };
                Row::new(vec![
                    Cell::from(Line::from(state_span(run.state()))),
                    Cell::from(Text::from(vec![
                        Line::from(format!("#{}", run.run_number)).dark_gray(),
                        attempt,
                    ])),
                    Cell::from(Text::from(vec![
                        fit(
                            vec![
                                Span::raw(run.workflow().to_string())
                                    .bold()
                                    .fg(state_color(run.state())),
                            ],
                            fill_w,
                        ),
                        fit(vec![Span::raw(run.display_title.clone())], fill_w),
                    ])),
                    Cell::from(Text::from(vec![
                        fit(
                            vec![Span::raw(format!("⎇ {}", run.branch())).magenta()],
                            meta_w as usize,
                        ),
                        fit(vec![Span::raw(trigger).dark_gray()], meta_w as usize),
                    ])),
                    Cell::from(Text::from(vec![
                        Line::from(ago(run.created_at, now)),
                        Line::from(duration(run.duration(now))).dark_gray(),
                    ])),
                ])
                .height(2)
            })
            .collect();

        let table = Table::new(rows, widths)
            .header(
                Row::new(["", "RUN", "WORKFLOW · TITLE", "BRANCH", "AGE"])
                    .style(Style::new().dark_gray().bold()),
            )
            .column_spacing(COLUMN_SPACING)
            .row_highlight_style(highlight_style())
            .highlight_symbol(Text::from(vec![
                Line::from("▌ ").cyan(),
                Line::from("▌ ").cyan(),
            ]));
        f.render_stateful_widget(table, table_area, &mut self.runs_table);
    }

    /// Splits off a details pane: to the right when there is room for it,
    /// otherwise below, or not at all on tiny terminals or when hidden with `v`.
    fn split_for_details(
        &self,
        area: Rect,
        side_min_width: u16,
        side_width: u16,
        bottom_height: u16,
    ) -> (Rect, Option<Rect>) {
        if !self.show_details {
            return (area, None);
        }
        if area.width >= side_min_width {
            let [a, b] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(side_width)])
                .spacing(1)
                .areas(area);
            (a, Some(b))
        } else if area.height >= bottom_height * 2 {
            let [a, b] = Layout::vertical([Constraint::Fill(1), Constraint::Length(bottom_height)])
                .areas(area);
            (a, Some(b))
        } else {
            (area, None)
        }
    }

    /// Jobs of a run, failures first, with the failing (or running) step under each job.
    fn jobs_lines(&self, run: &Run, now: chrono::DateTime<Utc>) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from("JOBS").dark_gray().bold()];
        let Some(entry) = self.jobs.get(&run.id) else {
            lines.push(Line::from(format!("{} loading…", spinner())).dark_gray());
            return lines;
        };
        let jobs = match &entry.jobs {
            Err(e) => {
                lines.push(Line::from(e.clone()).red());
                return lines;
            }
            Ok(jobs) if jobs.is_empty() => {
                lines.push(Line::from("no jobs").dark_gray());
                return lines;
            }
            Ok(jobs) => jobs,
        };
        let mut jobs: Vec<_> = jobs.iter().collect();
        jobs.sort_by_key(|j| j.state());
        for job in jobs {
            let state = job.state();
            let mut spans = vec![state_span(state), Span::raw(" ")];
            let name = Span::raw(job.name.clone());
            spans.push(if state == State::Failure {
                name.red().bold()
            } else {
                name
            });
            if let Some(d) = job.duration(now) {
                spans.push(Span::raw(format!("  {}", duration(d))).dark_gray());
            }
            lines.push(Line::from(spans));
            for step in job
                .steps
                .iter()
                .filter(|s| matches!(s.state(), State::Failure | State::Running))
            {
                lines.push(Line::from(vec![
                    Span::raw("  └ ").dark_gray(),
                    state_span(step.state()),
                    Span::raw(format!(" {}", step.name)).fg(state_color(step.state())),
                ]));
            }
        }
        lines
    }

    fn draw_picker(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.picker_rows();
        let owners = self.picker_owners();
        let tracked: HashSet<String> = self.config.repos.iter().map(|r| r.to_lowercase()).collect();
        let Some(p) = &self.picker else { return };
        let [title, tabs, search, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .areas(area);

        let adding = p.checked.iter().filter(|n| !tracked.contains(*n)).count();
        let removing = tracked.iter().filter(|n| !p.checked.contains(*n)).count();
        let mut t = vec![
            Span::raw(" Add repositories").bold(),
            Span::raw(format!(
                "  {} shown of {}",
                rows.len(),
                self.accessible_list().len()
            ))
            .dark_gray(),
        ];
        if adding > 0 {
            t.push(Span::raw(format!("  +{adding}")).green().bold());
        }
        if removing > 0 {
            t.push(Span::raw(format!("  −{removing}")).red().bold());
        }
        if p.show_archived {
            t.push(Span::raw("  incl. archived").dark_gray());
        }
        f.render_widget(Line::from(t), title);

        let mut tab_spans = vec![Span::raw(" ")];
        let names = std::iter::once(None).chain(owners.iter().map(Some));
        for owner in names {
            let label = owner.map_or("all", |o| o.as_str());
            let active = owner.map(|o| o.as_str()) == p.owner.as_deref();
            let span = Span::raw(format!(" {label} "));
            tab_spans.push(if active {
                span.black().on_cyan().bold()
            } else {
                span.dark_gray()
            });
            tab_spans.push(Span::raw(" "));
        }
        f.render_widget(Line::from(tab_spans), tabs);

        let search_line = Line::from(vec![
            Span::raw(" › ").cyan().bold(),
            Span::raw(p.query.clone()),
            Span::raw("█").dark_gray(),
        ]);
        f.render_widget(search_line, search);

        if rows.is_empty() {
            let msg = match &self.accessible {
                _ if self.accessible_loading => Line::from(format!(
                    "{} loading repositories you have access to…",
                    spinner()
                )),
                Some(Err(e)) => Line::from(format!("{e}  (ctrl-r to retry)")).red(),
                _ => Line::from("no repositories match").dark_gray(),
            };
            f.render_widget(
                Paragraph::new(msg).alignment(Alignment::Center),
                body.inner_y(1),
            );
            return;
        }

        let now = Utc::now();
        let list = self.accessible_list();
        let name_w = rows
            .iter()
            .map(|&i| list[i].full_name.len())
            .max()
            .unwrap_or(10)
            .clamp(10, 64) as u16;
        let table_rows = rows.iter().map(|&i| {
            let r = &list[i];
            let n = r.full_name.to_lowercase();
            let mark = match (p.checked.contains(&n), tracked.contains(&n)) {
                (true, true) => Span::raw("[x]").cyan(),
                (true, false) => Span::raw("[+]").green().bold(),
                (false, true) => Span::raw("[-]").red().bold(),
                (false, false) => Span::raw("[ ]").dark_gray(),
            };
            let (owner, name) = r.full_name.split_once('/').unwrap_or(("", &r.full_name));
            let mut tags = Vec::new();
            if r.private {
                tags.push("private");
            }
            if r.fork {
                tags.push("fork");
            }
            if r.archived {
                tags.push("archived");
            }
            Row::new(vec![
                Cell::from(mark),
                Cell::from(Line::from(vec![
                    Span::raw(format!("{owner}/")).dark_gray(),
                    Span::raw(name.to_string()),
                ])),
                Cell::from(Span::raw(tags.join(" ")).dark_gray()),
                Cell::from(
                    Span::raw(r.pushed_at.map(|t| ago(t, now)).unwrap_or_default()).dark_gray(),
                ),
                Cell::from(Span::raw(r.description.clone().unwrap_or_default()).dark_gray()),
            ])
        });
        let table = Table::new(
            table_rows,
            [
                Constraint::Length(3),
                Constraint::Length(name_w),
                Constraint::Length(16),
                Constraint::Length(4),
                Constraint::Fill(1),
            ],
        )
        .column_spacing(2)
        .row_highlight_style(highlight_style())
        .highlight_symbol(Line::from("▌ ").cyan());
        let mut state = p.table;
        f.render_stateful_widget(table, body, &mut state);
        if let Some(p) = &mut self.picker {
            p.table = state;
        }
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect) {
        let line = if let Some(c) = &self.confirm {
            Line::from(format!(" {}", c.prompt)).yellow().bold()
        } else if self.filtering {
            Line::from(vec![
                Span::raw(" /").cyan(),
                Span::raw(self.filter.clone()),
                Span::raw("█").dark_gray(),
            ])
        } else if let Some((text, color, _)) = self
            .flash
            .as_ref()
            .filter(|(_, _, at)| at.elapsed() < FLASH_FOR)
        {
            Line::from(format!(" {text}")).fg(*color)
        } else {
            let hints: &[(&str, &str)] = match self.view {
                View::Repos => &[
                    ("?", "help"),
                    ("enter", "runs"),
                    ("+", "add"),
                    ("l", "logs"),
                    ("w", "watch"),
                    ("R", "rerun"),
                    ("o", "open"),
                    ("/", "filter"),
                    ("s", "sort"),
                    ("v", "details"),
                    ("a", "actions"),
                    ("p", "PRs"),
                    ("d", "untrack"),
                    ("q", "quit"),
                ],
                View::Runs => &[
                    ("?", "help"),
                    ("esc", "back"),
                    ("enter", "summary"),
                    ("l", "logs"),
                    ("w", "watch"),
                    ("R", "rerun"),
                    ("C", "cancel"),
                    ("o", "open"),
                    ("v", "details"),
                ],
                View::Picker => &[
                    ("type", "search"),
                    ("space", "toggle"),
                    ("enter", "save"),
                    ("esc", "cancel"),
                    ("tab", "owner"),
                    ("ctrl-a", "toggle shown"),
                    ("ctrl-x", "archived"),
                ],
            };
            let mut spans = vec![Span::raw(" ")];
            let mut used = 1;
            if !self.filter.is_empty() && self.view == View::Repos {
                let label = format!("filter: {}  ", self.filter);
                used += label.chars().count();
                spans.push(Span::raw(label).cyan());
            }
            for (key, desc) in hints {
                let w = key.chars().count() + desc.chars().count() + 3;
                if used + w > area.width as usize {
                    break;
                }
                used += w;
                spans.push(Span::raw(*key).cyan().bold());
                spans.push(Span::raw(format!(" {desc}  ")).dark_gray());
            }
            Line::from(spans)
        };
        f.render_widget(Paragraph::new(line), area);
    }
}

const COLUMN_SPACING: u16 = 2;
const HISTORY: usize = 10;

fn highlight_style() -> Style {
    Style::new().bg(Color::Indexed(236))
}

/// Width of the single `Fill` column once fixed columns, spacing and the
/// highlight symbol are accounted for.
fn fill_width(total: u16, widths: &[Constraint]) -> usize {
    let fixed: u16 = widths
        .iter()
        .map(|c| if let Constraint::Length(n) = c { *n } else { 0 })
        .sum();
    let spacing = COLUMN_SPACING * (widths.len() as u16).saturating_sub(1);
    total.saturating_sub(fixed + spacing + 2) as usize
}

/// Joins spans into a line no wider than `width`, ending in `…` if cut.
fn fit(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut out = Vec::new();
    let mut used = 0;
    for span in spans {
        let len = span.content.chars().count();
        if used + len <= width {
            used += len;
            out.push(span);
            continue;
        }
        let keep = width.saturating_sub(used + 1);
        let text: String = span.content.chars().take(keep).collect();
        out.push(Span::styled(format!("{}…", text.trim_end()), span.style));
        break;
    }
    Line::from(out)
}

/// The last few run results as colored dots, newest on the right.
fn history(runs: &[Run]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(HISTORY.saturating_sub(runs.len())))];
    spans.extend(
        runs.iter()
            .take(HISTORY)
            .rev()
            .map(|r| Span::raw("●").fg(state_color(r.state()))),
    );
    Line::from(spans)
}

fn state_color(state: State) -> Color {
    match state {
        State::Success => Color::Green,
        State::Failure => Color::Red,
        State::Running => Color::Yellow,
        State::Queued => Color::Cyan,
        State::Neutral => Color::DarkGray,
        State::Unknown => Color::Gray,
    }
}

fn outcome(run: &Run) -> String {
    match run.state() {
        State::Success => "succeeded".into(),
        State::Failure if run.conclusion.as_deref() == Some("failure") => "failed".into(),
        State::Running => "running".into(),
        State::Queued => run.status.clone().unwrap_or_else(|| "queued".into()),
        _ => run
            .conclusion
            .clone()
            .unwrap_or_else(|| "unknown".into())
            .replace('_', " "),
    }
}

/// A few lines describing a run in full, for the details pane.
fn run_summary(run: &Run, now: chrono::DateTime<Utc>) -> Vec<Line<'static>> {
    let state = run.state();
    let mut trigger = format!("⎇ {} · {}", run.branch(), run.event);
    if let Some(a) = &run.actor {
        trigger.push_str(&format!(" · by {}", a.login));
    }
    vec![
        Line::from(vec![
            state_span(state),
            Span::raw(format!(" {}", run.workflow())).bold(),
            Span::raw(format!(" #{} ", run.run_number)).dark_gray(),
            Span::raw(outcome(run)).fg(state_color(state)).bold(),
        ]),
        Line::from(run.display_title.clone()),
        Line::from(trigger).magenta(),
        Line::from(format!(
            "started {} ago · took {}",
            ago(run.created_at, now),
            duration(run.duration(now))
        ))
        .dark_gray(),
    ]
}

fn attention(r: &RepoStatus) -> u8 {
    if r.error.is_some() {
        return 0;
    }
    let rank = |s: Option<State>| match s {
        Some(State::Failure) => 0,
        Some(State::Running) => 1,
        Some(State::Queued) => 2,
        _ => 3,
    };
    rank(r.latest().map(Run::state)).min(rank(r.default_branch_run().map(Run::state)))
}

fn spinner() -> &'static str {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    SPINNER[(ms / 150 % SPINNER.len() as u128) as usize]
}

fn state_span(state: State) -> Span<'static> {
    match state {
        State::Success => Span::raw("✓").green(),
        State::Failure => Span::raw("✗").red().bold(),
        State::Running => Span::raw(spinner()).yellow(),
        State::Queued => Span::raw("○").cyan(),
        State::Neutral => Span::raw("⊘").dark_gray(),
        State::Unknown => Span::raw("·").dark_gray(),
    }
}

fn draw_help(f: &mut Frame) {
    let keys: &[(&str, &str)] = &[
        ("j/k ↑/↓", "move"),
        ("g/G", "top / bottom"),
        ("enter", "repo: show runs · run: show summary in gh"),
        ("esc", "back / clear filter / quit"),
        ("o", "open repo (or run) in browser"),
        ("O", "open the selected/latest run in browser"),
        ("a / p / i", "open actions / pull requests / issues"),
        ("l", "logs (failed jobs only for failed runs)"),
        ("w", "watch a run live (gh run watch)"),
        ("R", "re-run (failed jobs) of the run"),
        ("C", "cancel an in-progress run"),
        ("/", "filter repos by name"),
        ("+", "add repos you have access to"),
        ("d", "stop tracking the selected repo"),
        ("s", "sort: activity → attention → name"),
        ("v", "show/hide the details pane (jobs, full titles)"),
        ("r", "refresh now"),
        ("q", "quit"),
    ];
    let mut lines: Vec<Line> = keys
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::raw(format!("  {k:<10}")).cyan().bold(),
                Span::raw(*d),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::from("  Actions apply to the latest run when on the repo list.").dark_gray());
    lines.push(Line::from("  Repos with running workflows refresh more often.").dark_gray());

    let height = lines.len() as u16 + 2;
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(f.area());
    let [area] = Layout::horizontal([Constraint::Length(64)])
        .flex(Flex::Center)
        .areas(area);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" keys ").cyan()),
        area,
    );
}

/// Pipes `gh run view --log…` into a pager, without needing a shell.
fn page_logs(repo: &str, run_id: u64, flag: &str) -> std::io::Result<()> {
    let pager = std::env::var("GH_RADAR_PAGER")
        .or_else(|_| std::env::var("PAGER"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "more".into()
            } else {
                "less -R +G".into()
            }
        });
    let mut parts = pager.split_whitespace();
    let program = parts.next().unwrap_or("less");

    let mut gh = Command::new("gh")
        .args(["run", "view", &run_id.to_string(), "-R", repo, flag])
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = gh.stdout.take().expect("stdout is piped");
    let result = Command::new(program).args(parts).stdin(stdout).status();
    // The pager may quit before gh is done writing; don't leave it behind.
    let _ = gh.kill();
    let _ = gh.wait();
    result.map(drop)
}

fn open_url(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = Command::new(opener)
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

fn desktop_notify(text: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        let quoted = text.replace('\\', "\\\\").replace('"', "\\\"");
        let mut c = Command::new("osascript");
        c.args([
            "-e",
            &format!("display notification \"{quoted}\" with title \"gh radar\""),
        ]);
        c
    } else if cfg!(target_os = "linux") {
        let mut c = Command::new("notify-send");
        c.args(["gh radar", text]);
        c
    } else {
        return;
    };
    let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

trait RectExt {
    fn inner_y(self, n: u16) -> Rect;
}

impl RectExt for Rect {
    fn inner_y(self, n: u16) -> Rect {
        let y = self.y + n.min(self.height);
        Rect {
            y,
            height: self.height - (y - self.y),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn fit_keeps_short_lines_and_cuts_long_ones() {
        let spans = || {
            vec![
                Span::raw("workflow"),
                Span::raw(" · "),
                Span::raw("a long commit title"),
            ]
        };
        assert_eq!(text(&fit(spans(), 100)), "workflow · a long commit title");
        assert_eq!(text(&fit(spans(), 16)), "workflow · a lo…");
        assert!(text(&fit(spans(), 16)).chars().count() <= 16);
    }
}
