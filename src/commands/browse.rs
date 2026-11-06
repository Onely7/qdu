//! Pick a directory with fzf, then show it.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use crate::cli::{BrowseArgs, Capacity, Ranking, ShowArgs};
use crate::commands::show;
use crate::config::ConfigRepository;
use crate::error::{Result, usage};
use crate::query::{self, QueryService};
use crate::sys;
use crate::units::format_bytes;

/// Pick a directory with fzf and show it.
pub fn browse(args: &BrowseArgs, config: &ConfigRepository) -> Result<i32> {
    let fzf = sys::find_executable("fzf")
        .ok_or_else(|| usage("fzf is not installed; use 'qdu show PATH' instead"))?;
    let service = QueryService::new(&args.profile.profile)?;
    let record = service.resolve(&args.selector.snapshot)?;
    let context = service.open(&record)?;
    // The executable is resolved to an absolute path and no shell is involved.
    // Each record is "SIZE<TAB>PATH", NUL-terminated so that a name containing
    // a newline or surrounding spaces survives the round trip. fzf shows both
    // fields but matches the query against the path only.
    let mut child = Command::new(fzf)
        .args([
            "--prompt",
            "qdu> ",
            "--height",
            "80%",
            "--reverse",
            "--read0",
            "--print0",
            "--delimiter",
            "\t",
            "--nth",
            "2..",
            "--tabstop",
            "1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| usage(format!("cannot start fzf: {error}")))?;
    if let Some(mut input) = child.stdin.take() {
        query::visit_directories_by_size(&context.connection, |path, allocated| {
            let line = format!("{:>8}\t{path}\0", format_bytes(allocated, false));
            input.write_all(line.as_bytes()).is_ok()
        })?;
    }
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut output);
    }
    let status = child.wait()?;
    // fzf exits with 1 when nothing matched and 130 when it was cancelled;
    // any other failure is an error of its own, which it has explained.
    match status.code() {
        Some(0 | 1 | 130) => {}
        Some(code) => return Err(usage(format!("fzf failed with exit status {code}"))),
        None => return Err(usage("fzf was stopped by a signal")),
    }
    let selected = output.split('\0').next().unwrap_or_default();
    if !status.success() || selected.is_empty() {
        return Ok(0);
    }
    let selected = selected
        .split_once('\t')
        .map_or(selected, |(_, path)| path)
        .to_string();
    drop(context);
    let ranking = Ranking {
        max_depth: args.max_depth,
        top: args.top,
        pattern: args.pattern.clone(),
    };
    let show = ShowArgs {
        path: Some(selected),
        profile: args.profile.clone(),
        selector: args.selector.clone(),
        ranking,
        min_size: args.min_size,
        metric: args.metric,
        owners: false,
        staleness: args.staleness.clone(),
        // The profile's own capacity policy applies.
        capacity: Capacity::default(),
        output: args.output.clone(),
    };
    show::show(&show, config)
}
