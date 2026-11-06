//! Take snapshots periodically through the user's crontab.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::cli::{Every, ScheduleArgs, Weekday};
use crate::commands::terminal_renderer;
use crate::config::ConfigRepository;
use crate::error::{Result, usage};
use crate::repository::shell_quote;
use crate::storage::ProfilePaths;
use crate::sys;

/// Print, install, or remove the crontab entry for a profile's snapshots.
pub fn schedule(args: &ScheduleArgs, config: &ConfigRepository) -> Result<i32> {
    let mut renderer = terminal_renderer(&args.terminal);
    let profile = &args.profile.profile;
    if args.weekday.is_some() && !matches!(args.every, Every::Weekly) {
        return Err(usage("--weekday applies only to --every weekly"));
    }
    config.load_profile(profile)?;
    let marker = format!("# qdu:{profile}");
    if args.remove {
        renderer.message(&if remove_entry(profile)? {
            format!("Removed the scheduled snapshot for {profile}.")
        } else {
            format!("No scheduled snapshot for profile {profile}.")
        });
        return Ok(0);
    }
    if args.executable.is_none() && running_executable().is_ok_and(|path| in_uv_cache(&path)) {
        renderer.warning(
            "this qdu runs from uv's cache (uvx), which uv may clean up; \
             install it with 'uv tool install qdu' or pass --qdu PATH",
        );
    }
    let paths = ProfilePaths::for_profile(profile)?;
    let entry = format!(
        "{} {} {marker}",
        cron_time(args),
        job(args, &paths.root.join("snapshot.log"))?
    );
    if !args.install {
        renderer.message("Add this line with 'crontab -e', or rerun with --install:");
        renderer.message("");
        renderer.message(&entry);
        return Ok(0);
    }
    paths.ensure()?;
    let mut lines: Vec<Vec<u8>> = read_crontab()?
        .into_iter()
        .filter(|line| !is_entry_of(line, &marker))
        .collect();
    lines.push(entry.clone().into_bytes());
    write_crontab(&lines)?;
    renderer.message(&format!("Installed in your crontab:\n{entry}"));
    Ok(0)
}

/// Remove the profile's line from the crontab; false if it had none.
pub fn remove_entry(profile: &str) -> Result<bool> {
    let marker = format!("# qdu:{profile}");
    let lines = read_crontab()?;
    let kept: Vec<Vec<u8>> = lines
        .iter()
        .filter(|line| !is_entry_of(line, &marker))
        .cloned()
        .collect();
    if kept.len() == lines.len() {
        return Ok(false);
    }
    write_crontab(&kept)?;
    Ok(true)
}

/// The crontab line that takes the profile's snapshots, if any.
pub fn scheduled_entry(profile: &str) -> Result<Option<String>> {
    let marker = format!("# qdu:{profile}");
    Ok(read_crontab()?
        .into_iter()
        .find(|line| is_entry_of(line, &marker))
        .map(|line| String::from_utf8_lossy(&line).into_owned()))
}

/// Whether a crontab line ends with a profile's marker.
fn is_entry_of(line: &[u8], marker: &str) -> bool {
    line.trim_ascii_end().ends_with(marker.as_bytes())
}

fn cron_time(args: &ScheduleArgs) -> String {
    let (hour, minute) = args.at;
    match args.every {
        Every::Daily => format!("{minute} {hour} * * *"),
        Every::Monthly => format!("{minute} {hour} 1 * *"),
        Every::Weekly => {
            let day = match args.weekday.unwrap_or(Weekday::Mon) {
                Weekday::Sun => 0,
                Weekday::Mon => 1,
                Weekday::Tue => 2,
                Weekday::Wed => 3,
                Weekday::Thu => 4,
                Weekday::Fri => 5,
                Weekday::Sat => 6,
            };
            format!("{minute} {hour} * * {day}")
        }
    }
}

fn job(args: &ScheduleArgs, log: &Path) -> Result<String> {
    // cron starts jobs with a minimal environment, so carry over the XDG
    // locations that decide where qdu reads its configuration and state.
    let mut words: Vec<String> = ["XDG_CONFIG_HOME", "XDG_STATE_HOME"]
        .iter()
        .filter_map(|name| match std::env::var(name) {
            Ok(value) if !value.is_empty() => Some(format!("{name}={}", shell_quote(&value))),
            _ => None,
        })
        .collect();
    let executable = match &args.executable {
        Some(path) => path.clone(),
        None => running_executable()?
            .into_os_string()
            .into_string()
            .map_err(|_| usage("the qdu executable path is not valid UTF-8; pass --qdu PATH"))?,
    };
    for part in [
        executable.as_str(),
        "snapshot",
        "--profile",
        &args.profile.profile,
        "--quiet",
    ] {
        words.push(shell_quote(part));
    }
    words.push(format!(">>{}", shell_quote(&log.display().to_string())));
    words.push("2>&1".into());
    // cron turns an unescaped % into a newline.
    Ok(words.join(" ").replace('%', "\\%"))
}

/// The path of the running qdu as the shell invoked it.
///
/// `uv tool install` and pipx put a symbolic link such as
/// `~/.local/bin/qdu` on PATH and replace its target on upgrades, so the
/// entry keeps that link instead of the file it currently points to.
fn running_executable() -> Result<PathBuf> {
    let program = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| {
            usage(format!(
                "cannot tell where qdu is installed ({error}); pass --qdu PATH"
            ))
        })?;
    let invoked = std::env::args_os().next().map(PathBuf::from);
    let candidate = invoked.and_then(|invoked| {
        if invoked.components().count() > 1 {
            std::env::current_dir()
                .ok()
                .map(|directory| directory.join(invoked))
        } else {
            sys::find_executable(invoked.to_str()?)
        }
    });
    Ok(candidate
        .filter(|candidate| candidate.canonicalize().ok().as_ref() == Some(&program))
        .unwrap_or(program))
}

/// Whether a path lies in uv's cache, where `uvx` runs tools from.
fn in_uv_cache(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|name| name.starts_with("archive-v"))
    })
}

fn crontab() -> Result<std::path::PathBuf> {
    sys::find_executable("crontab").ok_or_else(|| {
        usage("crontab is not available; see docs/QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE.md")
    })
}

fn read_crontab() -> Result<Vec<Vec<u8>>> {
    let output = Command::new(crontab()?)
        .arg("-l")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| usage(format!("cannot read your crontab: {error}")))?;
    if output.status.success() {
        let mut lines: Vec<Vec<u8>> = output
            .stdout
            .split(|byte| *byte == b'\n')
            .map(<[u8]>::to_vec)
            .collect();
        // The final newline leaves an empty piece behind.
        if lines.last().is_some_and(Vec::is_empty) {
            lines.pop();
        }
        return Ok(lines);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.to_lowercase().contains("no crontab") {
        return Ok(Vec::new());
    }
    Err(usage(format!(
        "cannot read your crontab: {}",
        stderr.trim()
    )))
}

/// Replace the crontab. Lines are bytes, so that other entries survive as
/// they were even when they are not UTF-8.
fn write_crontab(lines: &[Vec<u8>]) -> Result<()> {
    let mut content = Vec::new();
    for line in lines {
        content.extend_from_slice(line);
        content.push(b'\n');
    }
    let mut child = Command::new(crontab()?)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| usage(format!("cannot update your crontab: {error}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&content)
            .map_err(|error| usage(format!("cannot update your crontab: {error}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| usage(format!("cannot update your crontab: {error}")))?;
    if !output.status.success() {
        return Err(usage(format!(
            "cannot update your crontab: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::repository::shell_quote;

    #[test]
    fn quoting_matches_python_shlex() {
        assert_eq!(shell_quote("/usr/bin/qdu"), "/usr/bin/qdu");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }
}
