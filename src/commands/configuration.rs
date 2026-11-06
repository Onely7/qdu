//! Manage profiles and show the resolved configuration.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::cli::{
    ConfigAction, ConfigArgs, OutputOptions, ProfileAction, ProfileArgs, ProfileSetArgs,
};
use crate::commands::{
    effective_profile, known_profiles, renderer, require_known_profile, schedule, terminal_renderer,
};
use crate::config::{
    ConfigRepository, Initialized, ProfileConfig, default_profile_name, render_compact_after,
};
use crate::error::{Error, Result, exit, usage};
use crate::locking::ProfileLock;
use crate::patterns::PathMatcher;
use crate::render::{Align, KeyValue, Renderer};
use crate::report::{Column, Field, Report, Table};
use crate::repository::canonical_root;
use crate::storage::{IndexRepository, ProfilePaths};
use crate::sys;
use crate::units::{format_age, format_exact_size, format_local_time, group};

/// List, show, add, or remove a profile.
pub fn profile(args: &ProfileArgs, config: &ConfigRepository) -> Result<i32> {
    match &args.action {
        ProfileAction::List { output } => list_profiles(&mut renderer(output), config),
        ProfileAction::Show { name, output } => {
            let name = match name {
                Some(name) => name.clone(),
                None => default_profile_name(),
            };
            require_known_profile(config, &name)?;
            show_profile(output, &effective_profile(config, &name)?);
            Ok(0)
        }
        ProfileAction::Set(settings) => set_profile(settings, config),
        ProfileAction::Remove {
            name,
            delete_data,
            terminal,
        } => {
            let mut renderer = terminal_renderer(terminal);
            let paths = ProfilePaths::for_profile(name)?;
            // Refuse while a snapshot or compaction is using the profile,
            // before anything is changed.
            let lock = if *delete_data && paths.root.exists() {
                Some(ProfileLock::acquire(
                    &paths.lock,
                    &format!("profile remove {name}"),
                )?)
            } else {
                None
            };
            let removed = config.remove_profile(name)?;
            let deleted = lock.is_some();
            if deleted {
                fs::remove_dir_all(&paths.root)?;
            }
            drop(lock);
            // Without the profile, its scheduled snapshots would only fail.
            // A machine without crontab has no entry to remove.
            let unscheduled = matches!(schedule::scheduled_entry(name), Ok(Some(_)))
                && schedule::remove_entry(name)?;
            renderer.message(&match (removed, deleted) {
                (true, true) => format!("Profile removed: {name}, with its snapshots"),
                (true, false) if paths.root.exists() => format!(
                    "Profile removed: {name}; its snapshots remain in {}",
                    paths.root.display()
                ),
                (true, false) => format!("Profile removed: {name}"),
                (false, true) => {
                    format!("Profile was not configured: {name}; its snapshots were deleted")
                }
                (false, false) => format!("Profile was not configured: {name}"),
            });
            if unscheduled {
                renderer.message("Removed its scheduled snapshot from your crontab.");
            }
            Ok(0)
        }
    }
}

fn set_profile(args: &ProfileSetArgs, config: &ConfigRepository) -> Result<i32> {
    let mut renderer = terminal_renderer(&args.terminal);
    // The snapshots fix the directory: later ones must scan the same one.
    let recorded = IndexRepository::new(ProfilePaths::for_profile(&args.name)?)
        .load()
        .ok()
        .and_then(|index| index.recorded_root().map(str::to_string));
    match (&args.root, &recorded) {
        (None, None) if !config.configures_path(&args.name)? => {
            return Err(usage(format!(
                "profile '{}' has no directory yet; give it with --path DIR",
                args.name
            )));
        }
        (Some(root), Some(recorded)) if canonical_root(root) != Path::new(recorded) => {
            return Err(usage(format!(
                "profile '{name}' holds snapshots of {recorded}; use another profile for {}, \
                 or delete them first with 'qdu profile remove {name} --delete-data'",
                root.display(),
                name = args.name,
            )));
        }
        _ => {}
    }
    let mut overrides = args.collection.overrides();
    overrides.root = args.root.clone();
    if !args.exclude.is_empty() {
        // Check the patterns before they are stored.
        PathMatcher::new(&args.exclude)?;
        overrides.excludes = Some(args.exclude.clone());
    }
    let capacity = args.capacity.overrides();
    overrides.capacity_limit_bytes = capacity.limit_bytes;
    overrides.capacity_user = capacity.user;
    if overrides.capacity_limit_bytes == Some(None) && overrides.capacity_user.is_none() {
        // Removing the limit also removes the user it applied to.
        overrides.capacity_user = Some(None);
    }
    config.update_profile(&args.name, &overrides.render())?;
    renderer.message(&format!("Profile saved: {}", args.name));
    Ok(0)
}

/// Print the configuration path, create it, or show a profile's settings.
pub fn config(args: &ConfigArgs, config: &ConfigRepository) -> Result<i32> {
    match &args.action {
        ConfigAction::Path { terminal } => {
            terminal_renderer(terminal).message(&config.path.display().to_string());
        }
        ConfigAction::Init { terminal } => {
            let path = config.path.display();
            terminal_renderer(terminal).message(&match config.initialize()? {
                Initialized::Created => format!("Created {path}"),
                Initialized::AddedDefaults => format!(
                    "Added a [defaults] section to {path}; its exclusions apply to every \
                     profile that sets none of its own"
                ),
                Initialized::Unchanged => format!("Already has [defaults]: {path}"),
            });
        }
    }
    Ok(0)
}

fn list_profiles(renderer: &mut Renderer, config: &ConfigRepository) -> Result<i32> {
    let mut problems = Vec::new();
    let now = sys::now_epoch();
    let mut report = Report::new("profile list", None);
    let mut table = Table::new(vec![
        Column::new("profile", "Profile", Align::Left),
        Column::new("snapshots", "Snapshots", Align::Right),
        Column::data("latest_snapshot_id"),
        Column::new("latest_created_at", "Latest", Align::Left),
        Column::data("latest_complete"),
        Column::data("capacity_limit_bytes"),
        Column::data("capacity_user"),
        Column::table("Capacity", Align::Left),
        Column::path("root", "Root"),
    ]);
    for name in known_profiles(config)? {
        let index = IndexRepository::new(ProfilePaths::for_profile(&name)?).load();
        let snapshots = match index {
            Ok(index) => Some(index.snapshots),
            Err(error @ Error::Format(_)) => {
                problems.push(format!("{error}; run 'qdu repair --profile {name}'"));
                None
            }
            Err(error) => return Err(error),
        };
        let newest = snapshots.as_ref().and_then(|snapshots| snapshots.last());
        let count = match &snapshots {
            Some(snapshots) => Field::new(snapshots.len(), group(snapshots.len() as i128)),
            None => Field::new(Value::Null, "?"),
        };
        let latest = match (newest, &snapshots) {
            (Some(newest), _) => {
                let mut text = format!(
                    "{} ({} ago)",
                    format_local_time(newest.created_epoch as f64),
                    format_age(now - newest.created_epoch)
                );
                if !newest.complete {
                    text.push_str(", incomplete");
                }
                Field::new(newest.created_at.clone(), text)
            }
            (None, Some(_)) => Field::new(Value::Null, "none"),
            (None, None) => Field::new(Value::Null, "(invalid index)"),
        };
        let mut row = vec![
            Field::text(name.clone()),
            count,
            Field::data(newest.map(|record| record.snapshot_id.clone())),
            latest,
            Field::data(newest.map(|record| record.complete)),
        ];
        match effective_profile(config, &name) {
            Ok(profile) => row.extend([
                Field::data(profile.capacity_limit_bytes),
                Field::data(profile.capacity_user.clone()),
                Field::shown(capacity_text(&profile)),
                Field::text(profile.root.display().to_string()),
            ]),
            Err(error @ Error::Usage(_)) => {
                // One broken section should not hide the other profiles.
                problems.push(error.to_string());
                row.extend([
                    Field::data(Value::Null),
                    Field::data(Value::Null),
                    Field::shown(""),
                    Field::new(Value::Null, "(invalid configuration)"),
                ]);
            }
            Err(error) => return Err(error),
        }
        table.push(row);
    }
    report.rows(table);
    report.render(renderer);
    for message in &problems {
        renderer.warning(message);
    }
    Ok(if problems.is_empty() {
        0
    } else {
        exit::INVALID_PROFILE
    })
}

fn capacity_text(profile: &ProfileConfig) -> String {
    match profile.capacity_limit_bytes {
        None => "not set".into(),
        Some(limit) => format!(
            "{} ({})",
            format_exact_size(limit),
            profile.capacity_user.as_deref().unwrap_or("profile root")
        ),
    }
}

/// Each setting as configuration text; exclusions stay one per item.
fn profile_settings(profile: &ProfileConfig) -> Vec<(&'static str, Vec<String>)> {
    let one = |value: String| vec![value];
    vec![
        ("profile", one(profile.name.clone())),
        ("path", one(profile.root.display().to_string())),
        ("exclude", profile.excludes.clone()),
        ("keep_snapshots", one(profile.keep_snapshots.to_string())),
        (
            "compact_after",
            one(render_compact_after(profile.compact_after_seconds)),
        ),
        ("collect_users", one(profile.collect_users.to_string())),
        ("user_max_depth", one(profile.user_max_depth.to_string())),
        (
            "large_file_limit",
            one(profile.large_file_limit.to_string()),
        ),
        (
            "record_max_depth",
            one(profile
                .record_max_depth
                .map(|depth| depth.to_string())
                .unwrap_or_default()),
        ),
        (
            "cross_filesystems",
            one(profile.cross_filesystems.to_string()),
        ),
        (
            "capacity_limit",
            one(profile
                .capacity_limit_bytes
                .map(format_exact_size)
                .unwrap_or_default()),
        ),
        (
            "capacity_user",
            one(profile.capacity_user.clone().unwrap_or_default()),
        ),
    ]
}

fn show_profile(output: &OutputOptions, profile: &ProfileConfig) {
    let mut renderer = renderer(output);
    let mut report = Report::new("profile show", Some(&profile.name));
    report.context("settings", profile_json(profile));
    // One row per exclusion pattern; an unset value is empty.
    let mut table = Table::new(vec![Column::data("setting"), Column::data("value")]);
    for (key, values) in profile_settings(profile) {
        let unset = match key {
            "record_max_depth" => "unlimited",
            "exclude" => "none",
            _ => "not set",
        };
        let shown = if values.iter().all(String::is_empty) {
            KeyValue::One(unset.to_string())
        } else if key == "exclude" {
            KeyValue::Many(values.clone())
        } else {
            KeyValue::One(values.concat())
        };
        report.summary(key, shown);
        for value in values {
            table.push(vec![Field::data(key), Field::data(value)]);
        }
    }
    report.rows_for_tsv(table);
    report.render(&mut renderer);
}

fn profile_json(profile: &ProfileConfig) -> Value {
    json!({
        "name": profile.name,
        "path": profile.root.display().to_string(),
        "exclude": profile.excludes,
        "keep_snapshots": profile.keep_snapshots,
        "compact_after": render_compact_after(profile.compact_after_seconds),
        "collect_users": profile.collect_users,
        "user_max_depth": profile.user_max_depth,
        "large_file_limit": profile.large_file_limit,
        "record_max_depth": profile.record_max_depth,
        "cross_filesystems": profile.cross_filesystems,
        "capacity_limit": profile.capacity_limit_bytes.map(format_exact_size),
        "capacity_limit_bytes": profile.capacity_limit_bytes,
        "capacity_user": profile.capacity_user,
    })
}
