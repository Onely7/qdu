//! qdu: record disk-usage snapshots and explore where space goes.

#[cfg(not(unix))]
compile_error!("qdu reads directories through Unix system calls and builds on Unix only");

mod capacity;
mod cli;
mod commands;
mod config;
mod error;
mod filesystem;
mod live;
mod locking;
mod model;
mod patterns;
mod query;
mod render;
mod report;
mod repository;
mod scan;
mod selector;
mod signals;
mod staleness;
mod storage;
mod sys;
mod traverse;
mod units;

use std::ffi::OsString;
use std::io::Write;

use clap::Parser;

use crate::cli::{Cli, Command};
use crate::commands::{
    browse, check, configuration, diff, doctor, errors, files, forget, history, label, list,
    maintenance, schedule, show, snapshot, users,
};
use crate::config::ConfigRepository;

/// The profile a command works on, unless the command may create it or
/// manages profiles itself.
fn existing_profile(command: &Command) -> Option<&str> {
    let profile = match command {
        Command::Snapshot(args) if args.root.is_some() => return None,
        Command::Profile(_) | Command::Config(_) | Command::Completions(_) => return None,
        Command::Snapshot(args) => &args.profile,
        Command::Show(args) => &args.profile,
        Command::Diff(args) => &args.profile,
        Command::History(args) => &args.profile,
        Command::List(args) => &args.profile,
        Command::Errors(args) => &args.profile,
        Command::Users(args) => &args.profile,
        Command::Files(args) => &args.profile,
        Command::Browse(args) => &args.profile,
        Command::Check(args) => &args.profile,
        Command::Compact(args) => &args.profile,
        Command::Verify(args) => &args.profile,
        Command::Doctor(args) => &args.profile,
        Command::Repair(args) => &args.profile,
        Command::Schedule(args) => &args.profile,
        Command::Label(args) => &args.profile,
        Command::Forget(args) => &args.profile,
    };
    Some(&profile.profile)
}

fn run(cli: &Cli) -> error::Result<i32> {
    let config = ConfigRepository::new();
    if let Some(profile) = existing_profile(&cli.command) {
        // doctor reports a configuration it cannot read instead of stopping.
        let doctor = matches!(cli.command, Command::Doctor(_));
        if !doctor || config.has_profile(profile).is_ok() {
            commands::require_known_profile(&config, profile)?;
        }
    }
    match &cli.command {
        Command::Snapshot(args) => snapshot::snapshot(args, &config),
        Command::Show(args) => show::show(args, &config),
        Command::Diff(args) => diff::diff(args),
        Command::History(args) => history::history(args, &config),
        Command::List(args) => list::list(args),
        Command::Errors(args) => errors::errors(args),
        Command::Users(args) => users::users(args),
        Command::Files(args) => files::files(args, &config),
        Command::Browse(args) => browse::browse(args, &config),
        Command::Check(args) => check::check(args, &config),
        Command::Compact(args) => maintenance::compact(args),
        Command::Verify(args) => maintenance::verify(args),
        Command::Doctor(args) => doctor::doctor(args, &config),
        Command::Repair(args) => maintenance::repair(args),
        Command::Schedule(args) => schedule::schedule(args, &config),
        Command::Label(args) => label::label(args),
        Command::Forget(args) => forget::forget(args),
        Command::Profile(args) => configuration::profile(args, &config),
        Command::Config(args) => configuration::config(args, &config),
        Command::Completions(args) => {
            let mut command = <Cli as clap::CommandFactory>::command();
            clap_complete::generate(args.shell, &mut command, "qdu", &mut std::io::stdout());
            Ok(0)
        }
    }
}

/// The arguments with `show` inserted when they name no command, as in
/// `qdu` or `qdu -L 2`; help and version requests stay as they are.
fn with_default_command(mut arguments: Vec<OsString>) -> Vec<OsString> {
    let insert = match arguments.get(1).map(|argument| argument.to_string_lossy()) {
        None => true,
        Some(first) => {
            first.starts_with('-') && !matches!(&*first, "-h" | "--help" | "-V" | "--version")
        }
    };
    if insert {
        arguments.insert(1, "show".into());
    }
    arguments
}

fn main() {
    signals::install();
    let arguments = with_default_command(std::env::args_os().collect());
    let cli = Cli::parse_from(arguments);
    let code = match run(&cli) {
        Ok(code) => code,
        Err(error) => {
            let message = render::display_safe(&error.to_string());
            let _ = writeln!(std::io::stderr(), "qdu: {message}");
            error.exit_code()
        }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_without_a_command_run_show() {
        let run = |arguments: &[&str]| -> Vec<String> {
            let arguments = arguments.iter().map(OsString::from).collect();
            with_default_command(arguments)
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(run(&["qdu"]), ["qdu", "show"]);
        assert_eq!(run(&["qdu", "-L", "2"]), ["qdu", "show", "-L", "2"]);
        assert_eq!(
            run(&["qdu", "--profile", "x"]),
            ["qdu", "show", "--profile", "x"]
        );
        assert_eq!(run(&["qdu", "--help"]), ["qdu", "--help"]);
        assert_eq!(run(&["qdu", "-V"]), ["qdu", "-V"]);
        assert_eq!(run(&["qdu", "list"]), ["qdu", "list"]);
    }
}
