//! Load and atomically persist qdu profile configuration.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;

use crate::error::{Error, PathContext, Result, usage};
use crate::sys;
use crate::units::{format_duration, format_exact_size, parse_duration, parse_size};

/// The profile named by `QDU_PROFILE`, or `default` when it is unset or
/// empty.
pub fn default_profile_name() -> String {
    std::env::var("QDU_PROFILE")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "default".to_string())
}

/// Reject names that cannot safely identify one profile path component.
pub fn validate_profile_name(name: &str) -> Result<()> {
    let mut characters = name.chars();
    let valid = name.len() <= 64
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        });
    if valid {
        Ok(())
    } else {
        Err(usage(format!(
            "invalid profile name {name:?}: use up to 64 letters, digits, '.', '_', or '-', \
             starting with a letter or digit"
        )))
    }
}

/// The effective XDG configuration directory.
pub fn xdg_config_home() -> PathBuf {
    xdg_directory("XDG_CONFIG_HOME", ".config")
}

/// The effective XDG state directory.
pub fn xdg_state_home() -> PathBuf {
    xdg_directory("XDG_STATE_HOME", ".local/state")
}

fn xdg_directory(variable: &str, fallback: &str) -> PathBuf {
    match std::env::var(variable) {
        Ok(value) if !value.is_empty() => sys::expand_user(&value),
        _ => sys::home_dir().join(fallback),
    }
}

/// Validated settings for one named scan root and its retention policy.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileConfig {
    pub name: String,
    pub root: PathBuf,
    pub excludes: Vec<String>,
    pub keep_snapshots: u64,
    pub compact_after_seconds: Option<u64>,
    pub collect_users: bool,
    pub user_max_depth: u32,
    pub large_file_limit: u64,
    pub record_max_depth: Option<u32>,
    pub cross_filesystems: bool,
    pub capacity_limit_bytes: Option<u64>,
    pub capacity_user: Option<String>,
}

impl ProfileConfig {
    fn check(&self) -> std::result::Result<(), String> {
        if self.capacity_limit_bytes == Some(0) {
            return Err("capacity_limit must be greater than zero".into());
        }
        if let Some(user) = &self.capacity_user {
            if user.trim().is_empty() {
                return Err("capacity_user must not be empty".into());
            }
            if self.capacity_limit_bytes.is_none() {
                return Err("capacity_user requires capacity_limit_bytes".into());
            }
            if !self.collect_users {
                return Err("capacity_user requires collect_users".into());
            }
        }
        Ok(())
    }
}

/// Profile settings given on the command line; `None` keeps the stored value.
///
/// Settings that can also be cleared use a nested `Option`: `Some(None)`
/// clears the stored value, as `--no-capacity-limit` does.
#[derive(Clone, Debug, Default)]
pub struct ProfileOverrides {
    pub root: Option<PathBuf>,
    pub excludes: Option<Vec<String>>,
    pub keep_snapshots: Option<u64>,
    pub compact_after_seconds: Option<Option<u64>>,
    pub collect_users: Option<bool>,
    pub user_max_depth: Option<u32>,
    pub large_file_limit: Option<u64>,
    pub record_max_depth: Option<Option<u32>>,
    pub cross_filesystems: Option<bool>,
    pub capacity_limit_bytes: Option<Option<u64>>,
    pub capacity_user: Option<Option<String>>,
}

impl ProfileOverrides {
    /// Configuration text for each given setting, keyed by configuration key.
    pub fn render(&self) -> Vec<(&'static str, String)> {
        let mut settings = Vec::new();
        if let Some(root) = &self.root {
            settings.push(("path", root.display().to_string()));
        }
        if let Some(excludes) = &self.excludes {
            settings.push(("exclude", excludes.join("\n")));
        }
        if let Some(value) = self.keep_snapshots {
            settings.push(("keep_snapshots", value.to_string()));
        }
        if let Some(value) = self.compact_after_seconds {
            settings.push(("compact_after", render_compact_after(value)));
        }
        if let Some(value) = self.collect_users {
            settings.push(("collect_users", render_bool(value)));
        }
        if let Some(value) = self.user_max_depth {
            settings.push(("user_max_depth", value.to_string()));
        }
        if let Some(value) = self.large_file_limit {
            settings.push(("large_file_limit", value.to_string()));
        }
        if let Some(value) = self.record_max_depth {
            settings.push(("record_max_depth", render_optional(value)));
        }
        if let Some(value) = self.cross_filesystems {
            settings.push(("cross_filesystems", render_bool(value)));
        }
        if let Some(value) = self.capacity_limit_bytes {
            settings.push((
                "capacity_limit",
                value.map(format_exact_size).unwrap_or_default(),
            ));
        }
        if let Some(value) = &self.capacity_user {
            settings.push(("capacity_user", value.clone().unwrap_or_default()));
        }
        settings
    }

    /// Return a validated profile with these settings applied.
    pub fn apply(&self, profile: &ProfileConfig) -> Result<ProfileConfig> {
        let mut updated = profile.clone();
        if let Some(root) = &self.root {
            updated.root = root.clone();
        }
        if let Some(excludes) = &self.excludes {
            updated.excludes = excludes.clone();
        }
        if let Some(value) = self.keep_snapshots {
            updated.keep_snapshots = value;
        }
        if let Some(value) = self.compact_after_seconds {
            updated.compact_after_seconds = value;
        }
        if let Some(value) = self.collect_users {
            updated.collect_users = value;
        }
        if let Some(value) = self.user_max_depth {
            updated.user_max_depth = value;
        }
        if let Some(value) = self.large_file_limit {
            updated.large_file_limit = value;
        }
        if let Some(value) = self.record_max_depth {
            updated.record_max_depth = value;
        }
        if let Some(value) = self.cross_filesystems {
            updated.cross_filesystems = value;
        }
        if let Some(value) = self.capacity_limit_bytes {
            updated.capacity_limit_bytes = value;
        }
        if let Some(value) = &self.capacity_user {
            updated.capacity_user = value.clone();
        }
        updated
            .check()
            .map_err(|message| usage(format!("invalid profile settings: {message}")))?;
        Ok(updated)
    }
}

/// Every profile setting with its configuration key and built-in default.
pub const SETTINGS: [(&str, &str); 11] = [
    ("path", "~"),
    ("exclude", ""),
    // About a year of weekly snapshots.
    ("keep_snapshots", "52"),
    ("compact_after", "7d"),
    ("collect_users", "false"),
    ("user_max_depth", "3"),
    ("large_file_limit", "1000"),
    ("record_max_depth", ""),
    ("cross_filesystems", "false"),
    ("capacity_limit", ""),
    ("capacity_user", ""),
];

/// Parse a compaction age, where `off` or `never` disables compaction.
pub fn parse_compact_after(raw: &str) -> Result<Option<u64>> {
    let value = raw.trim();
    if value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("never") {
        return Ok(None);
    }
    parse_duration(value).map(Some)
}

pub fn render_compact_after(seconds: Option<u64>) -> String {
    seconds.map(format_duration).unwrap_or_else(|| "off".into())
}

fn render_bool(value: bool) -> String {
    if value { "true" } else { "false" }.to_string()
}

fn render_optional(value: Option<u32>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn parse_bool(key: &str, raw: &str) -> std::result::Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        other => Err(format!(
            "invalid value for {key}: {other:?}; use true or false"
        )),
    }
}

fn parse_count<T: TryFrom<i128>>(key: &str, raw: &str) -> std::result::Result<T, String> {
    let value: i128 = raw
        .trim()
        .parse()
        .map_err(|_| format!("invalid integer for {key}: {:?}", raw.trim()))?;
    if value < 0 {
        return Err(format!("{key} must be non-negative"));
    }
    T::try_from(value).map_err(|_| format!("{key} is too large"))
}

fn parse_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// A configparser-compatible INI document: ordered sections of ordered keys.
#[derive(Clone, Debug, Default)]
pub struct Ini {
    sections: Vec<(String, Vec<(String, String)>)>,
}

impl Ini {
    /// Parse INI text the way Python's configparser reads qdu's files.
    pub fn parse(text: &str) -> std::result::Result<Ini, String> {
        let mut ini = Ini::default();
        let mut current_key: Option<(usize, usize)> = None;
        for (number, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            let number = number + 1;
            let stripped = line.trim();
            if stripped.is_empty() || stripped.starts_with('#') || stripped.starts_with(';') {
                continue;
            }
            let indented = line.starts_with(|character: char| character.is_whitespace());
            if indented {
                if let Some((section, key)) = current_key {
                    let value = &mut ini.sections[section].1[key].1;
                    if !value.is_empty() {
                        value.push('\n');
                    }
                    value.push_str(stripped);
                    continue;
                }
            }
            if stripped.starts_with('[') && stripped.ends_with(']') {
                let name = stripped[1..stripped.len() - 1].trim().to_string();
                if ini.section_index(&name).is_some() {
                    return Err(format!("line {number}: section [{name}] appears twice"));
                }
                ini.sections.push((name, Vec::new()));
                current_key = None;
                continue;
            }
            let Some(delimiter) = stripped.find(['=', ':']) else {
                return Err(format!("line {number}: expected 'key = value'"));
            };
            let Some(section) = ini.sections.len().checked_sub(1) else {
                return Err(format!("line {number}: setting outside a [section]"));
            };
            let key = stripped[..delimiter].trim().to_lowercase();
            let value = stripped[delimiter + 1..].trim().to_string();
            if key.is_empty() {
                return Err(format!("line {number}: setting without a name"));
            }
            let entries = &mut ini.sections[section].1;
            if entries.iter().any(|(existing, _)| *existing == key) {
                return Err(format!(
                    "line {number}: {key} appears twice in [{}]",
                    ini.sections[section].0
                ));
            }
            entries.push((key, value));
            current_key = Some((section, entries.len() - 1));
        }
        Ok(ini)
    }

    /// Render the document with multi-line values on indented lines.
    pub fn render(&self) -> String {
        let mut text = String::new();
        for (name, entries) in &self.sections {
            text.push_str(&format!("[{name}]\n"));
            for (key, value) in entries {
                if value.is_empty() {
                    text.push_str(&format!("{key} =\n"));
                } else if value.contains('\n') {
                    text.push_str(&format!("{key} =\n"));
                    for line in value.lines() {
                        text.push_str(&format!("\t{line}\n"));
                    }
                } else {
                    text.push_str(&format!("{key} = {value}\n"));
                }
            }
            text.push('\n');
        }
        text
    }

    fn section_index(&self, name: &str) -> Option<usize> {
        self.sections
            .iter()
            .position(|(section, _)| section == name)
    }

    pub fn sections(&self) -> impl Iterator<Item = &str> {
        self.sections.iter().map(|(name, _)| name.as_str())
    }

    pub fn has_section(&self, name: &str) -> bool {
        self.section_index(name).is_some()
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let index = self.section_index(section)?;
        self.sections[index]
            .1
            .iter()
            .find(|(existing, _)| existing == key)
            .map(|(_, value)| value.as_str())
    }

    pub fn set(&mut self, section: &str, key: &str, value: String) {
        let index = match self.section_index(section) {
            Some(index) => index,
            None => {
                self.sections.push((section.to_string(), Vec::new()));
                self.sections.len() - 1
            }
        };
        let entries = &mut self.sections[index].1;
        match entries.iter_mut().find(|(existing, _)| existing == key) {
            Some(entry) => entry.1 = value,
            None => entries.push((key.to_string(), value)),
        }
    }

    pub fn add_section(&mut self, section: &str) {
        if !self.has_section(section) {
            self.sections.push((section.to_string(), Vec::new()));
        }
    }

    pub fn remove_section(&mut self, section: &str) -> bool {
        match self.section_index(section) {
            Some(index) => {
                self.sections.remove(index);
                true
            }
            None => false,
        }
    }
}

/// Read and atomically write profile settings in one INI file.
pub struct ConfigRepository {
    pub path: PathBuf,
}

impl ConfigRepository {
    pub fn new() -> Self {
        ConfigRepository {
            path: xdg_config_home().join("qdu").join("config.ini"),
        }
    }

    fn read(&self) -> Result<Ini> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(Error::Runtime(format!(
                    "cannot read {}: {error}",
                    self.path.display()
                )));
            }
        };
        Ini::parse(&text).map_err(|message| {
            usage(format!(
                "invalid configuration file {}: {message}",
                self.path.display()
            ))
        })
    }

    fn write(&self, ini: &Ini) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .at(parent)?;
        }
        let temporary = self.path.with_extension("tmp");
        {
            let mut handle = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temporary)
                .at(&temporary)?;
            handle.write_all(ini.render().as_bytes()).at(&temporary)?;
            handle.sync_all().at(&temporary)?;
        }
        fs::rename(&temporary, &self.path).at(&self.path)?;
        Ok(())
    }

    /// Configured profile names plus the implicit `default` profile.
    pub fn list_profiles(&self) -> Result<Vec<String>> {
        let ini = self.read()?;
        let mut names: Vec<String> = ini
            .sections()
            .filter_map(|section| section.strip_prefix("profile:"))
            .map(str::to_string)
            .collect();
        names.push("default".to_string());
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Resolve `[defaults]` and the named section into validated settings.
    pub fn load_profile(&self, name: &str) -> Result<ProfileConfig> {
        validate_profile_name(name)?;
        profile_from(&self.read()?, name)
    }

    /// Whether the configuration has a section for the profile.
    pub fn has_profile(&self, name: &str) -> Result<bool> {
        validate_profile_name(name)?;
        Ok(self.read()?.has_section(&format!("profile:{name}")))
    }

    /// Whether the configuration sets a non-empty `path` for the profile,
    /// in its own section or in `[defaults]`.
    pub fn configures_path(&self, name: &str) -> Result<bool> {
        validate_profile_name(name)?;
        let ini = self.read()?;
        let section = format!("profile:{name}");
        Ok(ini
            .get(&section, "path")
            .or_else(|| ini.get("defaults", "path"))
            .is_some_and(|value| !value.trim().is_empty()))
    }

    /// Store the given settings and keep every other stored value.
    pub fn update_profile(
        &self,
        name: &str,
        settings: &[(&'static str, String)],
    ) -> Result<ProfileConfig> {
        validate_profile_name(name)?;
        let mut ini = self.read()?;
        let section = format!("profile:{name}");
        ini.add_section(&section);
        for (key, value) in settings {
            ini.set(&section, key, value.clone());
        }
        let profile = profile_from(&ini, name)?;
        self.write(&ini)?;
        Ok(profile)
    }

    /// Remove a profile section without deleting its snapshots.
    pub fn remove_profile(&self, name: &str) -> Result<bool> {
        validate_profile_name(name)?;
        let mut ini = self.read()?;
        let removed = ini.remove_section(&format!("profile:{name}"));
        if removed {
            self.write(&ini)?;
        }
        Ok(removed)
    }

    /// Write the starter `[defaults]` section: into a new file, or at the
    /// top of a file without one, such as the file that a profile's first
    /// snapshot creates. An existing `[defaults]` is left as it is.
    pub fn initialize(&self) -> Result<Initialized> {
        let exists = self.path.exists();
        let mut ini = if exists { self.read()? } else { Ini::default() };
        if ini.has_section("defaults") {
            return Ok(Initialized::Unchanged);
        }
        let entries = SETTINGS
            .iter()
            .filter(|(key, _)| *key != "path")
            .map(|(key, default)| {
                let value = if *key == "exclude" {
                    ".git\nnode_modules".to_string()
                } else {
                    default.to_string()
                };
                (key.to_string(), value)
            })
            .collect();
        ini.sections.insert(0, ("defaults".to_string(), entries));
        self.write(&ini)?;
        Ok(if exists {
            Initialized::AddedDefaults
        } else {
            Initialized::Created
        })
    }
}

/// What `qdu config init` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initialized {
    Created,
    AddedDefaults,
    Unchanged,
}

impl Default for ConfigRepository {
    fn default() -> Self {
        Self::new()
    }
}

fn profile_from(ini: &Ini, name: &str) -> Result<ProfileConfig> {
    let section = format!("profile:{name}");
    let raw = |key: &str, default: &'static str| -> String {
        ini.get(&section, key)
            .or_else(|| ini.get("defaults", key))
            .unwrap_or(default)
            .to_string()
    };
    let value = |key: &str| -> String {
        let default = SETTINGS
            .iter()
            .find(|(setting, _)| *setting == key)
            .map(|(_, default)| *default)
            .unwrap_or("");
        raw(key, default)
    };
    let invalid = |message: String| {
        usage(format!(
            "invalid configuration for profile '{name}': {message}"
        ))
    };
    let optional_size = |key: &str| -> std::result::Result<Option<u64>, String> {
        let text = value(key);
        if text.trim().is_empty() {
            Ok(None)
        } else {
            parse_size(&text)
                .map(Some)
                .map_err(|error| error.to_string())
        }
    };
    let optional_text = |key: &str| -> Option<String> {
        let text = value(key);
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    };
    let build = || -> std::result::Result<ProfileConfig, String> {
        let record_max_depth = {
            let text = value("record_max_depth");
            if text.trim().is_empty() {
                None
            } else {
                Some(parse_count::<u32>("record_max_depth", &text)?)
            }
        };
        let profile = ProfileConfig {
            name: name.to_string(),
            root: {
                let path = value("path");
                let path = path.trim();
                sys::expand_user(if path.is_empty() { "~" } else { path })
            },
            excludes: parse_lines(&value("exclude")),
            keep_snapshots: parse_count("keep_snapshots", &value("keep_snapshots"))?,
            compact_after_seconds: parse_compact_after(&value("compact_after"))
                .map_err(|error| error.to_string())?,
            collect_users: parse_bool("collect_users", &value("collect_users"))?,
            user_max_depth: parse_count("user_max_depth", &value("user_max_depth"))?,
            large_file_limit: parse_count("large_file_limit", &value("large_file_limit"))?,
            record_max_depth,
            cross_filesystems: parse_bool("cross_filesystems", &value("cross_filesystems"))?,
            capacity_limit_bytes: optional_size("capacity_limit")?,
            capacity_user: optional_text("capacity_user"),
        };
        profile.check()?;
        Ok(profile)
    };
    build().map_err(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_reads_configparser_files() {
        let ini = Ini::parse(
            "[defaults]\nKeep_Snapshots = 7\nexclude =\n    .git\n    node_modules\n\n\
             # comment\n[profile:data]\npath: /srv/data\n",
        )
        .unwrap();
        assert_eq!(ini.get("defaults", "keep_snapshots"), Some("7"));
        assert_eq!(ini.get("defaults", "exclude"), Some(".git\nnode_modules"));
        assert_eq!(ini.get("profile:data", "path"), Some("/srv/data"));
        assert!(Ini::parse("key = value\n").is_err());
        assert!(Ini::parse("[a]\nx = 1\nx = 2\n").is_err());
    }

    #[test]
    fn ini_round_trips_multi_line_values() {
        let mut ini = Ini::default();
        ini.set("defaults", "exclude", ".git\nnode_modules".into());
        ini.set("defaults", "capacity_user", String::new());
        let reread = Ini::parse(&ini.render()).unwrap();
        assert_eq!(
            reread.get("defaults", "exclude"),
            Some(".git\nnode_modules")
        );
        assert_eq!(reread.get("defaults", "capacity_user"), Some(""));
    }

    #[test]
    fn profile_names_are_single_safe_components() {
        for name in ["default", "data-2", "a.b_c"] {
            assert!(validate_profile_name(name).is_ok());
        }
        for name in ["", "../x", "/abs", ".hidden", "a/b", &"x".repeat(65)] {
            assert!(validate_profile_name(name).is_err(), "{name}");
        }
    }
}
