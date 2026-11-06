//! qdu's command-line grammar.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::capacity::CapacityOverrides;
use crate::config::{ProfileOverrides, parse_compact_after, validate_profile_name};
use crate::model::Metric;
use crate::query::DiffOrder;
use crate::render::{ColorChoice, Format, Style};
use crate::selector::{Selector, validate_label};
use crate::staleness::StaleThresholds;
use crate::sys;
use crate::units::{parse_duration, parse_size};

#[derive(Parser, Debug)]
#[command(
    name = "qdu",
    version,
    about = "Record disk-usage snapshots and explore where space goes. \
             Without a command, qdu runs 'qdu show' with the options given.",
    after_help = "Examples:
  qdu snapshot --path ~/data   Record the first snapshot of a directory
  qdu                          Rank its largest directories
  qdu -L 2                     Rank two levels of directories
  qdu diff                     Rank what changed since the previous snapshot
  qdu schedule --install       Take a snapshot every week with cron

Run 'qdu COMMAND --help' for the options of a command."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scan the profile root and save a snapshot
    Snapshot(SnapshotArgs),
    /// Rank the largest directories in a snapshot
    Show(ShowArgs),
    /// Rank changes between two snapshots
    Diff(DiffArgs),
    /// Show how a directory grew over the snapshots, and when it will fill
    History(HistoryArgs),
    /// List snapshots and their disk use
    List(ListArgs),
    /// Rank file owners (needs --with-users)
    Users(UsersArgs),
    /// List paths a snapshot could not read
    Errors(ErrorsArgs),
    /// Rank the largest files
    Files(FilesArgs),
    /// Pick a directory with fzf, then show it like 'show'
    Browse(BrowseArgs),
    /// Evaluate alert thresholds; exit status 10 on an alert
    Check(CheckArgs),
    /// Take snapshots periodically with your crontab
    Schedule(ScheduleArgs),
    /// Gzip older snapshots now
    Compact(CompactArgs),
    /// Check snapshot checksums and database integrity
    Verify(VerifyArgs),
    /// Diagnose the qdu setup of a profile
    Doctor(DoctorArgs),
    /// Rebuild the snapshot index from the snapshot files
    Repair(RepairArgs),
    /// Label a snapshot, so that @NAME selects it and retention keeps it
    Label(LabelArgs),
    /// Remove snapshots from a profile
    Forget(ForgetArgs),
    /// Manage scan profiles
    Profile(ProfileArgs),
    /// Print the configuration path, or create a starter file
    Config(ConfigArgs),
    /// Print a completion script for your shell
    #[command(after_help = "Examples:
  qdu completions bash > ~/.local/share/bash-completion/completions/qdu
  qdu completions zsh > ~/.zfunc/_qdu      (with ~/.zfunc in fpath)
  qdu completions fish > ~/.config/fish/completions/qdu.fish")]
    Completions(CompletionsArgs),
}

// ---------------------------------------------------------------------------
// Value parsers

fn positive(value: &str) -> Result<usize, String> {
    match value.trim().parse::<usize>() {
        Ok(number) if number > 0 => Ok(number),
        _ => Err("must be a positive integer".into()),
    }
}

fn non_negative(value: &str) -> Result<u64, String> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| "must be a non-negative integer".into())
}

fn depth(value: &str) -> Result<u32, String> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| "must be a non-negative integer".into())
}

fn percentage(value: &str) -> Result<f64, String> {
    match value.trim().parse::<f64>() {
        Ok(number) if (0.0..=100.0).contains(&number) => Ok(number),
        _ => Err("must be between 0 and 100".into()),
    }
}

fn capacity_size(value: &str) -> Result<u64, String> {
    match parse_size(value) {
        Ok(0) => Err("must be greater than zero".into()),
        Ok(size) => Ok(size),
        Err(error) => Err(error.to_string()),
    }
}

fn size(value: &str) -> Result<u64, String> {
    parse_size(value).map_err(|error| error.to_string())
}

/// A compaction age; `None` means `off`.
#[derive(Clone, Copy, Debug)]
pub struct CompactAfter(pub Option<u64>);

fn compact_after(value: &str) -> Result<CompactAfter, String> {
    parse_compact_after(value)
        .map(CompactAfter)
        .map_err(|error| error.to_string())
}

fn duration(value: &str) -> Result<u64, String> {
    parse_duration(value).map_err(|error| error.to_string())
}

/// A number of days, written as such or as a duration of whole days.
fn days(value: &str) -> Result<u64, String> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value
            .parse()
            .map_err(|_| "is too large a number of days".to_string());
    }
    let seconds = duration(value)?;
    if seconds % 86_400 == 0 {
        Ok(seconds / 86_400)
    } else {
        Err("use whole days, such as 90 or 90d".into())
    }
}

/// A valid profile name; an empty one, as from `QDU_PROFILE=`, is the
/// default profile.
fn profile_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok("default".into());
    }
    validate_profile_name(value)
        .map(|()| value.to_string())
        .map_err(|error| error.to_string())
}

fn label_name(value: &str) -> Result<String, String> {
    validate_label(value).map(|()| value.to_string())
}

fn stale_thresholds(value: &str) -> Result<StaleThresholds, String> {
    StaleThresholds::parse(value)
}

fn expanded_path(value: &str) -> Result<PathBuf, String> {
    Ok(sys::expand_user(value))
}

/// A 24-hour `HH:MM` time of day.
pub fn clock_time(value: &str) -> Result<(u32, u32), String> {
    let invalid = || "use a 24-hour time such as 03:15".to_string();
    let (hour, minute) = value.split_once(':').ok_or_else(invalid)?;
    let digits = |text: &str| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit());
    if !digits(hour) || !digits(minute) {
        return Err(invalid());
    }
    let (hour, minute): (u32, u32) = (
        hour.parse().map_err(|_| invalid())?,
        minute.parse().map_err(|_| invalid())?,
    );
    if hour < 24 && minute < 60 {
        Ok((hour, minute))
    } else {
        Err(invalid())
    }
}

fn metric_from(value: &str, allowed: &[Metric]) -> Result<Metric, String> {
    let metric = match value {
        "allocated" | "allocated_bytes" => Some(Metric::Allocated),
        "apparent" | "apparent_bytes" => Some(Metric::Apparent),
        "inodes" | "inode_count" => Some(Metric::Inodes),
        _ => None,
    };
    match metric {
        Some(metric) if allowed.contains(&metric) => Ok(metric),
        _ => {
            let names: Vec<&str> = allowed.iter().map(|metric| metric.label()).collect();
            Err(format!("choose from {}", names.join(", ")))
        }
    }
}

fn size_metric(value: &str) -> Result<Metric, String> {
    metric_from(value, &[Metric::Allocated, Metric::Apparent])
}

fn any_metric(value: &str) -> Result<Metric, String> {
    metric_from(
        value,
        &[Metric::Allocated, Metric::Apparent, Metric::Inodes],
    )
}

fn owner_metric(value: &str) -> Result<Metric, String> {
    metric_from(value, &[Metric::Allocated, Metric::Inodes])
}

// ---------------------------------------------------------------------------
// Shared option groups

#[derive(Args, Debug, Clone)]
pub struct ProfileOption {
    /// Profile to use (default: $QDU_PROFILE, or 'default')
    #[arg(
        long,
        value_name = "NAME",
        env = "QDU_PROFILE",
        default_value = "default",
        hide_env = true,
        hide_default_value = true,
        value_parser = profile_name
    )]
    pub profile: String,
}

#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Output")]
pub struct OutputOptions {
    /// Output format; TSV and JSON are stable for scripts
    #[arg(long, value_enum, default_value = "table")]
    pub format: Format,
    #[command(flatten)]
    pub terminal: TerminalOptions,
}

#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Output")]
pub struct TerminalOptions {
    /// Table drawing: rich box lines on a UTF-8 terminal, or plain
    #[arg(long, value_enum, default_value = "auto")]
    pub style: Style,
    /// When to color output; auto colors a terminal unless NO_COLOR is set
    #[arg(long, value_enum, default_value = "auto")]
    pub color: ColorChoice,
}

#[derive(Args, Debug, Clone)]
pub struct SnapshotSelector {
    /// latest (the newest complete snapshot), latest-any, previous,
    /// latest~N, @LABEL, an age such as 30d, a date YYYY-MM-DD, or a
    /// snapshot ID or unique ID prefix
    #[arg(long, value_name = "SELECTOR", default_value = "latest", value_parser = Selector::parse)]
    pub snapshot: Selector,
}

#[derive(Args, Debug, Clone)]
pub struct Ranking {
    /// Include directories up to N levels below PATH
    #[arg(short = 'L', long, value_name = "N", default_value = "1", value_parser = positive)]
    pub max_depth: usize,
    /// Show N rows
    #[arg(short = 'n', long, value_name = "N", default_value = "20", value_parser = positive)]
    pub top: usize,
    /// Keep paths containing TEXT (ignoring case) or matching a glob such as 'models/*'
    #[arg(long = "match", value_name = "TEXT")]
    pub pattern: Option<String>,
}

#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Staleness")]
pub struct Staleness {
    /// Show inactivity age and color stale entries
    #[arg(long)]
    pub stale: bool,
    /// Show only entries whose newest modification is at least DAYS old, as
    /// a number of days or a duration such as 12w
    #[arg(long, value_name = "DAYS", value_parser = days)]
    pub stale_only: Option<u64>,
    /// Warning thresholds in days
    #[arg(long, value_name = "D1,D2,D3,D4", default_value = "30,90,180,365", value_parser = stale_thresholds)]
    pub stale_thresholds: StaleThresholds,
}

impl Staleness {
    pub fn enabled(&self) -> bool {
        self.stale || self.stale_only.is_some()
    }
}

#[derive(Args, Debug, Clone, Default)]
#[command(next_help_heading = "Capacity limit")]
pub struct Capacity {
    /// Operational capacity limit, for example 2TiB
    #[arg(long, value_name = "SIZE", value_parser = capacity_size, conflicts_with = "no_capacity_limit")]
    pub capacity_limit: Option<u64>,
    /// Ignore or clear the configured operational capacity limit
    #[arg(long)]
    pub no_capacity_limit: bool,
    /// Compare the limit with one user's owned allocation
    #[arg(long, value_name = "USER", conflicts_with = "capacity_profile")]
    pub capacity_user: Option<String>,
    /// Compare the limit with the whole profile root
    #[arg(long)]
    pub capacity_profile: bool,
}

impl Capacity {
    pub fn overrides(&self) -> CapacityOverrides {
        CapacityOverrides {
            limit_bytes: if self.no_capacity_limit {
                Some(None)
            } else {
                self.capacity_limit.map(Some)
            },
            user: if self.capacity_profile {
                Some(None)
            } else {
                self.capacity_user.clone().map(Some)
            },
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct Comparison {
    /// Older snapshot, named as with show --snapshot (default: the complete
    /// snapshot before --to)
    #[arg(long = "from", value_name = "SELECTOR", value_parser = Selector::parse)]
    pub from_snapshot: Option<Selector>,
    /// Newer snapshot
    #[arg(long = "to", value_name = "SELECTOR", default_value = "latest", value_parser = Selector::parse)]
    pub to_snapshot: Selector,
    /// Only show what grew
    #[arg(long, conflicts_with = "shrink_only")]
    pub growth_only: bool,
    /// Only show what shrank
    #[arg(long)]
    pub shrink_only: bool,
    /// Rank by growth, shrinkage, size of the change, or current value
    /// (default: shrink with --shrink-only, otherwise growth)
    #[arg(long, value_enum)]
    pub order: Option<DiffOrder>,
}

/// Profile settings shared by `snapshot` and `profile set`.
#[derive(Args, Debug, Clone)]
#[command(next_help_heading = "Profile settings")]
pub struct Collection {
    /// Keep at most N unlabeled snapshots, removing the oldest; 0 keeps all
    /// (default: 52)
    #[arg(long, value_name = "N", value_parser = non_negative)]
    pub keep_snapshots: Option<u64>,
    /// After a snapshot, gzip snapshots older than DURATION except the two
    /// newest; 'off' disables it (default: 7d)
    #[arg(long, value_name = "DURATION", value_parser = compact_after)]
    pub compact_after: Option<CompactAfter>,
    /// Record usage per file owner, for 'qdu users'
    #[arg(long, conflicts_with = "without_users")]
    pub with_users: bool,
    /// Do not record usage per file owner (default)
    #[arg(long)]
    pub without_users: bool,
    /// Record owner totals for directories down to depth N (default: 3)
    #[arg(long, value_name = "N", value_parser = depth)]
    pub user_max_depth: Option<u32>,
    /// Remember the N largest files for 'qdu files' (default: 1000)
    #[arg(long = "file-top", value_name = "N", value_parser = non_negative)]
    pub large_file_limit: Option<u64>,
    /// Store directories down to depth N only; deeper files still count
    #[arg(long, value_name = "N", value_parser = depth)]
    pub record_max_depth: Option<u32>,
    /// Descend into directories mounted from other filesystems
    #[arg(long, conflicts_with = "one_file_system")]
    pub cross_filesystems: bool,
    /// Stay on the root's filesystem (default)
    #[arg(long)]
    pub one_file_system: bool,
}

impl Collection {
    pub fn overrides(&self) -> ProfileOverrides {
        ProfileOverrides {
            keep_snapshots: self.keep_snapshots,
            compact_after_seconds: self.compact_after.map(|value| value.0),
            collect_users: if self.with_users {
                Some(true)
            } else if self.without_users {
                Some(false)
            } else {
                None
            },
            user_max_depth: self.user_max_depth,
            large_file_limit: self.large_file_limit,
            record_max_depth: self.record_max_depth.map(Some),
            cross_filesystems: if self.cross_filesystems {
                Some(true)
            } else if self.one_file_system {
                Some(false)
            } else {
                None
            },
            ..ProfileOverrides::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Commands

#[derive(Args, Debug)]
pub struct SnapshotArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    /// Directory to scan (default: the profile's path, or your home directory)
    #[arg(short = 'p', long = "path", value_name = "ROOT", value_parser = expanded_path)]
    pub root: Option<PathBuf>,
    /// Also exclude PATTERN; repeatable
    #[arg(long, value_name = "PATTERN")]
    pub exclude: Vec<String>,
    /// Also exclude the patterns in FILE, one per line; repeatable
    #[arg(long, value_name = "FILE")]
    pub exclude_from: Vec<PathBuf>,
    /// Keep the directory, exclusions, and profile settings given here for
    /// later snapshots; the first snapshot of an unconfigured profile keeps
    /// them anyway
    #[arg(long)]
    pub save: bool,
    /// Label the snapshot, so that @NAME selects it and retention keeps
    /// it; repeatable
    #[arg(long, value_name = "NAME", value_parser = label_name)]
    pub label: Vec<String>,
    /// Scan with N threads (default: $QDU_THREADS, or the CPU count up to 16)
    #[arg(long, value_name = "N", value_parser = positive)]
    pub threads: Option<usize>,
    /// Print nothing on success, not even progress
    #[arg(long)]
    pub quiet: bool,
    // Arguments after a flattened group with a heading take that heading.
    #[command(flatten)]
    pub collection: Collection,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug, Clone)]
pub struct ShowArgs {
    /// Directory to rank, relative to the root or an absolute path inside it
    /// (default: the root)
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub selector: SnapshotSelector,
    #[command(flatten)]
    pub ranking: Ranking,
    /// Hide directories smaller than SIZE, such as 1GiB; with --metric
    /// inodes, an inode count
    #[arg(long, value_name = "SIZE", default_value = "0", value_parser = size)]
    pub min_size: u64,
    /// Rank by allocated bytes, apparent bytes, or inodes
    #[arg(long, value_name = "{allocated,apparent,inodes}", default_value = "allocated", value_parser = any_metric)]
    pub metric: Metric,
    /// Name each directory's largest owner, from a snapshot taken with
    /// --with-users, down to its user_max_depth
    #[arg(long)]
    pub owners: bool,
    #[command(flatten)]
    pub staleness: Staleness,
    #[command(flatten)]
    pub capacity: Capacity,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct DiffArgs {
    /// Directory to compare, relative to the root or an absolute path inside
    /// it (default: the root)
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub comparison: Comparison,
    #[command(flatten)]
    pub ranking: Ranking,
    /// Hide rows whose larger value is below SIZE, such as 1GiB; with
    /// --metric inodes, an inode count
    #[arg(long, value_name = "SIZE", default_value = "0", value_parser = size)]
    pub min_size: u64,
    /// Compare by allocated bytes, apparent bytes or inodes
    #[arg(long, value_name = "{allocated,apparent,inodes}", default_value = "allocated", value_parser = any_metric)]
    pub metric: Metric,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct HistoryArgs {
    /// Directory to follow, relative to the root or an absolute path inside
    /// it (default: the root)
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
    #[command(flatten)]
    pub profile: ProfileOption,
    /// Follow the N newest complete snapshots
    #[arg(short = 'n', long, value_name = "N", default_value = "20", value_parser = positive)]
    pub top: usize,
    /// Follow allocated bytes, apparent bytes, or inodes
    #[arg(long, value_name = "{allocated,apparent,inodes}", default_value = "allocated", value_parser = any_metric)]
    pub metric: Metric,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    /// Show the N newest snapshots
    #[arg(short = 'n', long, value_name = "N", default_value = "20", value_parser = positive)]
    pub top: usize,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct UsersArgs {
    /// Rank the owners of the files below PATH, relative to the root or an
    /// absolute path inside it (default: the root); PATH must be within
    /// the profile's user_max_depth
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub selector: SnapshotSelector,
    /// Show N owners
    #[arg(short = 'n', long, value_name = "N", default_value = "10", value_parser = positive)]
    pub top: usize,
    /// Show each owner's N largest directories
    #[arg(long, value_name = "N", default_value = "5", value_parser = non_negative)]
    pub dirs: u64,
    /// Rank owners by allocated bytes or inodes
    #[arg(long, value_name = "{allocated,inodes}", default_value = "allocated", value_parser = owner_metric)]
    pub metric: Metric,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct ErrorsArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    /// The snapshot, named as with show --snapshot
    #[arg(long, value_name = "SELECTOR", default_value = "latest-any", value_parser = Selector::parse)]
    pub snapshot: Selector,
    /// Show N errors
    #[arg(short = 'n', long, value_name = "N", default_value = "100", value_parser = positive)]
    pub top: usize,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct FilesArgs {
    /// Directory to search, relative to the root or an absolute path inside
    /// it (default: the root)
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub selector: SnapshotSelector,
    /// Scan the filesystem now instead of reading the snapshot's candidates
    #[arg(long)]
    pub live: bool,
    /// Root to scan with --live (default: the snapshot or profile root)
    #[arg(long = "path", value_name = "DIR", value_parser = expanded_path, requires = "live")]
    pub root: Option<PathBuf>,
    /// Show N files
    #[arg(short = 'n', long, value_name = "N", default_value = "30", value_parser = positive)]
    pub top: usize,
    /// Hide files smaller than SIZE, such as 1GiB
    #[arg(long, value_name = "SIZE", default_value = "0", value_parser = size)]
    pub min_size: u64,
    /// Only files owned by USER, a name or a numeric UID
    #[arg(long, value_name = "USER")]
    pub user: Option<String>,
    /// With --live, scan with N threads (default: $QDU_THREADS, or the CPU
    /// count up to 16)
    #[arg(long, value_name = "N", value_parser = positive, requires = "live")]
    pub threads: Option<usize>,
    #[command(flatten)]
    pub staleness: Staleness,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct BrowseArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub selector: SnapshotSelector,
    /// Include directories up to N levels below the scope
    #[arg(short = 'L', long, value_name = "N", default_value = "1", value_parser = positive)]
    pub max_depth: usize,
    /// Show N rows
    #[arg(short = 'n', long, value_name = "N", default_value = "20", value_parser = positive)]
    pub top: usize,
    /// Hide directories smaller than SIZE, such as 1GiB
    #[arg(long, value_name = "SIZE", default_value = "0", value_parser = size)]
    pub min_size: u64,
    /// Keep paths containing TEXT (ignoring case) or matching a glob such as 'models/*'
    #[arg(long = "match", value_name = "TEXT")]
    pub pattern: Option<String>,
    /// Rank by allocated bytes or apparent bytes
    #[arg(long, value_name = "{allocated,apparent}", default_value = "allocated", value_parser = size_metric)]
    pub metric: Metric,
    #[command(flatten)]
    pub staleness: Staleness,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// Directory for --growth-over, relative to the root or an absolute path
    /// inside it (default: the root)
    #[arg(value_name = "PATH")]
    pub path: Option<String>,
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub selector: SnapshotSelector,
    /// Alert when the directory grew by more than SIZE since the previous snapshot
    #[arg(long, value_name = "SIZE", value_parser = size)]
    pub growth_over: Option<u64>,
    /// Alert when the root's filesystem is fuller than PERCENT now
    #[arg(long, value_name = "PERCENT", value_parser = percentage)]
    pub disk_usage_over: Option<f64>,
    /// Alert when the root's filesystem uses more than PERCENT of its inodes
    #[arg(long, value_name = "PERCENT", value_parser = percentage)]
    pub inode_usage_over: Option<f64>,
    /// Alert when the checked snapshot is older than DURATION, such as 8d
    #[arg(long, value_name = "DURATION", value_parser = duration)]
    pub snapshot_age_over: Option<u64>,
    #[command(flatten)]
    pub capacity: Capacity,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Every {
    Daily,
    Weekly,
    Monthly,
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq)]
pub enum Weekday {
    Sun,
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
}

#[derive(Args, Debug)]
pub struct ScheduleArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    /// How often to take a snapshot; monthly runs on the 1st
    #[arg(long, value_enum, default_value = "weekly")]
    pub every: Every,
    /// Local time of day
    #[arg(long, value_name = "HH:MM", default_value = "03:15", value_parser = clock_time)]
    pub at: (u32, u32),
    /// Day of weekly snapshots (default: mon)
    #[arg(long, value_enum)]
    pub weekday: Option<Weekday>,
    /// qdu executable for cron to run (default: the one running now)
    #[arg(long = "qdu", value_name = "PATH")]
    pub executable: Option<String>,
    /// Add the entry to your crontab, replacing this profile's old entry
    #[arg(long, conflicts_with = "remove")]
    pub install: bool,
    /// Remove this profile's entry from your crontab
    #[arg(long)]
    pub remove: bool,
    #[command(flatten)]
    pub terminal: TerminalOptions,
}

#[derive(Args, Debug)]
pub struct CompactArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    /// Compress snapshots older than DURATION
    #[arg(long, value_name = "DURATION", default_value = "7d", value_parser = duration)]
    pub older_than: u64,
    /// Leave the N newest snapshots uncompressed
    #[arg(long, value_name = "N", default_value = "2", value_parser = non_negative)]
    pub keep_latest: u64,
    /// List what would be compressed
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub terminal: TerminalOptions,
}

#[derive(Args, Debug)]
pub struct VerifyArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub selector: SnapshotSelector,
    /// Verify every snapshot of the profile
    #[arg(long)]
    pub all: bool,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub output: OutputOptions,
}

#[derive(Args, Debug)]
pub struct RepairArgs {
    #[command(flatten)]
    pub profile: ProfileOption,
    /// List the actions without doing them
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub terminal: TerminalOptions,
}

#[derive(Args, Debug)]
pub struct LabelArgs {
    /// The label: letters, digits, '.', '_', or '-'
    #[arg(value_name = "NAME", value_parser = label_name)]
    pub name: String,
    #[command(flatten)]
    pub profile: ProfileOption,
    #[command(flatten)]
    pub selector: SnapshotSelector,
    /// Remove the label from the snapshot that has it
    #[arg(long, conflicts_with = "snapshot")]
    pub remove: bool,
    #[command(flatten)]
    pub terminal: TerminalOptions,
}

#[derive(Args, Debug)]
pub struct ForgetArgs {
    /// Snapshots to remove, named as with show --snapshot
    #[arg(value_name = "SELECTOR", required = true, value_parser = Selector::parse)]
    pub snapshots: Vec<Selector>,
    #[command(flatten)]
    pub profile: ProfileOption,
    /// List what would be removed
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub terminal: TerminalOptions,
}

#[derive(Args, Debug)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub action: ProfileAction,
}

#[derive(Subcommand, Debug)]
pub enum ProfileAction {
    /// List configured profiles
    List {
        #[command(flatten)]
        output: OutputOptions,
    },
    /// Show one profile's effective settings
    Show {
        /// Profile name (default: $QDU_PROFILE, or 'default')
        name: Option<String>,
        #[command(flatten)]
        output: OutputOptions,
    },
    /// Create a profile, or change the given settings of one
    #[command(visible_alias = "add")]
    Set(ProfileSetArgs),
    /// Remove a profile
    Remove {
        /// Profile name
        name: String,
        /// Also delete the profile's snapshots
        #[arg(long)]
        delete_data: bool,
        #[command(flatten)]
        terminal: TerminalOptions,
    },
}

#[derive(Args, Debug)]
pub struct ProfileSetArgs {
    /// Profile name
    pub name: String,
    /// Directory the profile scans; needed for a new profile
    #[arg(long = "path", value_name = "PATH", value_parser = expanded_path)]
    pub root: Option<PathBuf>,
    /// Exclusion pattern; repeat to replace the stored list
    #[arg(long, value_name = "PATTERN")]
    pub exclude: Vec<String>,
    #[command(flatten)]
    pub collection: Collection,
    #[command(flatten)]
    pub capacity: Capacity,
    #[command(flatten)]
    pub terminal: TerminalOptions,
}

#[derive(Args, Debug)]
pub struct CompletionsArgs {
    /// The shell to complete qdu in
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Print the configuration path
    Path {
        #[command(flatten)]
        terminal: TerminalOptions,
    },
    /// Create a starter configuration file
    Init {
        #[command(flatten)]
        terminal: TerminalOptions,
    },
}
