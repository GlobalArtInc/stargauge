pub mod app;
pub mod cli;
pub mod diff;
pub mod error;
pub mod github;
pub mod model;
pub mod platform;
pub mod render;
pub mod stats;
pub mod store;
pub mod target;

use crate::app::{Ctx, output};
use crate::cli::{Cli, Command};
use crate::error::Error;
use crate::github::Client;
use crate::model::{RepoId, Zone};
use crate::render::term;
use crate::store::Layout;
use crate::target::Input;
use chrono::Utc;
use clap::Parser;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let mut out = io::stdout().lock();
    match dispatch(cli, &mut out) {
        Ok(code) => code,
        Err(err) => {
            let _ = writeln!(io::stderr(), "error: {err}");
            ExitCode::from(1)
        }
    }
}

fn dispatch(cli: Cli, out: &mut dyn Write) -> Result<ExitCode, Error> {
    let layout = Layout::discover()?;
    let width = term_width();
    let no_open = cli.no_open;
    match (cli.command, cli.repo) {
        (Some(Command::Paths), _) => {
            paths(&layout, out)?;
            Ok(ExitCode::SUCCESS)
        }
        (Some(Command::Clear { yes }), _) => clear(&layout, yes, out),
        (Some(Command::History), _) => {
            banner(out, width)?;
            history(&layout, no_open, width, out)
        }
        (None, Some(arg)) => {
            banner(out, width)?;
            act(&layout, target::parse_input(&arg)?, no_open, width, out)
        }
        (None, None) => interactive(&layout, no_open, width, out),
    }
}

fn interactive(
    layout: &Layout,
    no_open: bool,
    width: u16,
    out: &mut dyn Write,
) -> Result<ExitCode, Error> {
    banner(out, width)?;
    let line = prompt(
        out,
        "Enter url (repository on GitHub) or 'history / clear': ",
    )?;
    if line.trim().is_empty() {
        output(writeln!(out, "'enter' -> exit"))?;
        return Ok(ExitCode::SUCCESS);
    }
    act(layout, target::parse_input(&line)?, no_open, width, out)
}

fn act(
    layout: &Layout,
    input: Input,
    no_open: bool,
    width: u16,
    out: &mut dyn Write,
) -> Result<ExitCode, Error> {
    match input {
        Input::Repo(id) => scan_repo(layout, &id, no_open, width, out),
        Input::History => history(layout, no_open, width, out),
        Input::Clear => clear(layout, false, out),
        Input::Exit => Ok(ExitCode::SUCCESS),
    }
}

fn paths(layout: &Layout, out: &mut dyn Write) -> Result<(), Error> {
    output(writeln!(out, "{}", layout.data.display()))?;
    output(writeln!(out, "{}", layout.config_file.display()))
}

fn clear(layout: &Layout, yes: bool, out: &mut dyn Write) -> Result<ExitCode, Error> {
    if !yes {
        output(writeln!(out, "{}", layout.data.join("repos").display()))?;
        output(writeln!(out, "{}", layout.data.join("assets").display()))?;
        let answer = prompt(out, "[y/N]: ")?;
        if !matches!(answer.trim(), "y" | "Y") {
            output(writeln!(out, "cancel"))?;
            return Ok(ExitCode::SUCCESS);
        }
    }
    let freed = store::clear_data(layout)?;
    output(writeln!(
        out,
        "size of deleted data: {:.1} MB",
        freed as f64 / (1024.0 * 1024.0)
    ))?;
    Ok(ExitCode::SUCCESS)
}

fn history(
    layout: &Layout,
    no_open: bool,
    width: u16,
    out: &mut dyn Write,
) -> Result<ExitCode, Error> {
    let mut states = store::list_states(layout)?;
    if states.is_empty() {
        output(writeln!(out, "History is empty."))?;
        return Ok(ExitCode::SUCCESS);
    }
    states.sort_by_key(|state| std::cmp::Reverse(state.scanned_at));
    output(term::history_table(out, &states, &Zone::Local))?;
    let answer = prompt(out, "Select url (digit): ")?;
    let chosen = answer
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|n| (1..=states.len()).contains(n));
    let Some(number) = chosen else {
        output(writeln!(out, "not chosen"))?;
        return Ok(ExitCode::SUCCESS);
    };
    let id = target::parse_repo(&states[number - 1].repo)?;
    scan_repo(layout, &id, no_open, width, out)
}

fn scan_repo(
    layout: &Layout,
    id: &RepoId,
    no_open: bool,
    width: u16,
    out: &mut dyn Write,
) -> Result<ExitCode, Error> {
    let ctx = context(layout.clone(), !no_open, width)?;
    match app::scan(&ctx, id, out) {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(Error::Api(err)) => {
            output(term::api_error(
                out,
                &err,
                ctx.client.has_token(),
                &ctx.layout.config_file,
                &ctx.zone,
            ))?;
            Ok(ExitCode::from(1))
        }
        Err(err) => Err(err),
    }
}

fn context(layout: Layout, open_report: bool, term_width: u16) -> Result<Ctx, Error> {
    let config = store::config::load_or_init(&layout.config_file)?;
    Ok(Ctx {
        layout,
        client: Client::new("https://api.github.com", config.token, ""),
        zone: Zone::Local,
        tz_label: platform::local_tz_label(),
        now: Utc::now(),
        open_report,
        term_width,
    })
}

fn banner(out: &mut dyn Write, width: u16) -> Result<(), Error> {
    output(term::banner(out, width, env!("CARGO_PKG_VERSION")))
}

fn prompt(out: &mut dyn Write, text: &str) -> Result<String, Error> {
    output(write!(out, "{text}").and_then(|()| out.flush()))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|source| Error::Io {
            path: PathBuf::from("stdin"),
            source,
        })?;
    Ok(line)
}

fn term_width() -> u16 {
    console::Term::stdout()
        .size_checked()
        .map(|(_, cols)| cols)
        .unwrap_or(80)
}
