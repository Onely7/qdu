//! The capacity of the filesystems below a snapshot root, then and now.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde_json::{Value, json};

use crate::sys;
use crate::units::{format_size, group};

/// Filesystem capacity at snapshot time or now.
#[derive(Clone, Debug)]
pub struct FilesystemUsage {
    pub filesystem_count: u64,
    pub total_bytes: u64,
    /// Free to unprivileged users.
    pub available_bytes: u64,
    /// Free including the blocks reserved for root; snapshots before qdu
    /// 0.3 did not record it.
    pub free_bytes: Option<u64>,
    pub total_inodes: u64,
    pub available_inodes: u64,
}

impl FilesystemUsage {
    /// Used space as df counts it: what is not free.
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes
            .saturating_sub(self.free_bytes.unwrap_or(self.available_bytes))
    }

    /// Usage as df prints it: used over used plus available, so blocks
    /// reserved for root count as neither, and a full disk reads 100%.
    pub fn used_percent(&self) -> f64 {
        let usable = self.used_bytes().saturating_add(self.available_bytes);
        if usable == 0 {
            0.0
        } else {
            self.used_bytes() as f64 * 100.0 / usable as f64
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "filesystem_count": self.filesystem_count,
            "total_bytes": self.total_bytes,
            "available_bytes": self.available_bytes,
            "free_bytes": self.free_bytes,
            "used_bytes": self.used_bytes(),
            "used_percent": self.used_percent(),
            "total_inodes": self.total_inodes,
            "available_inodes": self.available_inodes,
        })
    }
}

/// Filesystem facts for a snapshot's report.
pub struct FilesystemInfo {
    pub cross_filesystems: bool,
    pub skipped_filesystem_count: u64,
    pub skipped_filesystems: Vec<String>,
    pub snapshot: FilesystemUsage,
    pub current: Option<FilesystemUsage>,
}

fn metadata_number(metadata: &HashMap<String, String>, key: &str) -> u64 {
    metadata
        .get(key)
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

fn metadata_lines(metadata: &HashMap<String, String>, key: &str) -> Vec<String> {
    metadata
        .get(key)
        .map(|value| {
            value
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl FilesystemInfo {
    pub fn new(metadata: &HashMap<String, String>, root: &Path) -> Self {
        let mut stored_paths = metadata_lines(metadata, "filesystem_paths");
        if stored_paths.is_empty() {
            stored_paths.push(".".into());
        }
        let filesystem_count = metadata
            .get("filesystem_count")
            .and_then(|value| value.parse().ok())
            .unwrap_or(stored_paths.len() as u64);
        FilesystemInfo {
            cross_filesystems: metadata.get("cross_filesystems").map(String::as_str)
                == Some("true"),
            skipped_filesystem_count: metadata_number(metadata, "skipped_filesystem_count"),
            skipped_filesystems: metadata_lines(metadata, "skipped_filesystems"),
            snapshot: FilesystemUsage {
                filesystem_count,
                total_bytes: metadata_number(metadata, "filesystem_total_bytes"),
                available_bytes: metadata_number(metadata, "filesystem_available_bytes"),
                free_bytes: metadata
                    .get("filesystem_free_bytes")
                    .and_then(|value| value.trim().parse().ok()),
                total_inodes: metadata_number(metadata, "filesystem_total_inodes"),
                available_inodes: metadata_number(metadata, "filesystem_available_inodes"),
            },
            current: taken_here(metadata)
                .then(|| current_capacity(root, &stored_paths))
                .flatten(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "mode": if self.cross_filesystems { "cross-filesystems" } else { "one-filesystem" },
            "skipped_filesystem_count": self.skipped_filesystem_count,
            "skipped_filesystems": self.skipped_filesystems,
            "snapshot": self.snapshot.to_json(),
            "current": self.current.as_ref().map(FilesystemUsage::to_json),
        })
    }

    fn effective(&self) -> (&FilesystemUsage, bool) {
        match &self.current {
            Some(current) => (current, true),
            None => (&self.snapshot, false),
        }
    }

    /// Disk usage as one line, preferring the current capacity.
    pub fn disk_text(&self) -> String {
        let (usage, is_current) = self.effective();
        if usage.total_bytes == 0 {
            return "capacity unavailable".into();
        }
        let mut text = format!(
            "{:.1}% used, {} available of {}",
            usage.used_percent(),
            format_size(usage.available_bytes),
            format_size(usage.total_bytes)
        );
        if !is_current {
            text.push_str(" at snapshot time");
        }
        if usage.filesystem_count > 1 {
            text.push_str(&format!(" across {} filesystems", usage.filesystem_count));
        }
        text
    }

    /// Inode usage as one line, preferring the current capacity.
    pub fn inode_text(&self) -> String {
        let (usage, is_current) = self.effective();
        if usage.total_inodes == 0 {
            return "inode counts unavailable".into();
        }
        let used = usage.total_inodes.saturating_sub(usage.available_inodes);
        let mut text = format!(
            "{:.1}% used, {} available of {}",
            used as f64 * 100.0 / usage.total_inodes as f64,
            group(i128::from(usage.available_inodes)),
            group(i128::from(usage.total_inodes))
        );
        if !is_current {
            text.push_str(" at snapshot time");
        }
        text
    }
}

/// Whether this host took the snapshot. A copy read on another host, such
/// as a dashboard that gathers several servers, would otherwise report the
/// capacity of whatever this host has at the same path.
pub fn taken_here(metadata: &HashMap<String, String>) -> bool {
    metadata
        .get("host")
        .is_none_or(|host| *host == sys::hostname())
}

fn current_capacity(root: &Path, relative_paths: &[String]) -> Option<FilesystemUsage> {
    let mut devices = HashSet::new();
    let mut usage = FilesystemUsage {
        filesystem_count: 0,
        total_bytes: 0,
        available_bytes: 0,
        free_bytes: Some(0),
        total_inodes: 0,
        available_inodes: 0,
    };
    for relative in relative_paths {
        let path = if relative == "." {
            root.to_path_buf()
        } else {
            root.join(relative)
        };
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if devices.contains(&metadata.dev()) {
            continue;
        }
        let Ok(value) = sys::statvfs(&path) else {
            continue;
        };
        devices.insert(metadata.dev());
        usage.total_bytes += value.total_bytes;
        usage.available_bytes += value.available_bytes;
        usage.free_bytes = usage.free_bytes.map(|free| free + value.free_bytes);
        usage.total_inodes += value.total_inodes;
        usage.available_inodes += value.available_inodes;
    }
    if devices.is_empty() {
        return None;
    }
    usage.filesystem_count = devices.len() as u64;
    Some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(total: u64, available: u64, free: Option<u64>) -> FilesystemUsage {
        FilesystemUsage {
            filesystem_count: 1,
            total_bytes: total,
            available_bytes: available,
            free_bytes: free,
            total_inodes: 0,
            available_inodes: 0,
        }
    }

    #[test]
    fn usage_counts_reserved_blocks_as_df_does() {
        // 100 blocks, 10 free, of which 5 are reserved for root: df shows
        // 90 used, 5 available, and 95%.
        let reserved = usage(100, 5, Some(10));
        assert_eq!(reserved.used_bytes(), 90);
        assert!((reserved.used_percent() - 90.0 * 100.0 / 95.0).abs() < 1e-9);
        // A full disk, with only the reserved blocks free, reads 100%.
        assert_eq!(usage(100, 0, Some(5)).used_percent(), 100.0);
        // Without free blocks on record, used is what is not available.
        assert_eq!(usage(100, 5, None).used_bytes(), 95);
        assert_eq!(usage(0, 0, None).used_percent(), 0.0);
    }
}
