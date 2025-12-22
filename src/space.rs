// SPDX-License-Identifier: GPL-3.0-only

//! Disk space calculations using statvfs.

use std::path::Path;

use anyhow::{Context, Result};
use nix::sys::statvfs::statvfs;

/// Disk space information for a single mount point.
#[derive(Debug, Clone)]
pub struct SpaceInfo {
    /// Total bytes on the filesystem.
    pub total: u64,
    /// Used bytes.
    pub used: u64,
    /// Available bytes (may differ from total - used due to reserved blocks).
    #[allow(dead_code)]
    pub available: u64,
}

impl SpaceInfo {
    /// Returns usage as a percentage (0-100).
    pub fn percent_used(&self) -> u8 {
        if self.total == 0 {
            return 0;
        }
        ((self.used as f64 / self.total as f64) * 100.0).round() as u8
    }
}

/// Queries disk space for the given mount point.
pub fn get_space_info(mount_point: &Path) -> Result<SpaceInfo> {
    let stat = statvfs(mount_point)
        .with_context(|| format!("failed to statvfs {}", mount_point.display()))?;

    let block_size = stat.block_size() as u64;
    let total = stat.blocks() * block_size;
    let available = stat.blocks_available() * block_size;
    let free = stat.blocks_free() * block_size;

    // Used = total - free (not available, since available excludes reserved blocks)
    let used = total.saturating_sub(free);

    Ok(SpaceInfo {
        total,
        used,
        available,
    })
}

/// Formats bytes into a human-readable string (e.g., "1.5 GB").
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
